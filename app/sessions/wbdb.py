# -*- coding: utf-8 -*-
"""WorkBuddy 会话的读取与删除。

会话的落盘构成：sessions 行 + session_usage 行 + projects/<slug>/<id>.jsonl（正文）
+ 同名前缀附属文件 + tasks/<id>/ + todos/<id>.json。跨版本的搬运已移除，
这里只保留「读清单 / 定位正文 / 删除」三件事。
"""
from __future__ import annotations

import re
import shutil
import sqlite3
from pathlib import Path

from . import trash
from .editions import (Edition, connect_ro, connect_rw, path_size,
                       remove_path, table_columns)

# 会话 ID 是 uuid / 32 位十六进制；projects 下其它名字的文件不是会话正文
SESSION_ID_RE = re.compile(r"^[0-9a-fA-F][0-9a-fA-F-]{15,}$")


# --------------------------------------------------------------------- 读取
def list_local_sessions(ep: Edition) -> list[dict]:
    conn = connect_ro(ep.db)
    if conn is None:
        return []
    try:
        conn.row_factory = sqlite3.Row
        return [dict(row) for row in conn.execute(
            "SELECT * FROM sessions WHERE transport='local' "
            "ORDER BY COALESCE(last_activity_at, created_at) DESC"
        )]
    finally:
        conn.close()


def list_all_sessions(ep: Edition) -> list[dict]:
    """全量行（含 transport='cloud' 的云端缓存行）。

    云端行没有本地正文，但它是「会话实际存在」的一部分——
    只读 local 行会漏掉它们，正是“列表不显示”的来源之一。
    """
    conn = connect_ro(ep.db)
    if conn is None:
        return []
    try:
        conn.row_factory = sqlite3.Row
        return [dict(row) for row in conn.execute(
            "SELECT * FROM sessions "
            "ORDER BY COALESCE(last_activity_at, created_at) DESC"
        )]
    finally:
        conn.close()


def all_session_ids(ep: Edition) -> set[str]:
    """库里出现过的全部会话 ID（含云端行），用来判断正文文件是否还有主。"""
    conn = connect_ro(ep.db)
    if conn is None:
        return set()
    try:
        return {str(row[0]) for row in conn.execute("SELECT id FROM sessions")}
    finally:
        conn.close()


def find_session_files(ep: Edition, sid: str) -> list[Path]:
    """定位正文与附属文件：<sid>.jsonl / <sid>.file-rollback.ndjson / <sid>/tool-results。"""
    hits = []
    if not ep.projects.exists():
        return hits
    for path in ep.projects.glob(f"*/{sid}*"):
        if path.name == sid or path.name.startswith(sid + "."):
            hits.append(path)
    return sorted(hits)


def project_files(ep: Edition) -> dict[str, list[Path]]:
    """一次遍历 projects/，按会话 ID 归组正文与附属文件。

    逐条 glob 会把每个项目目录列 N 遍（几百条会话时明显发涩），这里只列一遍。
    """
    index: dict[str, list[Path]] = {}
    if not ep.projects.exists():
        return index
    try:
        projects = list(ep.projects.iterdir())
    except OSError:
        return index
    for project in projects:
        if not project.is_dir():
            continue
        try:
            entries = list(project.iterdir())
        except OSError:
            continue
        for entry in entries:
            name = entry.name if entry.is_dir() else entry.name.split(".", 1)[0]
            if not SESSION_ID_RE.match(name):
                continue
            index.setdefault(name, []).append(entry)
    return index


def slug_of(files, cwd: str) -> str:
    if files:
        return files[0].parent.name
    from .editions import cwd_to_slug
    return cwd_to_slug(cwd)


def visible_rows(ep: Edition) -> list[dict]:
    """完全照客户端 sidebar 的查询条件取一次，用来确认「点了之后能不能看到」。"""
    uid = ep.current_uid()[0]
    conn = connect_ro(ep.db)
    if conn is None:
        return []
    try:
        conn.row_factory = sqlite3.Row
        return [dict(row) for row in conn.execute(
            "SELECT id, title, custom_title, last_activity_at FROM sessions "
            "WHERE transport = 'local' AND deleted_at IS NULL "
            "AND (user_id = ? OR user_id = '') "
            "ORDER BY COALESCE(last_activity_at, created_at) DESC, id ASC",
            (uid,),
        )]
    finally:
        conn.close()


def visible_count(ep: Edition) -> int:
    return len(visible_rows(ep))


def cloud_count(ep: Edition) -> int:
    """云端会话本地删不掉，重新登录会再拉回来，删除前要提示。"""
    conn = connect_ro(ep.db)
    if conn is None:
        return 0
    try:
        return conn.execute(
            "SELECT COUNT(*) FROM sessions WHERE transport = 'cloud'").fetchone()[0]
    finally:
        conn.close()


# --------------------------------------------------------------------- 删除
def delete_sessions(ids, keep_trash: bool = True) -> dict:
    """两个版本同时删除：数据库行 + 正文 + 任务/待办，并加入黑名单。

    黑名单不再用于同步，但「恢复会话」会看它——删掉的会话不该被再次关联回来。
    """
    ids = list(dict.fromkeys(ids))
    if not ids:
        return {"deleted": 0, "files": 0, "bytes": 0, "trash": "", "ids": []}

    tag = trash.new_tag(trash.TRASH_ROOT)
    target_dir = trash.TRASH_ROOT / tag
    items = []
    stats = {"deleted": 0, "files": 0, "bytes": 0, "trash": str(target_dir), "ids": []}

    for ep in (Edition("cn"), Edition("intl")):
        if not ep.exists:
            continue
        conn = connect_rw(ep.db)
        try:
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
            scols = table_columns(conn, "sessions")
            ucols = table_columns(conn, "session_usage")
            for sid in ids:
                row = conn.execute(
                    "SELECT * FROM sessions WHERE id = ? AND transport = 'local'", (sid,)
                ).fetchone()
                usage = conn.execute(
                    "SELECT * FROM session_usage WHERE session_id = ?", (sid,)
                ).fetchone()
                files = find_session_files(ep, sid)
                for extra in (ep.tasks / sid, ep.todos / f"{sid}.json"):
                    if extra.exists():
                        files.append(extra)
                if row is None and usage is None and not files:
                    continue

                record = {"sid": sid, "edition": ep.key, "files": [], "size": 0,
                          "row": dict(zip(scols, row)) if row else None,
                          "usage": dict(zip(ucols, usage)) if usage else None}
                if row is not None:
                    conn.execute("DELETE FROM sessions WHERE id = ? AND transport = 'local'",
                                 (sid,))
                    stats["deleted"] += 1
                    stats["ids"].append(sid)
                if usage is not None:
                    conn.execute("DELETE FROM session_usage WHERE session_id = ?", (sid,))

                for path in files:
                    size = path_size(path)
                    rel = path.relative_to(ep.root)
                    try:
                        if keep_trash:
                            dest = target_dir / ep.key / rel
                            dest.parent.mkdir(parents=True, exist_ok=True)
                            if dest.exists():
                                remove_path(dest)
                            shutil.move(str(path), str(dest))
                        else:
                            remove_path(path)
                        record["files"].append(str(rel))
                        record["size"] += size
                        stats["files"] += 1
                        stats["bytes"] += size
                    except OSError:
                        pass
                items.append(record)
            conn.commit()
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        finally:
            conn.close()

    if items and keep_trash:
        trash.write_manifest(tag, items)
    trash.add_tombstones(ids)
    stats["ids"] = list(dict.fromkeys(stats["ids"]))
    if not items:
        stats["trash"] = ""
    return stats
