# -*- coding: utf-8 -*-
"""技能部署：把 data/skills 里的技能复制到各平台目录，并记录归属。

平台目录里已经存在、但不是本工具部署的同名技能不静默覆盖：
调用方拿到 conflict 后先问用户，再带 overwrite 重试。
"""
from __future__ import annotations

from datetime import datetime
from pathlib import Path

from . import store
from .base import Batch, HubError, Step
from .store import SkillInfo
from .targets import TARGET_MAP


def _now() -> str:
    return datetime.now().isoformat(timespec="seconds")


def entry(hub: dict, name: str) -> dict:
    item = hub["skills"].get(name)
    if item is None:
        item = {"targets": [], "deployed": {}}
        hub["skills"][name] = item
    item.setdefault("targets", [])
    item.setdefault("deployed", {})
    return item


def state(entry_item: dict, skill: SkillInfo | None) -> dict[str, str]:
    """每个平台的状态：synced / stale / missing / untracked。

    这里比对平台副本的真实内容，而不是只看 hub 里的记录：用户在平台目录里
    手动改过技能时，界面要能显示「待更新」，立即同步才会真的覆盖回去。
    """
    result: dict[str, str] = {}
    targets = entry_item.get("targets") or []
    deployed = entry_item.get("deployed") or {}
    if skill is None or not skill.valid:
        return {key: "missing" for key in targets}
    for key in targets:
        target = TARGET_MAP.get(key)
        if target is None:
            continue
        dest = target.skill_path(skill.name)
        if not dest.exists():
            result[key] = "missing"
            continue
        same = store.digest_dir(dest) == skill.digest
        if same:
            result[key] = "synced"
        elif key in deployed:
            result[key] = "stale"
        else:
            result[key] = "untracked"
    return result


def overall(entry_item: dict, skill: SkillInfo | None) -> str:
    if skill is not None and not skill.valid:
        return "invalid"
    targets = entry_item.get("targets") or []
    if not targets:
        return "unassigned"
    states = state(entry_item, skill)
    if any(value in ("missing", "stale") for value in states.values()):
        return "stale"
    if any(value == "untracked" for value in states.values()):
        return "untracked"
    return "synced"


def deploy(skill: SkillInfo, keys, hub: dict, overwrite: set[str] | None = None,
           batch: Batch | None = None) -> Batch:
    """把技能复制到指定平台；已是最新的平台跳过复制。"""
    batch = batch or Batch()
    if not skill.valid:
        raise HubError(f"技能「{skill.name}」无效：{skill.reason}")
    overwrite = overwrite or set()
    item = entry(hub, skill.name)
    for key in keys:
        target = TARGET_MAP.get(key)
        if target is None:
            batch.add(Step(False, key, "未知平台"))
            continue
        dest = target.skill_path(skill.name)
        record = (item.get("deployed") or {}).get(key)
        exists = dest.exists()
        if exists and store.is_link(dest):
            batch.add(Step(False, key, "目标是链接，未覆盖（请手动处理）", str(dest)))
            continue
        if exists and not record and key not in overwrite:
            # 平台里已经有同名目录：内容一样就直接接管，不一样先报冲突
            if store.digest_dir(dest) == skill.digest:
                item["deployed"][key] = {"at": _now(), "digest": skill.digest}
                batch.add(Step(True, key, "平台已有相同内容，已接管", str(dest)))
                continue
            batch.add(Step(False, key, "平台已有同名技能，未覆盖", str(dest),
                           conflict=True))
            continue
        if exists and store.digest_dir(dest) == skill.digest:
            item["deployed"][key] = {"at": _now(), "digest": skill.digest}
            batch.add(Step(True, key, "已是最新", str(dest)))
            continue
        try:
            if exists and not store.remove_path(dest):
                batch.add(Step(False, key, "旧副本删除失败（可能被客户端占用）",
                               str(dest)))
                continue
            store.copy_tree(skill.path, dest)
        except HubError as exc:
            batch.add(Step(False, key, str(exc), str(dest)))
            continue
        item["deployed"][key] = {"at": _now(), "digest": skill.digest}
        batch.add(Step(True, key, "已同步", str(dest)))
    return batch


def remove(name: str, keys, hub: dict, batch: Batch | None = None) -> Batch:
    """只删本工具部署过的副本；平台自带的同名技能不碰。"""
    batch = batch or Batch()
    item = hub["skills"].get(name)
    if item is None:
        return batch
    deployed = item.get("deployed") or {}
    for key in keys:
        target = TARGET_MAP.get(key)
        if target is None:
            continue
        record = deployed.get(key)
        dest = target.skill_path(name)
        if record is None:
            batch.add(Step(False, key, "不是本工具部署的，已跳过", str(dest),
                           skipped=True))
            continue
        if dest.exists() and store.is_link(dest):
            batch.add(Step(False, key, "目标是链接，未删除（请手动处理）", str(dest)))
            continue
        if dest.exists():
            try:
                store.move_to_trash(dest, f"platform-skills/{key}")
            except HubError as exc:
                batch.add(Step(False, key, str(exc), str(dest)))
                continue
        deployed.pop(key, None)
        item["targets"] = [value for value in item.get("targets") or [] if value != key]
        batch.add(Step(True, key, "已移除（副本在 data/trash）", str(dest)))
    return batch


def set_targets(name: str, keys, hub: dict, skill: SkillInfo | None,
                batch: Batch | None = None) -> Batch:
    """调整归属：取消勾选的平台删掉本工具副本，新勾选的立即部署。"""
    batch = batch or Batch()
    item = entry(hub, name)
    old = set(item.get("targets") or [])
    new = set(keys)
    removed = old - new
    if removed:
        remove(name, removed, hub, batch)
    item["targets"] = sorted(new)
    if skill is not None and new:
        deploy(skill, sorted(new - removed), hub, batch=batch)
    return batch


def scan_sources() -> list[dict]:
    """扫描各平台已有技能，供「从平台导入」使用。

    同名同内容只列一条，并记下它出现在哪些平台；导入后这些平台都算已同步。
    """
    groups: dict[tuple[str, str], dict] = {}
    for key, target in TARGET_MAP.items():
        root = target.skill_dir
        if not root.is_dir():
            continue
        try:
            children = sorted(root.iterdir(), key=lambda item: item.name.lower())
        except OSError:
            continue
        for child in children:
            if not child.is_dir() or child.name.startswith("."):
                continue
            if store.skill_md(child) is None:
                continue
            info = store.read_skill(child)
            signature = (child.name.lower(), info.digest)
            group = groups.get(signature)
            if group is None:
                group = {
                    "id": f"{child.name}|{info.digest}",
                    "name": info.name,
                    "path": str(child),
                    "description": info.description,
                    "valid": info.valid,
                    "sources": [],
                }
                groups[signature] = group
            if key not in group["sources"]:
                group["sources"].append(key)
    found = list(groups.values())
    for item in found:
        item["source"] = item["sources"][0] if item["sources"] else ""
        item["source_label"] = "、".join(TARGET_MAP[key].label
                                        for key in item["sources"])
    found.sort(key=lambda item: item["name"].lower())
    return found


def import_items(items: list[dict], hub: dict) -> dict:
    """把选中的平台技能复制进技能库；同名时自动加 -2 / -3 后缀。"""
    imported: list[str] = []
    skipped: list[str] = []
    errors: list[str] = []
    existing = {info.name.lower() for info in store.scan_library(hub)}
    for item in items:
        source = Path(str(item.get("path") or ""))
        if not source.is_dir() or store.skill_md(source) is None:
            skipped.append(f"{item.get('name')}（目录已不存在）")
            continue
        base = store.safe_name(item.get("name") or source.name)
        name = base
        index = 2
        while name.lower() in existing:
            name = f"{base}-{index}"
            index += 1
        try:
            folder = store.copy_into_library(source, hub, name=name)
        except HubError as exc:
            errors.append(f"{base}：{exc}")
            continue
        existing.add(name.lower())
        info = store.read_skill(folder)
        item_entry = entry(hub, name)
        sources = [key for key in (item.get("sources") or [item.get("source")])
                   if key in TARGET_MAP]
        item_entry["targets"] = sources
        for key in sources:
            item_entry["deployed"][key] = {"at": _now(), "digest": info.digest}
        imported.append(name)
    return {"imported": imported, "skipped": skipped, "errors": errors}


def sync_all(hub: dict, skills: dict[str, SkillInfo]) -> dict:
    """启动时自动同步：只补缺失和过期的平台，不覆盖平台自带的同名技能。"""
    changed = 0
    errors: list[str] = []
    conflicts: list[str] = []
    for name, item in list(hub["skills"].items()):
        skill = skills.get(name)
        if skill is None:
            errors.append(f"{name}：技能库中已不存在")
            continue
        keys = item.get("targets") or []
        if not keys:
            continue
        states = state(item, skill)
        pending = [key for key, value in states.items() if value in ("missing", "stale")]
        if not pending:
            continue
        batch = deploy(skill, pending, hub)
        changed += batch.ok_count
        conflicts.extend(f"{name} → {TARGET_MAP[step.target].label}"
                         for step in batch.conflicts if step.target in TARGET_MAP)
        errors.extend(f"{name} → {TARGET_MAP[step.target].label}：{step.message}"
                      for step in batch.failed if step.target in TARGET_MAP)
    return {"changed": changed, "errors": errors, "conflicts": conflicts}
