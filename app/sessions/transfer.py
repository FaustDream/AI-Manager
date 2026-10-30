# -*- coding: utf-8 -*-
"""把另一版本（国内 ↔ 国际）独有的会话搬进当前版本。

两个版本是同一产品的两套包装：sessions 表结构一致，正文同为
projects/<slug>/<sid>.jsonl，所以不需要格式转换，把行和文件搬过去即可。

只补本版没有的 id：已有的一律跳过，本版删过的（黑名单）也不复活——否则用户
删掉的会话会在下次恢复时自己长回来。行原样复制，只把 user_id 改成目标账号、
deleted_at 清空，其余字段（时间、标题、模型、快照引用）保持原值。
"""
from __future__ import annotations

import shutil
import sqlite3
from pathlib import Path

from ..core import winproc
from . import trash, wbdb
from .editions import (Edition, all_columns, connect_ro, connect_rw,
                       ensure_writable, other_edition, table_columns)


def _copy_into(dst: Edition, src: Edition, path: Path) -> bool:
    """按相对位置复制正文 / 任务 / 待办；目标已存在算成功，不覆盖。"""
    try:
        rel = path.relative_to(src.root)
    except ValueError:
        return False
    dest = dst.root / rel
    if dest.exists():
        return True
    try:
        dest.parent.mkdir(parents=True, exist_ok=True)
        if path.is_dir():
            shutil.copytree(path, dest)
        else:
            shutil.copy2(path, dest)
        ensure_writable(dest)
        return True
    except OSError:
        return False


def plan(dst: Edition) -> tuple[Edition, list[dict], int, dict[str, list[Path]]]:
    """另一版本里本版还没有、且有正文的会话；返回 (源版本, 行, 缺正文数, 正文索引)。"""
    src = other_edition(dst)
    if not src.exists:
        return src, [], 0, {}
    blocked = trash.tombstone_ids()
    have = {row["id"] for row in wbdb.list_local_sessions(dst)}
    files = wbdb.project_files(src)
    rows: list[dict] = []
    missing = 0
    for row in wbdb.list_local_sessions(src):
        sid = row["id"]
        if sid in blocked or sid in have:
            continue
        if not files.get(sid):
            missing += 1
            continue
        rows.append(row)
    return src, rows, missing, files


def count(dst: Edition) -> tuple[str, int]:
    """另一版本可引入的会话数，用于恢复前的确认文案。"""
    src, rows, _missing, _files = plan(dst)
    return src.short, len(rows)


def _extras(src: Edition, log) -> tuple[dict, dict]:
    """一次读完用量与快照：逐条查会为每条会话开一次连接。"""
    usage: dict = {}
    buddy: dict = {}
    conn = connect_ro(src.db)
    if conn is None:
        return usage, buddy
    try:
        conn.row_factory = sqlite3.Row
        usage = {row["session_id"]: dict(row)
                 for row in conn.execute("SELECT * FROM session_usage")}
        buddy = {row["snapshot_id"]: dict(row)
                 for row in conn.execute("SELECT * FROM buddy_snapshots")}
    except sqlite3.Error as exc:
        # 旧版可能没有 buddy_snapshots；用量读不到也不影响会话本身
        log(f"读取 {src.short} 的会话附加信息失败：{exc}", "warn")
    finally:
        conn.close()
    return usage, buddy


def import_from_other(dst: Edition, uid: str, log=None) -> dict:
    """把另一版本独有的会话搬进 dst 并归属到 uid。

    调用方必须先关闭两边的客户端：源版本在写会让正文复制到一半，目标版本在跑
    会用它内存里的会话表覆盖刚写进去的行。只新增，不修改目标已有的任何行。
    """
    log = log or (lambda message, level="info": None)
    src, rows, missing, files_map = plan(dst)
    stats = {"edition": src.short, "added": 0, "missing": missing, "failed": 0}
    if not rows:
        return stats
    if winproc.process_running(src.proc):
        raise RuntimeError(f"{src.label} 正在运行，请先关闭客户端再引入会话")

    usage, buddy = _extras(src, log)
    conn = connect_rw(dst.db)
    try:
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        cols = set(table_columns(conn, "sessions"))
        ucols = set(table_columns(conn, "session_usage"))
        try:
            bcols = set(all_columns(conn, "buddy_snapshots"))
        except sqlite3.Error:
            bcols = set()
        for row in rows:
            sid = row["id"]
            payload = list(files_map.get(sid) or [])
            for extra in (src.tasks / sid, src.todos / f"{sid}.json"):
                if extra.exists():
                    payload.append(extra)
            # 正文没搬全就不要插行：空壳会话比没有这条会话更让人困惑
            if not all(_copy_into(dst, src, path) for path in payload):
                stats["failed"] += 1
                continue
            # 归属留给恢复流程统一重指：这里只补行。同一账号在两版登录时
            # 原 user_id 就是目标账号，恢复时自然算「已存在」，不必多写一次。
            data = {**row, "user_id": row.get("user_id") or uid,
                    "deleted_at": None, "transport": "local"}
            names = [name for name in data if name in cols]
            try:
                cur = conn.execute(
                    f"INSERT OR IGNORE INTO sessions ({','.join(names)}) "
                    f"VALUES ({','.join('?' * len(names))})",
                    [data[name] for name in names])
            except sqlite3.Error as exc:
                log(f"引入会话 {sid[:8]} 失败：{exc}", "warn")
                stats["failed"] += 1
                continue
            if cur.rowcount <= 0:
                continue
            stats["added"] += 1
            use = usage.get(sid)
            if use:
                unames = [name for name in use if name in ucols]
                conn.execute(
                    f"INSERT OR REPLACE INTO session_usage ({','.join(unames)}) "
                    f"VALUES ({','.join('?' * len(unames))})",
                    [use[name] for name in unames])
            cwd = row.get("cwd")
            if cwd:
                conn.execute(
                    "INSERT OR IGNORE INTO workspaces (path, last_opened_at) VALUES (?, ?)",
                    (cwd, int(row.get("last_activity_at") or 0)))
            snap = buddy.get(row.get("buddy_snapshot_id") or "")
            if snap and bcols:
                bnames = [name for name in snap if name in bcols]
                conn.execute(
                    f"INSERT OR IGNORE INTO buddy_snapshots ({','.join(bnames)}) "
                    f"VALUES ({','.join('?' * len(bnames))})",
                    [snap[name] for name in bnames])
        conn.commit()
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    except Exception:
        conn.rollback()
        raise
    finally:
        conn.close()

    if stats["added"]:
        log(f"已从 {src.label} 引入 {stats['added']} 条会话")
    if stats["failed"]:
        log(f"有 {stats['failed']} 条会话正文复制失败，已跳过", "warn")
    return stats
