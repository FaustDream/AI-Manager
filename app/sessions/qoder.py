# -*- coding: utf-8 -*-
"""Qoder 国内版 / 国际版的本地会话：读取、删除、跨版本复制。

正文不在文件里，而是落在 Electron 用户目录的 main.sqlite：chat_sessions 一行一条
会话，chat_session_messages 一条消息一行（payload_json），搜索片段在
chat_session_search_segments（触发器维护 FTS）。两个版本各有一份结构相同的库，
所以「同软件同步」就是把会话行复制到另一版的库。

账号在库里没有对应字段——同一客户端换号共用这份库，没有可搬的归属，因此这里
不做账号维度的恢复，只做版本之间的复制。
"""
from __future__ import annotations

import json
import sqlite3
from datetime import datetime
from pathlib import Path

from ..core import paths
from . import lifecycle, trash
from .backup import backup_file, prune
from .editions import connect_ro, connect_rw

EDITIONS = {
    "qoder_cn": {"dir": "com.qodercn.app.stable", "label": "Qoder CN",
                 "proc": "Qoder CN.exe"},
    "qoder_intl": {"dir": "com.qoder.app.stable", "label": "Qoder 国际版",
                   "proc": "Qoder.exe"},
}
# 复制时跟着会话一起搬：消息正文、搜索片段（触发器会同步 FTS）、上下文用量、摘要高亮
COPY_TABLES = ("chat_session_messages", "chat_session_search_segments",
               "chat_session_context_usage", "chat_session_highlights")
FTS_PREFIX = "chat_session_search_fts"


def db_path(key: str) -> Path:
    return paths.user_appdata_dir() / EDITIONS[key]["dir"] / "main.sqlite"


def label(key: str) -> str:
    return EDITIONS[key]["label"]


def client(key: str) -> lifecycle.Client:
    """写库要关的客户端；exe 找不到时给 None，重启会提示手动打开。"""
    cfg = EDITIONS[key]
    exe = paths.user_local_dir() / "Programs" / cfg["proc"][:-4] / cfg["proc"]
    return lifecycle.Client(cfg["proc"], cfg["label"], exe if exe.exists() else None)


def root_fp(key: str) -> str:
    """源指纹：库文件的 mtime:size，变了才重扫。"""
    try:
        stat = db_path(key).stat()
        return f"{stat.st_mtime_ns}:{stat.st_size}"
    except OSError:
        return "absent"


# ------------------------------------------------------------------ 读取
def _session_tables(conn) -> list[str]:
    """所有带 session_id 的表：删除、导出都按它走，客户端加表也不会漏。"""
    out = []
    for (name,) in conn.execute("SELECT name FROM sqlite_master WHERE type='table'"):
        if name.startswith(FTS_PREFIX):
            continue
        try:
            cols = [r[1] for r in conn.execute(f'PRAGMA table_info("{name}")')]
        except sqlite3.Error:
            continue
        if "session_id" in cols:
            out.append(name)
    return out


def _state(deleted, archived) -> str:
    if deleted is not None:
        return "已删除"
    return "已归档" if archived else "可见"


def scan() -> list[dict]:
    rows: list[dict] = []
    for key, cfg in EDITIONS.items():
        db = db_path(key)
        if not db.exists():
            continue
        conn = connect_ro(db)
        if conn is None:
            continue
        try:
            stats = {sid: (n, size) for sid, n, size in conn.execute(
                "SELECT session_id, COUNT(*), COALESCE(SUM(LENGTH(payload_json)), 0) "
                "FROM chat_session_messages GROUP BY session_id")}
            for sid, title, cwd, updated, archived, deleted in conn.execute(
                    "SELECT session_id, title, cwd, updated_at, archived, deleted_at "
                    "FROM chat_sessions WHERE COALESCE(session_kind, 'standard') = 'standard' "
                    "ORDER BY updated_at DESC"):
                msgs, size = stats.get(sid, (0, 0))
                rows.append({
                    "kind": "qoder",
                    "source_key": key,
                    "source_label": cfg["label"],
                    "account": "",
                    "id": sid,
                    "title": title or "（未命名会话）",
                    "last": int(updated or 0),
                    "msgs": msgs,
                    "size": size,
                    "state": _state(deleted, archived),
                    "has_body": msgs > 0,
                    "files": [],
                    "path": str(db),
                    "cwd": cwd or "",
                    "client_status": "",
                    "raw": None,
                })
        except sqlite3.Error:
            continue
        finally:
            conn.close()
    return rows


# ------------------------------------------------------------------ 写入前置
def _dump(conn, sid: str, tables: list[str]) -> dict | None:
    session = conn.execute(
        "SELECT * FROM chat_sessions WHERE session_id = ?", (sid,)).fetchone()
    if session is None:
        return None
    side = {}
    for table in tables:
        try:
            side[table] = [dict(r) for r in conn.execute(
                f'SELECT * FROM "{table}" WHERE session_id = ?', (sid,))]
        except sqlite3.Error:
            continue
    return {"session": dict(session), "side": side}


def _insert(conn, table: str, rows) -> int:
    """按目标库的实际列插入；搜索片段的 row_id 由 SQLite 重新分配，避免撞行。"""
    if not rows:
        return 0
    try:
        cols = [r[1] for r in conn.execute(f'PRAGMA table_info("{table}")')]
    except sqlite3.Error:
        return 0
    added = 0
    for row in rows:
        names = [c for c in row if c in cols
                 and not (table == "chat_session_search_segments" and c == "row_id")]
        if not names:
            continue
        conn.execute(
            f'INSERT OR REPLACE INTO "{table}" ({",".join(names)}) '
            f'VALUES ({",".join("?" * len(names))})',
            [row[name] for name in names])
        added += 1
    return added


def _purge(conn, sid: str, tables: list[str]) -> None:
    for table in tables:
        try:
            conn.execute(f'DELETE FROM "{table}" WHERE session_id = ?', (sid,))
        except sqlite3.Error:
            continue
    conn.execute("DELETE FROM chat_sessions WHERE session_id = ?", (sid,))


# ------------------------------------------------------------------ 删除 / 恢复
def delete(rows: list[dict], log) -> dict:
    stats = {"deleted": 0, "bytes": 0, "trash": ""}
    groups: dict[str, list[dict]] = {}
    for row in rows:
        groups.setdefault(row["source_key"], []).append(row)
    tag = trash.new_tag(trash.TRASH_ROOT)
    items = []
    for key, group in groups.items():
        if not db_path(key).exists():
            continue
        with lifecycle.client_guard([client(key)], log):
            conn = connect_rw(db_path(key))
            try:
                conn.row_factory = sqlite3.Row
                conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
                tables = _session_tables(conn)
                for row in group:
                    data = _dump(conn, row["id"], tables)
                    if data is None:
                        continue
                    rel = Path("qoder") / key / f'{row["id"]}.json'
                    dest = trash.TRASH_ROOT / tag / rel
                    dest.parent.mkdir(parents=True, exist_ok=True)
                    dest.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
                    size = dest.stat().st_size
                    _purge(conn, row["id"], tables)
                    items.append({"kind": "qoder", "sid": row["id"], "source": key,
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
        log(f"🗑 Qoder 会话 {stats['deleted']} 条，已移入回收站")
    return stats


def restore_item(item: dict, tag_dir: Path) -> bool:
    key = str(item.get("source") or "")
    if key not in EDITIONS:
        return False
    src = tag_dir / str(item.get("trash_rel") or "")
    try:
        data = json.loads(src.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return False
    if not db_path(key).exists():
        return False
    with lifecycle.client_guard([client(key)]):
        conn = connect_rw(db_path(key))
        try:
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
            _insert(conn, "chat_sessions", [data.get("session") or {}])
            for table, rows in (data.get("side") or {}).items():
                _insert(conn, table, rows)
            conn.commit()
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        finally:
            conn.close()
    return True


# ------------------------------------------------------------------ 同软件复制
def _copy_one(src, dst, sid: str, tables: list[str], stats: dict) -> None:
    """单条会话的复制决策：缺源跳过，已存在不覆盖，副本一律可见。"""
    data = _dump(src, sid, tables)
    if data is None:
        stats["missing"] += 1
        return
    current = dst.execute(
        "SELECT deleted_at FROM chat_sessions WHERE session_id = ?",
        (sid,)).fetchone()
    if current is not None:
        if current["deleted_at"] is None:
            stats["reused"] += 1
        else:
            dst.execute("UPDATE chat_sessions SET deleted_at = NULL, archived = 0 "
                        "WHERE session_id = ?", (sid,))
            stats["resurrected"] += 1
        return
    session = dict(data["session"])
    # 复制过去的副本一律置为可见：把「已删除」状态一起搬过去只会让目标库多一条废行
    session["deleted_at"] = None
    session["archived"] = 0
    _insert(dst, "chat_sessions", [session])
    for table, items in (data.get("side") or {}).items():
        if table in COPY_TABLES:
            _insert(dst, table, items)
    stats["copied"] += 1


def copy_to(target_key: str, rows: list[dict], log) -> dict:
    """把会话复制到另一版本的库；已存在且未删除的跳过，已删除的恢复可见。"""
    if target_key not in EDITIONS:
        raise RuntimeError("目标版本不可用")
    src_key = rows[0]["source_key"]
    if target_key == src_key:
        raise RuntimeError("源版本与目标版本相同，无需复制")
    target_db = db_path(target_key)
    if not target_db.exists():
        raise RuntimeError(f"{EDITIONS[target_key]['label']} 还没有本地会话库，"
                           "请先启动该版本并登录一次")
    with lifecycle.client_guard([client(src_key), client(target_key)], log):
        tag = datetime.now().strftime("%Y%m%d-%H%M%S")
        dest = paths.BACKUP_ROOT / "session-copy" / tag / f"{target_key}.db"
        backup_file(target_db, dest)
        prune(dest.parent.parent, 10)
        log(f"已备份目标会话库：{dest}")

        stats = {"copied": 0, "reused": 0, "resurrected": 0, "missing": 0}
        src = connect_ro(db_path(src_key))
        if src is None:
            raise RuntimeError("源会话库无法读取（客户端可能正在写入）")
        dst = connect_rw(target_db)
        try:
            src.row_factory = sqlite3.Row
            dst.row_factory = sqlite3.Row
            dst.execute("PRAGMA wal_checkpoint(TRUNCATE)")
            tables = _session_tables(src)
            for row in rows:
                _copy_one(src, dst, row["id"], tables, stats)
            dst.commit()
            dst.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        finally:
            src.close()
            dst.close()
    log(f"复制到 {EDITIONS[target_key]['label']}：新增 {stats['copied']}，"
        f"已存在 {stats['reused']}，恢复 {stats['resurrected']}", "ok")
    return stats
