# -*- coding: utf-8 -*-
"""WorkBuddy 会话的读、搬运、恢复可见、删除。

同步一条会话需要：session 行 + session_usage 行 + workspaces 登记 +
projects/<slug>/<id>.jsonl（正文）+ 同名前缀附属文件 + tasks/<id>/ + todos/<id>.json。
"""
from __future__ import annotations

import shutil
import sqlite3
import time
from pathlib import Path

from . import trash
from .editions import (Edition, connect_ro, connect_rw, path_size, all_columns,
                       copy_any, remove_path, table_columns)


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


def find_session_files(ep: Edition, sid: str) -> list[Path]:
    """定位正文与附属文件：<sid>.jsonl / <sid>.file-rollback.ndjson / <sid>/tool-results。"""
    hits = []
    if not ep.projects.exists():
        return hits
    for path in ep.projects.glob(f"*/{sid}*"):
        if path.name == sid or path.name.startswith(sid + "."):
            hits.append(path)
    return sorted(hits)


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


# --------------------------------------------------------------------- 计划
class Item:
    __slots__ = ("direction", "sid", "title", "action", "files", "slug", "size",
                 "row", "deleted")

    def __init__(self, direction, sid, title, action, files, slug, size, row, deleted):
        self.direction = direction
        self.sid = sid
        self.title = title
        self.action = action      # insert / update / resurrect
        self.files = files
        self.slug = slug
        self.size = size
        self.row = row
        self.deleted = deleted


def build_plan(src: Edition, dst: Edition, include_deleted: bool,
               update_newer: bool) -> tuple[list[Item], list[tuple]]:
    items: list[Item] = []
    skipped: list[tuple] = []
    blocked = trash.tombstone_ids()
    dst_rows = {row["id"]: row for row in list_local_sessions(dst)}
    for row in list_local_sessions(src):
        sid = row["id"]
        if sid in blocked:
            skipped.append((row, "已在本机删除过，黑名单拦截"))
            continue
        deleted = row.get("deleted_at") is not None
        if deleted and not include_deleted:
            skipped.append((row, "源里已被标记删除（勾选「含已删除」可带上）"))
            continue
        files = find_session_files(src, sid)
        if not files:
            skipped.append((row, "找不到正文文件，跳过（否则会变成空对话）"))
            continue
        dst_row = dst_rows.get(sid)
        if dst_row:
            if int(row.get("updated_at") or 0) > int(dst_row.get("updated_at") or 0):
                if not update_newer:
                    skipped.append((row, "目标已有且未开启更新"))
                    continue
                action = "update"
            elif dst_row.get("deleted_at") is not None:
                action = "resurrect"
            else:
                skipped.append((row, "目标已有且内容一致"))
                continue
        else:
            action = "insert"
        size = sum(path_size(f) for f in files)
        items.append(Item(
            f"{src.short} → {dst.short}", sid,
            row.get("custom_title") or row.get("title"),
            action, files, slug_of(files, row.get("cwd") or ""), size, row, deleted,
        ))
    return items, skipped


def build_local_restore(ep: Edition, uid: str) -> list[dict]:
    """本版本内“属于自己但不可见”的会话：被 soft-delete 的行。

    归属判定规则（与客户端列表条件一致）：
        user_id = 当前账号 或 user_id = ''（无主）→ 属于当前列表；
        user_id = 其他账号 → 属于旧账号，保留在本地但绝不并入当前账号。

    旧行为会把旧账号的会话改挂到当前账号名下，导致账号切换后
    列表“串号”；现在只恢复软删行，不动旧账号的归属。
    """
    if not uid:
        return []
    blocked = trash.tombstone_ids()
    rows = []
    for row in list_local_sessions(ep):
        if row["id"] in blocked:
            continue
        owner = row.get("user_id") or ""
        if owner not in ("", uid):
            continue          # 旧账号的会话：保留原样，不进当前账号列表
        deleted = row.get("deleted_at")
        if deleted is not None and deleted != -1:
            rows.append(row)
    return rows


# --------------------------------------------------------------------- 执行
def apply_local_restore(ep: Edition, rows, uid: str) -> int:
    conn = connect_rw(ep.db)
    changed = 0
    try:
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        for row in rows:
            cur = conn.execute(
                "UPDATE sessions SET user_id = ?, deleted_at = NULL "
                "WHERE id = ? AND transport = 'local'",
                (uid, row["id"]),
            )
            changed += cur.rowcount
            cwd = row.get("cwd")
            if cwd:
                conn.execute(
                    "INSERT OR IGNORE INTO workspaces (path, last_opened_at) VALUES (?, ?)",
                    (cwd, int(row.get("last_activity_at") or time.time() * 1000)),
                )
        conn.commit()
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    finally:
        conn.close()
    return changed


def apply_item(src: Edition, dst: Edition, item: Item, target_uid: str) -> None:
    src_conn = connect_ro(src.db)
    dst_conn = connect_rw(dst.db)
    try:
        dst_conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        dst_cols = table_columns(dst_conn, "sessions")
        cols = [c for c in item.row.keys() if c in dst_cols]
        values = []
        for col in cols:
            value = item.row[col]
            if col == "user_id":
                value = target_uid
            elif col == "deleted_at":
                value = None          # 关键：本地会话只有 deleted_at IS NULL 才会显示
            elif col == "unread":
                value = 0
            elif col == "transport":
                value = "local"
            values.append(value)
        if item.action == "resurrect":
            dst_conn.execute(
                "UPDATE sessions SET deleted_at = NULL, updated_at = ? "
                "WHERE id = ? AND transport = 'local'",
                (int(item.row.get("updated_at") or 0), item.sid),
            )
        else:
            dst_conn.execute(
                f"INSERT OR REPLACE INTO sessions ({','.join(cols)}) "
                f"VALUES ({','.join('?' * len(cols))})",
                values,
            )

        # 用量行
        if src_conn is not None:
            usage = src_conn.execute(
                "SELECT * FROM session_usage WHERE session_id = ?", (item.sid,)
            ).fetchone()
            if usage:
                ucols = table_columns(src_conn, "session_usage")
                urow = dict(zip(ucols, usage))
                dcols = [c for c in urow if c in table_columns(dst_conn, "session_usage")]
                dst_conn.execute(
                    f"INSERT OR REPLACE INTO session_usage ({','.join(dcols)}) "
                    f"VALUES ({','.join('?' * len(dcols))})",
                    [urow[c] for c in dcols],
                )

        # 工作区登记：没有这行客户端可能找不到目录
        cwd = item.row.get("cwd")
        if cwd:
            dst_conn.execute(
                "INSERT OR IGNORE INTO workspaces (path, last_opened_at) VALUES (?, ?)",
                (cwd, int(item.row.get("last_activity_at") or time.time() * 1000)),
            )

        # buddy 快照（会话引用了就需要，否则头像/角色信息缺失）
        bsid = item.row.get("buddy_snapshot_id")
        if bsid and src_conn is not None:
            exists = dst_conn.execute(
                "SELECT 1 FROM buddy_snapshots WHERE snapshot_id = ?", (bsid,)
            ).fetchone()
            if exists is None:
                snapshot = src_conn.execute(
                    "SELECT * FROM buddy_snapshots WHERE snapshot_id = ?", (bsid,)
                ).fetchone()
                if snapshot:
                    bcols = all_columns(src_conn, "buddy_snapshots")
                    dcols = [c for c in bcols if c in all_columns(dst_conn, "buddy_snapshots")]
                    record = dict(zip(bcols, snapshot))
                    dst_conn.execute(
                        f"INSERT OR REPLACE INTO buddy_snapshots ({','.join(dcols)}) "
                        f"VALUES ({','.join('?' * len(dcols))})",
                        [record[c] for c in dcols],
                    )

        dst_conn.commit()
        dst_conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    finally:
        if src_conn is not None:
            src_conn.close()
        dst_conn.close()

    # 正文与附属文件
    dst_dir = dst.projects / item.slug
    dst_dir.mkdir(parents=True, exist_ok=True)
    for path in item.files:
        copy_any(path, dst_dir / path.name)

    # 任务 / 待办
    src_tasks = src.tasks / item.sid
    if src_tasks.exists():
        copy_any(src_tasks, dst.tasks / item.sid)
    todo = src.todos / f"{item.sid}.json"
    if todo.exists():
        target = dst.todos / f"{item.sid}.json"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(todo.read_bytes())


def copy_single_session(src_key: str, dst_key: str, sid: str) -> tuple[bool, str]:
    """把一条 WorkBuddy 会话从某个版本搬到另一个版本（同格式）。"""
    src, dst = Edition(src_key), Edition(dst_key)
    if sid in trash.tombstone_ids():
        return False, "该会话已删除并加入黑名单，不再搬运"
    row = next((r for r in list_local_sessions(src) if r["id"] == sid), None)
    if row is None:
        return False, "源版本里没有这条会话"
    files = find_session_files(src, sid)
    if not files:
        return False, "源会话没有正文文件"
    item = Item(f"{src.short} → {dst.short}", sid,
                row.get("custom_title") or row.get("title"), "insert", files,
                slug_of(files, row.get("cwd") or ""),
                sum(path_size(f) for f in files), row,
                row.get("deleted_at") is not None)
    apply_item(src, dst, item, dst.current_uid()[0])
    return True, f"已写入 {dst.label}"


# --------------------------------------------------------------------- 删除
def delete_sessions(ids, keep_trash: bool = True) -> dict:
    """两个版本同时删除：数据库行 + 正文 + 任务/待办，并加入黑名单。"""
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
