# -*- coding: utf-8 -*-
"""平台技能盘点：把每个平台实际存在的技能都列出来，分清「官方」和「个人」。

官方 = 随客户端分发的内置技能（安装目录里的 bundled / builtin 目录）；
个人 = 平台用户目录（~/.xxx/skills）里自己加的，包括本工具部署出去的副本。
两类分开是因为内置技能只读、不能删，用户目录里的才能导入和清理。
"""
from __future__ import annotations

import os
from pathlib import Path

from ..core import paths
from . import store
from .targets import TARGETS, Target, short_path

# 各平台安装根目录的候选位置（按优先级）。平台只装在标准位置，
# 这里照抄各客户端适配器的查找顺序，hub 不反过来依赖签到域。
_CANDIDATE_ROOTS: dict[str, tuple[str, ...]] = {
    "qoder_cn": (r"C:\Program Files\Qoder CN",),
    "qoder_intl": (r"C:\Program Files\Qoder",),
    "trae_cn": (r"C:\Program Files\Trae CN",),
    "workbuddy_cn": (r"C:\App\WB\CN",),
    "workbuddy_intl": (r"C:\App\WB\AI",),
    "codebuddy": (r"C:\Program Files\CodeBuddy",),
    "zcode": (),
}

# 内置技能目录相对安装根的 glob。每个平台的布局不一样，只能按实际观测到的写。
_BUILTIN_GLOBS: dict[str, tuple[str, ...]] = {
    "workbuddy_cn": ("resources/app.asar.unpacked/resources/plugins/*/skills",),
    "workbuddy_intl": ("resources/app.asar.unpacked/resources/plugins/*/skills",),
    "zcode": ("resources/glm/packages/*/skills",),
    "qoder_cn": (".qoder-versions/*/resources/*/skills", "resources/*/skills"),
    "qoder_intl": (".qoder-versions/*/resources/*/skills", "resources/*/skills"),
    "trae_cn": ("resources/app/extensions/*/skills",),
    "codebuddy": ("resources/app.asar.unpacked/**/skills",),
}

_PROGRAMS = paths.user_local_dir() / "Programs"


def _roots(target: Target) -> list[Path]:
    """候选安装根目录：程序目录下的标准名字优先，再看固定路径。"""
    names = {
        "qoder_cn": ("Qoder CN",),
        "qoder_intl": ("Qoder",),
        "trae_cn": ("Trae CN", "TRAE SOLO CN"),
        "workbuddy_cn": ("WorkBuddy", "CodeBuddy CN"),
        "workbuddy_intl": ("WorkBuddyAI",),
        "codebuddy": ("CodeBuddy", "CodeBuddy CN"),
        "zcode": ("ZCode",),
    }.get(target.key, ())
    out = [_PROGRAMS / name for name in names]
    out.extend(Path(raw) for raw in _CANDIDATE_ROOTS.get(target.key, ()))
    return [root for root in out if root.is_dir()]


def builtin_dirs(target: Target) -> list[Path]:
    """内置技能目录：容器（skills/ 下一层才是技能）和单技能目录两种布局都收。"""
    found: list[Path] = []
    for root in _roots(target):
        for pattern in _BUILTIN_GLOBS.get(target.key, ()):
            try:
                matches = [p for p in root.glob(pattern) if p.is_dir()]
            except OSError:
                continue
            for path in matches:
                if path not in found and (_skill_dirs(path) or store.skill_md(path)):
                    found.append(path)
    return found


def _skill_dirs(root: Path) -> list[Path]:
    if not root.is_dir():
        return []
    try:
        children = sorted(root.iterdir(), key=lambda item: item.name.lower())
    except OSError:
        return []
    return [child for child in children
            if child.is_dir() and not child.name.startswith(".")
            and store.skill_md(child) is not None]


def _collect(folder: Path) -> list[Path]:
    """一个内置目录里可能既有技能子目录，也可能本身就是个技能（Qoder 布局）。"""
    out = _skill_dirs(folder)
    if store.skill_md(folder) is not None:
        out.append(folder)
    return out


def _display_name(folder: Path) -> str:
    """优先用 SKILL.md frontmatter 里的 name：Qoder 的技能目录就叫 skills。"""
    entry = store.skill_md(folder)
    if entry is not None:
        try:
            meta = store.parse_frontmatter(
                entry.read_text(encoding="utf-8", errors="replace"))
        except OSError:
            meta = {}
        name = str(meta.get("name") or "").strip()
        if name:
            return name
    return folder.name


def scan() -> list[dict]:
    """列出每个平台的全部技能；同名时内置优先，用户目录副本标记为覆盖。"""
    out: list[dict] = []
    for target in TARGETS:
        official: dict[str, Path] = {}
        for folder in builtin_dirs(target):
            for child in _collect(folder):
                official.setdefault(_display_name(child).lower(), child)
        personal = {_display_name(child).lower(): child
                    for child in _skill_dirs(target.skill_dir)}

        for key, folder in {**official, **personal}.items():
            live = personal.get(key) or folder     # 用户目录里的副本才是实际生效的
            info = store.read_skill(live)
            is_official = key in official
            out.append({
                "id": f"{target.key}|{key}",
                "name": _display_name(live),
                "platform_key": target.key,
                "platform_label": target.label,
                "group": target.group,
                "kind": "official" if is_official else "personal",
                "kind_label": "官方" if is_official else "个人",
                "path": str(live),
                "path_text": short_path(live),
                "builtin_path": str(official[key]) if key in official else "",
                "description": info.description,
                "valid": info.valid,
                "reason": info.reason,
                "files": info.files,
                "size": info.size,
                # 内置 + 用户目录同名：用户那份是覆盖副本，删了才回到内置
                "override": bool(is_official and key in personal),
            })
    out.sort(key=lambda item: (item["group"], item["platform_label"],
                               item["kind"] != "official", item["name"].lower()))
    return out


def platform_summary(items: list[dict]) -> list[dict]:
    """按平台汇总官方/个人数量，给筛选条显示。"""
    buckets: dict[str, dict] = {}
    for item in items:
        bucket = buckets.setdefault(item["platform_key"], {
            "key": item["platform_key"],
            "label": item["platform_label"],
            "group": item["group"],
            "official": 0,
            "personal": 0,
        })
        bucket["official" if item["kind"] == "official" else "personal"] += 1
    return list(buckets.values())


def delete_personal(item: dict) -> str:
    """删除用户目录里的个人技能；内置技能只读，直接拒绝。

    走回收站而不是直接删：手滑删掉自己写的技能要能找回来。
    """
    if item.get("kind") != "personal":
        raise store.HubError("内置技能随客户端分发，不能删除")
    folder = Path(str(item.get("path") or ""))
    if not folder.is_dir():
        raise store.HubError("目录已不存在")
    if item.get("override") and item.get("builtin_path"):
        raise store.HubError("这是内置技能的用户副本，请用「删除」以外的方式恢复内置版本")
    store.move_to_trash(folder, f"platform-skills/{item.get('platform_key')}")
    return f"已移到回收站：{folder.name}"


def open_dir(item: dict) -> bool:
    from ..core import winproc
    folder = Path(str(item.get("path") or ""))
    return folder.is_dir() and winproc.open_path(folder)


def size_text(item: dict) -> str:
    from ..core.format import size as fmt_size
    return fmt_size(item.get("size") or 0) if item.get("size") else "—"
