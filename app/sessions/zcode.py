# -*- coding: utf-8 -*-
"""ZCode 本地会话：读取、删除、回收站恢复。

ZCode 把会话写在自己的 SQLite 里（~/.zcode/cli/db/db.sqlite，几百 MB）：
session 一行一条会话，message 一条消息一行（data 里存 role 等元信息），
正文片段在 part（type=text 的 data.text）。账号在库里没有字段，同一客户端换号
共用这份库，所以这里只做管理，不做账号间复制。

库很大，列表只查 session 与 message 的计数；单条会话的大小不逐条统计（要把
几百 MB 的 blob 全读一遍），界面上显示为「—」。
"""
from __future__ import annotations

import json
import sqlite3
from pathlib import Path

from ..core import paths
from . import lifecycle, trash
from .editions import connect_ro, connect_rw

DB = Path.home() / ".zcode" / "cli" / "db" / "db.sqlite"
LABEL = "ZCode"
PROC = "ZCode.exe"
# 删除 / 导出时跟着会话走的表（都有 session_id）
TABLES = ("message", "part", "session_entry", "session_input")


def client() -> lifecycle.Client:
    exe = paths.user_local_dir() / "Programs" / "ZCode" / "ZCode.exe"
    return lifecycle.Client(PROC, LABEL, exe if exe.exists() else None)


def root_fp() -> str:
    """源指纹：库文件的 mtime:size，变了才重扫。"""
    try:
        stat = DB.stat()
        return f"{stat.st_mtime_ns}:{stat.st_size}"
    except OSError:
        return "absent"


def _state(deleted, archived) -> str:
    if deleted is not None:
        return "已删除"
    return "已归档" if archived else "可见"


def scan() -> list[dict]:
    if not DB.exists():
        return []
    conn = connect_ro(DB)
    if conn is None:
        return []
    rows: list[dict] = []
    try:
        counts = dict(conn.execute(
            "SELECT session_id, COUNT(*) FROM message GROUP BY session_id"))
        for sid, title, directory, updated, deleted, archived in conn.execute(
                "SELECT id, title, directory, time_updated, time_deleted, time_archived "
                "FROM session WHERE parent_id IS NULL ORDER BY time_updated DESC"):
            rows.append({
                "kind": "zcode",
                "source_key": "zcode",
                "source_label": LABEL,
                "account": "",
                "id": sid,
                "title": title or "（未命名会话）",
                "last": int(updated or 0),
                "msgs": int(counts.get(sid, 0)),
                "size": 0,
                "state": _state(deleted, archived),
                "has_body": bool(counts.get(sid)),
                "files": [],
                "path": str(DB),
                "cwd": directory or "",
                "client_status": "",
                "raw": None,
            })
    except sqlite3.Error:
        return rows
    finally:
        conn.close()
    return rows


def _dump(conn, sid: str) -> dict | None:
    session = conn.execute("SELECT * FROM session WHERE id = ?", (sid,)).fetchone()
    if session is None:
        return None
    side = {}
    for table in TABLES:
        try:
            side[table] = [dict(r) for r in conn.execute(
                f"SELECT * FROM {table} WHERE session_id = ?", (sid,))]
        except sqlite3.Error:
            continue
    return {"session": dict(session), "side": side}


def _insert(conn, table: str, rows) -> int:
    if not rows:
        return 0
    try:
        cols = [r[1] for r in conn.execute(f"PRAGMA table_info({table})")]
    except sqlite3.Error:
        return 0
    added = 0
    for row in rows:
        names = [c for c in row if c in cols]
        if not names:
            continue
        conn.execute(
            f'INSERT OR REPLACE INTO {table} ({",".join(names)}) '
            f'VALUES ({",".join("?" * len(names))})',
            [row[name] for name in names])
        added += 1
    return added


def delete(rows: list[dict], log) -> dict:
    stats = {"deleted": 0, "bytes": 0, "trash": ""}
    if not rows or not DB.exists():
        return stats
    with lifecycle.client_guard([client()], log):
        tag = trash.new_tag(trash.TRASH_ROOT)
        items = []
        conn = connect_rw(DB)
        try:
            conn.row_factory = sqlite3.Row
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
            for row in rows:
                data = _dump(conn, row["id"])
                if data is None:
                    continue
                rel = Path("zcode") / f'{row["id"]}.json'
                dest = trash.TRASH_ROOT / tag / rel
                dest.parent.mkdir(parents=True, exist_ok=True)
                dest.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
                size = dest.stat().st_size
                for table in TABLES:
                    conn.execute(f"DELETE FROM {table} WHERE session_id = ?", (row["id"],))
                conn.execute("DELETE FROM session WHERE id = ?", (row["id"],))
                items.append({"kind": "zcode", "sid": row["id"], "source": "zcode",
                              "trash_rel": str(rel), "size": size,
                              "files": [], "row": None, "usage": None})
                stats["deleted"] += 1
                stats["bytes"] += size
            conn.commit()
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        finally:
            conn.close()
    if items:
        trash.write_manifest(tag, items)
        stats["trash"] = str(trash.TRASH_ROOT / tag)
    if stats["deleted"]:
        log(f"🗑 ZCode 会话 {stats['deleted']} 条，已移入回收站")
    return stats


def restore_item(item: dict, tag_dir: Path) -> bool:
    if not DB.exists():
        return False
    src = tag_dir / str(item.get("trash_rel") or "")
    try:
        data = json.loads(src.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return False
    with lifecycle.client_guard([client()]):
        conn = connect_rw(DB)
        try:
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
            _insert(conn, "session", [data.get("session") or {}])
            for table, rows in (data.get("side") or {}).items():
                _insert(conn, table, rows)
            conn.commit()
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        finally:
            conn.close()
    return True
