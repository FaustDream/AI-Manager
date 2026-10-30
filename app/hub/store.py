# -*- coding: utf-8 -*-
"""技能库与 hub.json：登记「哪个技能 / MCP 要同步到哪些平台」。

技能实体只存一份在 data/skills/<名称>/，各平台目录是它部署出去的副本；
hub.json 记录归属与内容摘要，用来判断「已同步 / 待更新 / 平台本来就有」。
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import stat
import time
from dataclasses import dataclass
from pathlib import Path

from ..core import paths
from .base import HubError
from .targets import TARGET_MAP

HUB_VERSION = 1
SKILL_FILE = "SKILL.md"
_SKIP_DIRS = {".git", "__pycache__", ".idea", ".vscode"}
_BAD_CHARS = re.compile(r'[<>:"/\\|?*\x00-\x1f]')


def defaults() -> dict:
    return {
        "version": HUB_VERSION,
        "library": str(paths.SKILL_LIBRARY),
        "auto_sync": True,
        "default_targets": [],
        "skills": {},
        "mcp": {},
    }


def _coerce(raw) -> dict:
    hub = defaults()
    if not isinstance(raw, dict):
        return hub
    hub["auto_sync"] = bool(raw.get("auto_sync", True))
    library = raw.get("library")
    if isinstance(library, str) and library.strip():
        hub["library"] = library
    hub["default_targets"] = [key for key in raw.get("default_targets") or []
                              if key in TARGET_MAP]

    for name, item in (raw.get("skills") or {}).items():
        if not isinstance(item, dict) or not isinstance(name, str):
            continue
        deployed = {}
        for key, record in (item.get("deployed") or {}).items():
            if key not in TARGET_MAP or not isinstance(record, dict):
                continue
            deployed[key] = {
                "at": str(record.get("at") or ""),
                "digest": str(record.get("digest") or ""),
            }
        hub["skills"][name] = {
            "targets": [key for key in item.get("targets") or [] if key in TARGET_MAP],
            "deployed": deployed,
        }

    for name, item in (raw.get("mcp") or {}).items():
        if not isinstance(item, dict) or not isinstance(name, str):
            continue
        config = item.get("config")
        if not isinstance(config, dict):
            continue
        deployed = {}
        for key, record in (item.get("deployed") or {}).items():
            if key not in TARGET_MAP or not isinstance(record, dict):
                continue
            deployed[key] = {
                "at": str(record.get("at") or ""),
                "hash": str(record.get("hash") or ""),
            }
        hub["mcp"][name] = {
            "config": config,
            "targets": [key for key in item.get("targets") or [] if key in TARGET_MAP],
            "deployed": deployed,
            # 从平台导入的条目保留各平台原配置，自动同步不覆盖
            "preserve": bool(item.get("preserve", False)),
        }
    return hub


def load() -> dict:
    try:
        raw = json.loads(paths.HUB_FILE.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return defaults()
    return _coerce(raw)


def save(hub: dict) -> None:
    """先写临时文件再替换；失败要抛出，让界面提示，而不是静默丢配置。"""
    path = paths.HUB_FILE
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_suffix(".json.tmp")
        tmp.write_text(json.dumps(hub, ensure_ascii=False, indent=2), encoding="utf-8")
        os.replace(tmp, path)
    except OSError as exc:
        raise HubError(f"写入 {path} 失败：{exc}") from exc


# ------------------------------------------------------------------ 技能库
def library_dir(hub: dict) -> Path:
    raw = str(hub.get("library") or paths.SKILL_LIBRARY)
    path = Path(raw).expanduser()
    if not path.is_absolute():
        path = paths.DATA_DIR / path
    return path


def ensure_library(hub: dict) -> Path:
    path = library_dir(hub)
    try:
        path.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        raise HubError(f"创建技能库 {path} 失败：{exc}") from exc
    return path


def safe_name(name: str) -> str:
    value = _BAD_CHARS.sub("-", str(name or "").strip()).strip(" .")
    value = re.sub(r"-{2,}", "-", value)
    if not value or value in (".", ".."):
        raise HubError("技能名称不能为空，也不能包含路径分隔符")
    return value[:80]


def skill_md(folder: Path) -> Path | None:
    """SKILL.md 的入口文件；Windows 大小写不敏感，但别依赖这一点。"""
    try:
        for child in folder.iterdir():
            if child.is_file() and child.name.lower() == SKILL_FILE.lower():
                return child
    except OSError:
        return None
    return None


def parse_frontmatter(text: str) -> dict:
    """只解析 SKILL.md 头部需要的 name / description，不引第三方 YAML。

    支持 `description: >-` / `|` 这类多行写法，其余字段原样保留为字符串。
    """
    lines = text.splitlines()
    if not lines or lines[0].strip() != "---":
        return {}
    end = None
    for index in range(1, len(lines)):
        if lines[index].strip() == "---":
            end = index
            break
    if end is None:
        return {}
    meta: dict[str, str] = {}
    index = 1
    while index < end:
        match = re.match(r"^([A-Za-z0-9_-]+)\s*:\s*(.*)$", lines[index])
        if not match:
            index += 1
            continue
        key, value = match.group(1), match.group(2).strip()
        if value in (">", ">-", ">+", "|", "|-", "|+"):
            block: list[str] = []
            index += 1
            while index < end and (not lines[index].strip()
                                   or lines[index][:1] in (" ", "\t")):
                block.append(lines[index].strip())
                index += 1
            value = " ".join(part for part in block if part)
        else:
            index += 1
        if len(value) >= 2 and value[0] == value[-1] and value[0] in "'\"":
            value = value[1:-1]
        meta[key] = value
    return meta


def digest_dir(root: Path) -> str:
    """内容摘要：路径 + 文件内容，用来判断平台副本是否需要更新。"""
    hasher = hashlib.blake2b(digest_size=16)
    try:
        entries = sorted(root.rglob("*"))
    except OSError:
        return ""
    for path in entries:
        if not path.is_file():
            continue
        relative = path.relative_to(root)
        if any(part in _SKIP_DIRS for part in relative.parts):
            continue
        hasher.update(relative.as_posix().encode("utf-8"))
        hasher.update(b"\0")
        try:
            size = path.stat().st_size
        except OSError:
            hasher.update(b"<missing>")
            continue
        if size > 4 * 1024 * 1024:
            # 大文件不做全量摘要：把大小与修改时间算进去，足够判断是否变过
            try:
                hasher.update(f"{size}:{path.stat().st_mtime_ns}".encode("ascii"))
            except OSError:
                hasher.update(str(size).encode("ascii"))
            continue
        try:
            with path.open("rb") as handle:
                for chunk in iter(lambda: handle.read(1 << 20), b""):
                    hasher.update(chunk)
        except OSError:
            hasher.update(b"<unreadable>")
        hasher.update(b"\0")
    return hasher.hexdigest()


@dataclass
class SkillInfo:
    name: str
    path: Path
    description: str = ""
    files: int = 0
    size: int = 0
    digest: str = ""
    updated: float = 0.0
    valid: bool = True
    reason: str = ""


def read_skill(folder: Path) -> SkillInfo:
    entry = skill_md(folder)
    updated = 0.0
    try:
        updated = folder.stat().st_mtime
    except OSError:
        pass
    if entry is None:
        return SkillInfo(folder.name, folder, updated=updated, valid=False,
                         reason="缺少 SKILL.md")
    try:
        meta = parse_frontmatter(entry.read_text(encoding="utf-8", errors="replace"))
    except OSError as exc:
        return SkillInfo(folder.name, folder, updated=updated, valid=False,
                         reason=f"读取 SKILL.md 失败：{exc}")
    files = 0
    size = 0
    try:
        for path in folder.rglob("*"):
            if path.is_file() and not any(part in _SKIP_DIRS
                                          for part in path.relative_to(folder).parts):
                files += 1
                try:
                    stat_info = path.stat()
                except OSError:
                    continue
                size += stat_info.st_size
                updated = max(updated, stat_info.st_mtime)
    except OSError:
        pass
    return SkillInfo(
        name=folder.name,
        path=folder,
        description=str(meta.get("description") or "").strip(),
        files=files,
        size=size,
        digest=digest_dir(folder),
        updated=updated,
    )


def scan_library(hub: dict) -> list[SkillInfo]:
    root = ensure_library(hub)
    items: list[SkillInfo] = []
    try:
        children = sorted(root.iterdir(), key=lambda item: item.name.lower())
    except OSError as exc:
        raise HubError(f"读取技能库 {root} 失败：{exc}") from exc
    for child in children:
        if not child.is_dir() or child.name.startswith("."):
            continue
        items.append(read_skill(child))
    return items


def remove_path(path: Path) -> bool:
    if not path.exists():
        return True
    if path.is_dir():
        def _onexc(func, target, _exc):
            try:
                os.chmod(target, stat.S_IWRITE)
                func(target)
            except OSError:
                pass

        try:
            shutil.rmtree(path, onexc=_onexc)
            return True
        except OSError:
            return False
    try:
        os.chmod(path, stat.S_IWRITE)
        path.unlink()
        return True
    except OSError:
        return False


def is_link(path: Path) -> bool:
    """符号链接 / Windows junction：不能当普通目录 rmtree，会删到链接目标。"""
    if path.is_symlink():
        return True
    checker = getattr(path, "is_junction", None)
    try:
        return bool(checker and checker())
    except OSError:
        return False


def move_to_trash(path: Path, category: str) -> Path:
    """删除前先移进 data/trash：平台副本和技能库源目录都不直接抹掉。

    同名目标已存在时追加序号，不会覆盖回收站里的旧副本。
    """
    if not path.exists():
        return path
    stamp = time.strftime("%Y%m%d-%H%M%S")
    root = paths.TRASH_ROOT / category
    try:
        root.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        raise HubError(f"创建回收站目录 {root} 失败：{exc}") from exc
    dest = root / f"{path.name}-{stamp}"
    index = 2
    while dest.exists():
        dest = root / f"{path.name}-{stamp}-{index}"
        index += 1
    try:
        shutil.move(str(path), str(dest))
    except OSError as exc:
        raise HubError(f"移动到回收站失败：{path} → {dest}：{exc}") from exc
    return dest


def copy_tree(src: Path, dest: Path) -> None:
    """先复制到临时目录再替换，避免中途失败留下半个技能。"""
    tmp = dest.with_name(f".{dest.name}.tmp-{os.getpid()}-{int(time.time() * 1000)}")
    remove_path(tmp)
    try:
        shutil.copytree(src, tmp, ignore=shutil.ignore_patterns(
            ".git", "__pycache__", "*.pyc", ".idea", ".vscode"))
        remove_path(dest)
        os.replace(tmp, dest)
    except OSError as exc:
        remove_path(tmp)
        raise HubError(f"复制到 {dest} 失败：{exc}（客户端可能正在占用）") from exc


def copy_into_library(src: Path, hub: dict, name: str | None = None,
                      overwrite: bool = False) -> Path:
    src = Path(src)
    if not src.is_dir() or skill_md(src) is None:
        raise HubError(f"{src} 不是有效技能目录（缺少 SKILL.md）")
    root = ensure_library(hub)
    folder = root / safe_name(name or src.name)
    try:
        src_path = src.resolve()
        dest_path = folder.resolve()
    except OSError:
        src_path, dest_path = src, folder
    if src_path == dest_path:
        return folder                 # 选中的就是技能库里这一份，不用再复制
    if dest_path in src_path.parents:
        raise HubError("源目录在技能库目标目录内部，无法复制")
    if folder.exists():
        if not overwrite:
            raise HubError(f"技能库里已有同名技能：{folder.name}")
        # 覆盖前把旧版本移进回收站，误覆盖还能找回来
        move_to_trash(folder, "skills")
    copy_tree(src, folder)
    return folder
