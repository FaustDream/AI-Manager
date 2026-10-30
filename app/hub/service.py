# -*- coding: utf-8 -*-
"""技能 / MCP 的编排层：界面只调这里，文件读写与平台写入都在后台线程完成。"""
from __future__ import annotations

from pathlib import Path

from ..core import tasks
from . import mcp, platforms, skills, store, targets
from .base import Batch, HubError


def _hub() -> dict:
    return store.load()


def counts() -> dict:
    """总览卡片用：只数已登记的条目，不扫描磁盘 / 不算摘要。"""
    hub = _hub()
    return {"skills": len(hub["skills"]), "mcp": len(hub["mcp"])}


def snapshot() -> dict:
    """页面一次加载需要的全部数据：平台、技能、MCP 与各自状态。"""
    hub = store.load()
    found = store.scan_library(hub)
    by_name = {info.name: info for info in found}
    mcp_current, mcp_errors = mcp.read_all()

    target_rows = [{
        "key": target.key,
        "label": target.label,
        "group": target.group,
        "installed": target.installed,
        "skill_dir": str(target.skill_dir),
        "mcp_file": str(target.mcp_file),
        "note": target.note,
    } for target in targets.TARGETS]

    skill_rows: list[dict] = []
    for info in found:
        item = hub["skills"].get(info.name) or {"targets": [], "deployed": {}}
        skill_rows.append({
            "id": info.name,
            "name": info.name,
            "description": info.description,
            "path": str(info.path),
            "files": info.files,
            "size": info.size,
            "digest": info.digest,
            "updated": info.updated,
            "valid": info.valid,
            "reason": info.reason,
            "targets": list(item.get("targets") or []),
            "states": skills.state(item, info),
            "state": skills.overall(item, info),
        })
    for name, item in hub["skills"].items():
        if name in by_name:
            continue
        skill_rows.append({
            "id": name,
            "name": name,
            "description": "技能库中找不到源目录",
            "path": "",
            "files": 0,
            "size": 0,
            "digest": "",
            "updated": 0.0,
            "valid": False,
            "reason": "技能库中找不到源目录",
            "targets": list(item.get("targets") or []),
            "states": {},
            "state": "invalid",
        })
    skill_rows.sort(key=lambda row: row["name"].lower())

    mcp_rows: list[dict] = []
    for name, item in hub["mcp"].items():
        config = item.get("config") or {}
        states = {
            key: mcp.state(item, name, key,
                           (mcp_current.get(key) or {}).get(name))
            for key in item.get("targets") or []
        }
        mcp_rows.append({
            "id": name,
            "name": name,
            "config": config,
            "kind": mcp.kind(config),
            "summary": mcp.summary(config),
            "targets": list(item.get("targets") or []),
            "states": states,
            "state": mcp.overall(item, mcp_current, name),
            "preserve": bool(item.get("preserve")),
        })
    mcp_rows.sort(key=lambda row: row["name"].lower())
    untracked = sorted({
        name
        for servers in mcp_current.values()
        for name in servers
        if name not in hub["mcp"]
    })

    known = {info.name.lower() for info in found}
    skill_untracked: set[str] = set()
    for target in targets.TARGETS:
        root = target.skill_dir
        if not root.is_dir():
            continue
        try:
            children = list(root.iterdir())
        except OSError:
            continue
        for child in children:
            if not child.is_dir() or child.name.startswith("."):
                continue
            if child.name.lower() in known:
                continue
            if store.skill_md(child) is not None:
                skill_untracked.add(child.name)

    return {
        "hub": hub,
        "targets": target_rows,
        "skills": skill_rows,
        "skill_untracked": sorted(skill_untracked),
        "mcp": mcp_rows,
        "mcp_errors": mcp_errors,
        "mcp_untracked": untracked,
        "library": str(store.library_dir(hub)),
        # 各平台实际存在的技能（官方 + 个人）：随快照一起回来，页面不用再手动点刷新
        "platform_skills": platforms.scan(),
    }


def counts() -> dict:
    """总览卡片用的轻量计数：只数目录与登记项。

    不能复用 snapshot()：那里每个技能都算全量摘要，为两个数字读几百个文件不值。
    """
    hub = store.load()
    names = set(hub["skills"])
    try:
        for child in store.library_dir(hub).iterdir():
            if child.is_dir() and not child.name.startswith("."):
                names.add(child.name)
    except OSError:
        pass
    return {"skills": len(names), "mcp": len(hub["mcp"])}


# ------------------------------------------------------------------ 技能动作
def add_skill(src: Path, keys, overwrite: bool = False) -> dict:
    hub = _hub()
    folder = store.copy_into_library(src, hub, overwrite=overwrite)
    info = store.read_skill(folder)
    if not info.valid:
        raise HubError(f"技能「{info.name}」无效：{info.reason}")
    item = skills.entry(hub, info.name)
    item["targets"] = [key for key in keys if key in targets.TARGET_MAP]
    batch = skills.deploy(info, item["targets"], hub)
    store.save(hub)
    return {"name": info.name, "batch": batch}


def set_skill_targets(name: str, keys) -> dict:
    hub = _hub()
    info = next((item for item in store.scan_library(hub) if item.name == name), None)
    if info is None:
        raise HubError(f"技能库里找不到「{name}」")
    batch = skills.set_targets(name, keys, hub, info)
    store.save(hub)
    return {"batch": batch}


def remove_skill(name: str) -> dict:
    hub = _hub()
    item = hub["skills"].get(name) or {"targets": [], "deployed": {}}
    batch = skills.remove(name, list(item.get("targets") or []), hub)
    if batch.failed:
        store.save(hub)
        return {"batch": batch}
    folder = store.library_dir(hub) / name
    if folder.exists():
        try:
            store.move_to_trash(folder, "skills")
        except HubError:
            store.save(hub)
            raise
    hub["skills"].pop(name, None)
    store.save(hub)
    return {"batch": batch}


def scan_skill_sources() -> list[dict]:
    return skills.scan_sources()


def import_skills(items: list[dict]) -> dict:
    hub = _hub()
    result = skills.import_items(items, hub)
    store.save(hub)
    return result


# ------------------------------------------------------------------ MCP 动作
def add_mcp(name: str, config: dict, keys) -> dict:
    hub = _hub()
    item = mcp.entry(hub, name, config)
    item["targets"] = [key for key in keys if key in targets.TARGET_MAP]
    batch = mcp.deploy(name, config, item["targets"], hub)
    store.save(hub)
    return {"batch": batch}


def update_mcp(name: str, config: dict) -> dict:
    hub = _hub()
    item = mcp.entry(hub, name, config)
    keys = list(item.get("targets") or [])
    batch = mcp.deploy(name, config, keys, hub, overwrite=set(keys))
    item["preserve"] = False
    store.save(hub)
    return {"batch": batch}


def set_mcp_targets(name: str, keys) -> dict:
    hub = _hub()
    if name not in hub["mcp"]:
        raise HubError(f"统一管理里找不到「{name}」")
    batch = mcp.set_targets(name, keys, hub)
    store.save(hub)
    return {"batch": batch}


def remove_mcp(name: str) -> dict:
    hub = _hub()
    item = hub["mcp"].get(name) or {"targets": [], "deployed": {}}
    batch = mcp.remove(name, list(item.get("targets") or []), hub)
    if not batch.failed:
        hub["mcp"].pop(name, None)
    store.save(hub)
    return {"batch": batch}


def scan_mcp_sources() -> list[dict]:
    return mcp.scan_sources()


def parse_mcp_text(text: str) -> list[tuple[str, dict]]:
    return mcp.parse_text(text)


def import_mcp(items: list[dict]) -> dict:
    """把平台现有 MCP 纳入统一管理。

    来源平台保留各自原来的写法，不在这里改写它们的配置；合并后的配置
    只用于以后新增的平台，避免把本来能用的平台配置换成不兼容的版本。
    """
    hub = _hub()
    result = mcp.import_items(items, hub)
    store.save(hub)
    return result


def import_payload(entries: list[tuple[str, dict]], keys) -> dict:
    """从粘贴 / 文件导入：一次写入所有选中平台，不需要逐个平台改配置。"""
    hub = _hub()
    keys = [key for key in keys if key in targets.TARGET_MAP]
    imported: list[str] = []
    skipped: list[str] = []
    batch = Batch()
    for name, config in entries:
        name = str(name or "").strip()
        if not name:
            continue
        if name in hub["mcp"]:
            skipped.append(name)
            continue
        cfg = mcp.normalize_config(config)
        if not cfg.get("command") and not cfg.get("url"):
            skipped.append(f"{name}（没有 command / url）")
            continue
        item = mcp.entry(hub, name, cfg)
        item["config"] = cfg
        item["targets"] = list(keys)
        mcp.deploy(name, cfg, keys, hub, overwrite=set(keys), batch=batch)
        imported.append(name)
    store.save(hub)
    return {"imported": imported, "skipped": skipped, "batch": batch}


# ------------------------------------------------------------------ 全局
def auto_sync() -> dict:
    hub = _hub()
    if not hub.get("auto_sync", True):
        return {"changed": 0, "errors": [], "conflicts": [], "skipped": True}
    if not hub["skills"] and not hub["mcp"]:
        return {"changed": 0, "errors": [], "conflicts": [], "skipped": True}
    found = {info.name: info for info in store.scan_library(hub)}
    skill_result = skills.sync_all(hub, found)
    mcp_result = mcp.sync_all(hub)
    changed = skill_result["changed"] + mcp_result["changed"]
    if changed:
        store.save(hub)
    return {
        "changed": changed,
        "errors": skill_result["errors"] + mcp_result["errors"],
        "conflicts": skill_result["conflicts"] + mcp_result["conflicts"],
        "skipped": False,
    }


def set_auto_sync(enabled: bool) -> None:
    hub = _hub()
    hub["auto_sync"] = bool(enabled)
    store.save(hub)


def set_default_targets(keys) -> None:
    hub = _hub()
    hub["default_targets"] = [key for key in keys if key in targets.TARGET_MAP]
    store.save(hub)


def ensure_library() -> Path:
    return store.ensure_library(_hub())


def sync_now(name: str, kind: str) -> dict:
    """手动「立即同步」单个条目：技能或 MCP。"""
    hub = _hub()
    if kind == "skill":
        info = next((item for item in store.scan_library(hub)
                     if item.name == name), None)
        if info is None:
            raise HubError(f"技能库里找不到「{name}」")
        item = hub["skills"].get(name) or {"targets": [], "deployed": {}}
        batch = skills.deploy(info, list(item.get("targets") or []), hub,
                              overwrite=set(item.get("targets") or []))
    else:
        item = hub["mcp"].get(name)
        if item is None:
            raise HubError(f"统一管理里找不到「{name}」")
        batch = mcp.deploy(name, item.get("config") or {},
                           list(item.get("targets") or []), hub,
                           overwrite=set(item.get("targets") or []))
        item["preserve"] = False
    store.save(hub)
    return {"batch": batch}


def sync_all() -> dict:
    """手动同步全部：技能 + MCP，等同于把每条都重新推一遍。"""
    hub = _hub()
    found = {info.name: info for info in store.scan_library(hub)}
    total = Batch()
    entries = [(name, item) for name, item in hub["skills"].items()
               if found.get(name) is not None]
    steps = max(1, len(entries) + len(hub["mcp"]))
    for index, (name, item) in enumerate(entries, 1):
        tasks.report(index / steps, f"同步技能 {name}")
        keys = list(item.get("targets") or [])
        skills.deploy(found[name], keys, hub, overwrite=set(keys), batch=total)
    for name, item in hub["mcp"].items():
        keys = list(item.get("targets") or [])
        mcp.deploy(name, item.get("config") or {}, keys, hub,
                   overwrite=set(keys), batch=total)
        if keys:
            item["preserve"] = False
    store.save(hub)
    return {"batch": total}
