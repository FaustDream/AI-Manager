# -*- coding: utf-8 -*-
"""删除黑名单与回收站。

删除会话时文件先搬到回收站（可恢复），同时把会话 ID 加入黑名单：
同步已经取消，但「恢复会话」会看这份名单，删掉的会话不该被再次关联回来。
"""
from __future__ import annotations

import json
import shutil
from datetime import datetime
from pathlib import Path

from ..core import paths
from .editions import (Edition, connect_rw, remove_path, table_columns)

TOMB_PATH = paths.TOMB_FILE
TRASH_ROOT = paths.TRASH_ROOT


def new_tag(root: Path) -> str:
    """同一秒内多次操作也不能撞目录：撞了就加序号。"""
    base = datetime.now().strftime("%Y%m%d-%H%M%S")
    tag, index = base, 2
    while (root / tag).exists():
        tag = f"{base}-{index}"
        index += 1
    return tag


# ------------------------------------------------------------------- 黑名单
def load_tombstones() -> dict:
    try:
        data = json.loads(TOMB_PATH.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    ids = data.get("ids")
    return ids if isinstance(ids, dict) else {}


def save_tombstones(ids: dict) -> None:
    try:
        TOMB_PATH.parent.mkdir(parents=True, exist_ok=True)
        TOMB_PATH.write_text(json.dumps({"ids": ids}, ensure_ascii=False, indent=2),
                             encoding="utf-8")
    except OSError:
        pass


def tombstone_ids() -> set:
    return set(load_tombstones())


def add_tombstones(ids) -> None:
    data = load_tombstones()
    now = datetime.now().isoformat(timespec="seconds")
    for sid in ids:
        data[sid] = now
    save_tombstones(data)


def remove_tombstones(ids) -> None:
    data = load_tombstones()
    changed = False
    for sid in ids:
        if data.pop(sid, None) is not None:
            changed = True
    if changed:
        save_tombstones(data)


def clear_tombstones() -> int:
    count = len(load_tombstones())
    save_tombstones({})
    return count


# ------------------------------------------------------------------- 回收站
def list_trash() -> list[dict]:
    if not TRASH_ROOT.exists():
        return []
    out = []
    for entry in sorted(TRASH_ROOT.iterdir(), reverse=True):
        meta = entry / "manifest.json"
        if not meta.exists():
            continue
        try:
            data = json.loads(meta.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        out.append({
            "tag": entry.name,
            "created": data.get("created_at", ""),
            "count": len({item["sid"] for item in data.get("items", [])}),
            "path": str(entry),
            "size": sum(int(item.get("size") or 0) for item in data.get("items", [])),
        })
    return out


def write_manifest(tag: str, items: list) -> Path:
    target = TRASH_ROOT / tag
    target.mkdir(parents=True, exist_ok=True)
    manifest = {
        "tag": tag,
        "created_at": datetime.now().isoformat(timespec="seconds"),
        "items": items,
    }
    (target / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")
    return target


def restore_trash(tag) -> int:
    """把某次删除的内容放回原处，并从黑名单移除。

    WorkBuddy 的条目按 edition 还原数据库行；CodeBuddy 的条目交给
    codebuddy.restore_item 搬回目录并重新登记索引。
    """
    target_dir = TRASH_ROOT / tag
    meta = target_dir / "manifest.json"
    if not meta.exists():
        raise RuntimeError(f"找不到回收站记录：{target_dir}")
    data = json.loads(meta.read_text(encoding="utf-8"))
    restored = 0
    ids = []
    for item in data.get("items", []):
        if item.get("kind") == "codebuddy":
            from . import codebuddy
            if codebuddy.restore_item(item, target_dir):
                restored += 1
            continue
        if item.get("kind") in ("qoder", "zcode"):
            from . import others
            if others.restore_item(item, target_dir):
                restored += 1
            continue
        edition = Edition(item["edition"])
        for rel in item.get("files", []):
            src = target_dir / item["edition"] / rel
            if not src.exists():
                continue
            dst = edition.root / rel
            dst.parent.mkdir(parents=True, exist_ok=True)
            if dst.exists():
                remove_path(dst)
            shutil.move(str(src), str(dst))
        if not item.get("row") and not item.get("usage"):
            ids.append(item["sid"])
            continue
        conn = connect_rw(edition.db)
        try:
            if item.get("row"):
                cols = [c for c in item["row"] if c in table_columns(conn, "sessions")]
                conn.execute(
                    f"INSERT OR REPLACE INTO sessions ({','.join(cols)}) "
                    f"VALUES ({','.join('?' * len(cols))})",
                    [item["row"][c] for c in cols],
                )
                restored += 1
            if item.get("usage"):
                cols = [c for c in item["usage"] if c in table_columns(conn, "session_usage")]
                conn.execute(
                    f"INSERT OR REPLACE INTO session_usage ({','.join(cols)}) "
                    f"VALUES ({','.join('?' * len(cols))})",
                    [item["usage"][c] for c in cols],
                )
            conn.commit()
        finally:
            conn.close()
        ids.append(item["sid"])
    remove_tombstones(ids)
    # 标记为已恢复，避免同一条记录被重复恢复
    try:
        meta.rename(target_dir / "manifest.restored.json")
    except OSError:
        pass
    return restored


def purge_trash(tag=None) -> int:
    """彻底删除回收站内容。tag 为空时清空全部。"""
    removed = 0
    if tag:
        target = TRASH_ROOT / tag
        if target.exists() and remove_path(target):
            removed = 1
    elif TRASH_ROOT.exists():
        for entry in list(TRASH_ROOT.iterdir()):
            if entry.is_dir() and remove_path(entry):
                removed += 1
    return removed
