# -*- coding: utf-8 -*-
"""账号维度的会话恢复：只改归属引用，不复制正文。

同一个版本里一条会话只有一份正文（projects/<slug>/<sid>.jsonl），客户端按
sessions.user_id 过滤会话列表。所以“把 A 的会话恢复到 B”不需要搬文件，只要把
已有会话行的 user_id 指向 B、清掉软删除标记即可；正文原地复用，不会产生副本。

data/session_index.json 记录每条会话的原始归属（origin）、被哪些账号引用过
（refs）、所属项目与正文位置，用于重复恢复时去重和事后核对“这条会话在哪”。
"""
from __future__ import annotations

import json
import os
import sqlite3
import time
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path

from ..core import paths, winproc
from . import backup, trash, wbdb
from .editions import (Edition, all_columns, connect_ro, connect_rw,
                       remove_path, table_columns)

_INDEX_VERSION = 1
_STAMP_FMT = "%Y-%m-%dT%H:%M:%S"
_EDITION_KEYS = ("cn", "intl")


def _stamp() -> str:
    return datetime.now().strftime(_STAMP_FMT)


# ------------------------------------------------------------------ 会话索引
def _blank_index() -> dict:
    return {
        "version": _INDEX_VERSION,
        "updated_at": "",
        "editions": {key: {"sessions": {}} for key in _EDITION_KEYS},
    }


def load_index() -> dict:
    """读 data/session_index.json；缺失或损坏时按空索引处理。"""
    try:
        raw = json.loads(paths.SESSION_INDEX_FILE.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return _blank_index()
    if not isinstance(raw, dict):
        return _blank_index()
    index = _blank_index()
    editions = raw.get("editions")
    if isinstance(editions, dict):
        for key in _EDITION_KEYS:
            bucket = editions.get(key)
            if isinstance(bucket, dict) and isinstance(bucket.get("sessions"), dict):
                index["editions"][key]["sessions"] = bucket["sessions"]
    index["updated_at"] = str(raw.get("updated_at") or "")
    return index


def _save_index(index: dict, log=None) -> None:
    """先写临时文件再替换：索引坏了只会丢历史，不能丢当前会话。"""
    index["updated_at"] = _stamp()
    try:
        path = paths.SESSION_INDEX_FILE
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_suffix(".json.tmp")
        tmp.write_text(json.dumps(index, ensure_ascii=False, indent=2), encoding="utf-8")
        os.replace(tmp, path)
    except OSError as exc:
        if log is not None:
            log(f"会话索引写入失败：{exc}", "error")


def _note(index: dict, ep: Edition, row: dict, origin: str, ref_uid: str,
          files) -> None:
    """登记一条会话：origin 只写一次，refs 记录所有引用过它的账号。"""
    bucket = index["editions"].setdefault(ep.key, {"sessions": {}})["sessions"]
    sid = row["id"]
    entry = bucket.get(sid)
    if not isinstance(entry, dict):
        entry = {}
    entry["origin"] = entry.get("origin") or origin or ref_uid
    refs = entry.get("refs")
    refs = set(refs) if isinstance(refs, list) else set()
    refs.add(ref_uid)
    entry["refs"] = sorted(refs)
    entry["cwd"] = row.get("cwd") or ""
    entry["slug"] = wbdb.slug_of(files, row.get("cwd") or "") if files else ""
    entry["title"] = row.get("custom_title") or row.get("title") or sid
    entry["last"] = int(row.get("last_activity_at") or row.get("updated_at") or 0)
    entry["seen_at"] = _stamp()
    bucket[sid] = entry


# ------------------------------------------------------------------ 统计
@dataclass
class ProjectStat:
    cwd: str
    total: int = 0
    changed: int = 0
    reused: int = 0
    missing: int = 0


@dataclass
class RestoreReport:
    edition: str
    uid: str
    source: tuple[str, ...] = ()
    total: int = 0
    changed: int = 0
    reused: int = 0
    resurrected: int = 0
    missing: int = 0
    merged: int = 0
    unverified: int = 0
    backup: str = ""
    projects: dict[str, ProjectStat] = field(default_factory=dict)

    def summary(self) -> str:
        restored_projects = sum(1 for stat in self.projects.values() if stat.total)
        text = f"恢复完成：{restored_projects} 个项目 / {self.total} 条会话"
        if self.changed:
            text += f"，新关联 {self.changed}"
        if self.reused:
            text += f"，已存在 {self.reused}"
        if self.merged:
            text += f"，补回 {self.merged}"
        if self.missing:
            text += f"，缺正文 {self.missing}"
        if self.unverified:
            text += f"，未确认 {self.unverified}"
        if not self.changed and not self.merged:
            text += "（无变化）"
        return text

    def detail(self, limit: int = 12) -> str:
        lines = [self.summary()]
        if self.backup:
            lines.append(f"会话库备份：{self.backup}")
        ordered = sorted(self.projects.values(), key=lambda p: p.total, reverse=True)
        for stat in ordered[:limit]:
            name = stat.cwd or "（未知项目）"
            line = f"· {name}：{stat.total} 条"
            if stat.changed:
                line += f"，新关联 {stat.changed}"
            if stat.reused:
                line += f"，已存在 {stat.reused}"
            if stat.missing:
                line += f"，缺正文 {stat.missing}"
            lines.append(line)
        if len(ordered) > limit:
            lines.append(f"· 其余 {len(ordered) - limit} 个项目略")
        if self.missing:
            lines.append("缺正文的会话没有可复用的内容，已跳过；请先确认 projects 目录是否完整。")
        return "\n".join(lines)


def preview(ep: Edition, uid: str, source_uids: set[str] | None = None) -> dict:
    """恢复前预览：只读，不需要关客户端。"""
    blocked = trash.tombstone_ids()
    projects: dict[str, int] = {}
    total = pending = missing = 0
    for row in wbdb.list_local_sessions(ep):
        sid = row["id"]
        if sid in blocked:
            continue
        owner = row.get("user_id") or ""
        if source_uids is not None and owner not in source_uids:
            continue
        if not wbdb.find_session_files(ep, sid):
            missing += 1
            continue
        total += 1
        key = row.get("cwd") or ""
        projects[key] = projects.get(key, 0) + 1
        # 本地行只认 deleted_at IS NULL 为可见；-1 是云端行的“未删除”哨兵，
        # 落到本地行上同样不可见，恢复时要清成 NULL
        deleted = row.get("deleted_at") is not None
        if owner != uid or deleted:
            pending += 1
    return {"total": total, "pending": pending, "missing": missing,
            "projects": len(projects)}


# ------------------------------------------------------------------ 备份
def _backup_live(ep: Edition, tag: str) -> str:
    """恢复前备份会话库；失败直接抛出，宁可不恢复也不要在没有退路时改库。"""
    dest = paths.BACKUP_ROOT / "session-restore" / tag / f"{ep.key}.db"
    backup.backup_db(ep, dest)
    return str(dest)


def _write_meta(tag: str, payload: dict) -> None:
    try:
        target = paths.BACKUP_ROOT / "session-restore" / tag / "meta.json"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(json.dumps(payload, ensure_ascii=False, indent=2),
                          encoding="utf-8")
    except OSError:
        pass


def _prune_backups(keep: int = 20) -> None:
    """只留最近若干份恢复前快照：每份不到 1MB，但没必要无限增长。"""
    base = paths.BACKUP_ROOT / "session-restore"
    if not base.exists():
        return
    dirs = sorted((p for p in base.iterdir() if p.is_dir()), reverse=True)
    for old in dirs[keep:]:
        remove_path(old)


# ------------------------------------------------------------------ 旧档案补回
def _pack_dbs(ep: Edition) -> list[Path]:
    base = paths.PROFILE_ROOT / ep.key
    if not base.exists():
        return []
    return sorted(p for p in base.glob("*/workbuddy.db") if p.is_file())


def _merge_packs(ep: Edition, log) -> int:
    """把旧账号档案里的会话行补回当前库。

    旧版本切换账号时会整库替换 workbuddy.db，某些会话可能只留在档案里。
    这里只补“当前库完全没有”的行，不覆盖、不复活已删除的行：
    档案是旧快照，不能拿它推翻现在的删除状态。
    """
    live_ids = {row["id"] for row in wbdb.list_local_sessions(ep)}
    blocked = trash.tombstone_ids()
    pending: list[tuple[dict, dict | None, dict | None]] = []
    for db in _pack_dbs(ep):
        pack_uid = db.parent.name
        conn = connect_ro(db)
        if conn is None:
            continue
        try:
            conn.row_factory = sqlite3.Row
            for raw in conn.execute(
                    "SELECT * FROM sessions WHERE transport='local'"):
                row = dict(raw)
                sid = row.get("id")
                if not sid or sid in live_ids or sid in blocked:
                    continue
                deleted = row.get("deleted_at")
                if deleted is not None and deleted != -1:
                    continue          # 档案里就是删掉的，不要复活
                if not wbdb.find_session_files(ep, sid):
                    continue          # 没有正文，补回去只会变成空对话
                row["deleted_at"] = None
                row["user_id"] = row.get("user_id") or pack_uid
                usage = conn.execute(
                    "SELECT * FROM session_usage WHERE session_id = ?", (sid,)
                ).fetchone()
                buddy = None
                if row.get("buddy_snapshot_id"):
                    buddy = conn.execute(
                        "SELECT * FROM buddy_snapshots WHERE snapshot_id = ?",
                        (row["buddy_snapshot_id"],),
                    ).fetchone()
                pending.append((
                    row,
                    dict(usage) if usage is not None else None,
                    dict(buddy) if buddy is not None else None,
                ))
        except sqlite3.Error as exc:
            log(f"读取旧档案失败：{exc}", "warn")
        finally:
            conn.close()

    if not pending:
        return 0

    conn = connect_rw(ep.db)
    added = 0
    try:
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        session_cols = set(table_columns(conn, "sessions"))
        usage_cols = set(table_columns(conn, "session_usage"))
        try:
            buddy_cols = set(all_columns(conn, "buddy_snapshots"))
        except sqlite3.Error:
            buddy_cols = set()
        for row, usage, buddy in pending:
            names = [name for name in row if name in session_cols]
            try:
                cur = conn.execute(
                    f"INSERT OR IGNORE INTO sessions ({','.join(names)}) "
                    f"VALUES ({','.join('?' * len(names))})",
                    [row[name] for name in names],
                )
            except sqlite3.Error as exc:
                # 旧档案的表结构可能比当前客户端少列，单条失败不要拖垮整轮
                log(f"补回会话 {str(row.get('id'))[:8]} 失败：{exc}", "warn")
                continue
            if cur.rowcount <= 0:
                continue
            added += 1
            if usage:
                ucols = [name for name in usage if name in usage_cols]
                conn.execute(
                    f"INSERT OR REPLACE INTO session_usage ({','.join(ucols)}) "
                    f"VALUES ({','.join('?' * len(ucols))})",
                    [usage[name] for name in ucols],
                )
            cwd = row.get("cwd")
            if cwd:
                conn.execute(
                    "INSERT OR IGNORE INTO workspaces (path, last_opened_at) VALUES (?, ?)",
                    (cwd, int(row.get("last_activity_at") or time.time() * 1000)),
                )
            if buddy and buddy_cols:
                bcols = [name for name in buddy if name in buddy_cols]
                conn.execute(
                    f"INSERT OR IGNORE INTO buddy_snapshots ({','.join(bcols)}) "
                    f"VALUES ({','.join('?' * len(bcols))})",
                    [buddy[name] for name in bcols],
                )
        conn.commit()
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    finally:
        conn.close()
    if added:
        log(f"已从旧账号档案补回 {added} 条会话引用")
    return added


# ------------------------------------------------------------------ 恢复主流程
def restore_for_account(ep: Edition, uid: str,
                        source_uids: set[str] | None = None,
                        log=None) -> RestoreReport:
    """把本版本已有的项目会话关联到 uid。

    source_uids 为 None 表示“全部已有会话”（手动登录后的「恢复会话」）；
    传集合时只处理这些账号名下的会话（账号切换时只搬切换前账号那一份）。

    调用方必须先关闭客户端；本函数会先备份会话库，再改 user_id / deleted_at，
    最后核对目标账号的可见清单。重复执行不会产生新行，也不会复制正文。
    """
    log = log or (lambda message, level="info": None)
    if not uid:
        raise ValueError("缺少目标账号，无法恢复会话")
    if not ep.exists:
        raise RuntimeError(f"找不到 {ep.label} 的会话库：{ep.db}")
    if winproc.process_running(ep.proc):
        raise RuntimeError(f"{ep.label} 正在运行，请先关闭客户端再恢复会话")

    tag = datetime.now().strftime("%Y%m%d-%H%M%S")
    backup_path = _backup_live(ep, tag)
    merged = _merge_packs(ep, log)

    blocked = trash.tombstone_ids()
    rows = wbdb.list_local_sessions(ep)
    index = load_index()
    report = RestoreReport(
        edition=ep.key, uid=uid,
        source=tuple(sorted(source_uids)) if source_uids else (),
        merged=merged, backup=backup_path,
    )
    expected: set[str] = set()
    notes: list[tuple[dict, str, list]] = []
    conn = connect_rw(ep.db)
    try:
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        for row in rows:
            sid = row["id"]
            if sid in blocked:
                continue
            owner = row.get("user_id") or ""
            if source_uids is not None and owner not in source_uids:
                continue
            files = wbdb.find_session_files(ep, sid)
            key = row.get("cwd") or ""
            stat = report.projects.get(key)
            if stat is None:
                stat = report.projects[key] = ProjectStat(cwd=key)
            if not files:
                stat.missing += 1
                report.missing += 1
                continue
            stat.total += 1
            report.total += 1
            expected.add(sid)
            deleted = row.get("deleted_at") is not None
            if owner != uid or deleted:
                cur = conn.execute(
                    "UPDATE sessions SET user_id = ?, deleted_at = NULL "
                    "WHERE id = ? AND transport = 'local'",
                    (uid, sid),
                )
                if cur.rowcount > 0:
                    report.changed += 1
                    stat.changed += 1
                    if deleted:
                        report.resurrected += 1
                # rowcount == 0：行在读取后被改掉了，交给末尾的可见性校验兜底
            else:
                report.reused += 1
                stat.reused += 1
            notes.append((row, owner, files))
            cwd = row.get("cwd")
            if cwd:
                conn.execute(
                    "INSERT OR IGNORE INTO workspaces (path, last_opened_at) VALUES (?, ?)",
                    (cwd, int(row.get("last_activity_at") or time.time() * 1000)),
                )
        conn.commit()
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        # 提交成功后再写索引：索引只记录真正生效的引用
        for note_row, note_owner, note_files in notes:
            _note(index, ep, note_row, note_owner, uid, note_files)
    except Exception:
        conn.rollback()
        raise
    finally:
        conn.close()

    # 完整性校验：目标账号的可见清单必须覆盖本次处理过的全部会话
    visible: set[str] = set()
    ro = connect_ro(ep.db)
    if ro is not None:
        try:
            visible = {r[0] for r in ro.execute(
                "SELECT id FROM sessions WHERE transport = 'local' "
                "AND deleted_at IS NULL AND (user_id = ? OR user_id = '')",
                (uid,),
            )}
        finally:
            ro.close()
    report.unverified = len(expected - visible)
    if report.unverified:
        log(f"{ep.label} 有 {report.unverified} 条会话恢复后仍不可见，请检查会话库", "error")

    _save_index(index, log)
    _write_meta(tag, {
        "edition": ep.key, "uid": uid,
        "source": list(report.source), "created_at": _stamp(),
        "total": report.total, "changed": report.changed,
        "reused": report.reused, "missing": report.missing,
        "merged": report.merged, "unverified": report.unverified,
    })
    _prune_backups()
    level = "ok" if not report.missing and not report.unverified else "warn"
    log(f"{ep.label} {report.summary()}", level)
    return report
