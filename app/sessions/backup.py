# -*- coding: utf-8 -*-
"""同步前的数据库 + 正文备份，以及按备份回滚。

备份目录沿用 ~/.workbuddy-session-sync/backups，旧版本的备份仍然可以直接回滚。
"""
from __future__ import annotations

import json
import shutil
import sqlite3
from datetime import datetime
from pathlib import Path

from ..core import paths, winproc
from .editions import Edition, connect_ro, path_size, remove_path

BACKUP_ROOT = paths.BACKUP_ROOT


def backup_db(ep: Edition, dest: Path) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    src_conn = connect_ro(ep.db)
    if src_conn is None:
        raise RuntimeError(f"无法读取 {ep.db}")
    try:
        dst_conn = sqlite3.connect(str(dest))
        try:
            src_conn.backup(dst_conn)
        finally:
            dst_conn.close()
    finally:
        src_conn.close()


def make_backup(plans, tag: str) -> Path:
    """plans: [(src, dst, items, skipped)]，只备份「会被覆盖」的文件。"""
    bdir = BACKUP_ROOT / tag
    (bdir / "db").mkdir(parents=True, exist_ok=True)
    # 两个版本的库都要备份：本版本内恢复也会改另一个库
    for key in ("cn", "intl"):
        ep = Edition(key)
        if ep.exists:
            backup_db(ep, bdir / "db" / f"{key}.db")

    touched = {}
    for src, dst, items, _ in plans:
        touched[dst.key] = (dst, items)
    for key, (ep, items) in touched.items():
        for item in items:
            dst_dir = ep.projects / item.slug
            for path in item.files:
                target = dst_dir / path.name
                if not target.exists():
                    continue
                rel = target.relative_to(ep.root)
                save = bdir / "files" / key / rel
                save.parent.mkdir(parents=True, exist_ok=True)
                if target.is_dir():
                    if save.exists():
                        remove_path(save)
                    shutil.copytree(target, save)
                else:
                    shutil.copy2(target, save)
            for target in (ep.tasks / item.sid, ep.todos / f"{item.sid}.json"):
                if not target.exists():
                    continue
                save = bdir / "files" / key / target.relative_to(ep.root)
                save.parent.mkdir(parents=True, exist_ok=True)
                if target.is_dir():
                    if save.exists():
                        remove_path(save)
                    shutil.copytree(target, save)
                else:
                    shutil.copy2(target, save)

    manifest = {
        "tag": tag,
        "created_at": datetime.now().isoformat(timespec="seconds"),
        "plans": [
            {
                "src": src.key, "dst": dst.key,
                "items": [
                    {"sid": item.sid, "action": item.action, "slug": item.slug,
                     "files": [f.name for f in item.files]}
                    for item in items
                ],
            }
            for src, dst, items, _ in plans
        ],
    }
    (bdir / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    return bdir


def list_backups() -> list[dict]:
    if not BACKUP_ROOT.exists():
        return []
    out = []
    for entry in sorted(BACKUP_ROOT.iterdir(), reverse=True):
        meta = entry / "manifest.json"
        if not meta.exists():
            continue
        try:
            data = json.loads(meta.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        out.append({
            "tag": entry.name,
            "count": sum(len(p.get("items", [])) for p in data.get("plans", [])),
            "created": data.get("created_at", ""),
            "path": str(entry),
            "size": path_size(entry),
        })
    return out


def running_clients() -> list[Edition]:
    return [ep for ep in (Edition("cn"), Edition("intl"))
            if winproc.process_running(ep.proc)]


def rollback(tag: str) -> None:
    bdir = BACKUP_ROOT / tag
    manifest_path = bdir / "manifest.json"
    if not manifest_path.exists():
        raise RuntimeError(f"找不到备份：{bdir}")
    running = running_clients()
    if running:
        raise RuntimeError("请先关闭以下客户端再回滚：" + "、".join(ep.label for ep in running))

    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    for key, ep in (("cn", Edition("cn")), ("intl", Edition("intl"))):
        db_backup = bdir / "db" / f"{key}.db"
        if db_backup.exists():
            if ep.db.exists():
                for suffix in ("", "-wal", "-shm"):
                    remove_path(Path(str(ep.db) + suffix))
            shutil.copy2(db_backup, ep.db)
        files_root = bdir / "files" / key
        if files_root.exists():
            for save in files_root.rglob("*"):
                if save.is_dir():
                    continue
                rel = save.relative_to(files_root)
                target = ep.root / rel
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(save, target)

    # 本次新复制进去的文件在备份里没有原件，回滚时要删掉，否则留下孤儿正文
    for plan in manifest.get("plans", []):
        ep = Edition(plan["dst"])
        for item in plan.get("items", []):
            targets = [ep.projects / item["slug"] / name for name in item.get("files", [])]
            targets += [ep.tasks / item["sid"], ep.todos / f'{item["sid"]}.json']
            for target in targets:
                if not target.exists():
                    continue
                save = bdir / "files" / ep.key / target.relative_to(ep.root)
                if not save.exists():
                    remove_path(target)
