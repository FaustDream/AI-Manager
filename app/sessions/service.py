# -*- coding: utf-8 -*-
"""会话编排层：把两个版本、两个 CodeBuddy 平台的会话摊平成一份可管理的清单。

只读本地会话库与正文目录，回答「本机有哪些会话、占多少空间、删哪些」。
账号之间的会话恢复由 app/sessions/restore.py 负责，这一层不做任何跨版本搬运。
"""
from __future__ import annotations

import re
import shutil
from datetime import datetime
from pathlib import Path

from ..core import winproc
from . import backup, cloud, codebuddy as cb, profiles, trash, wbdb
from .editions import Edition, path_size

MSG_PAT = re.compile(rb'"type"\s*:\s*"message"')
_count_cache: dict = {}

SOURCE_LABELS = {
    "cn": "WorkBuddy 国内版",
    "intl": "WorkBuddy 国际版",
    "CodeBuddyIDE": "CodeBuddy IDE",
    "JetBrains": "IDEA 插件",
}

STALE_DAYS = 180


def count_messages(files) -> int:
    total = 0
    for path in files:
        path = Path(path)
        if path.suffix != ".jsonl" or not path.is_file():
            continue
        try:
            stat = path.stat()
        except OSError:
            continue
        key = (str(path), stat.st_mtime, stat.st_size)
        if key in _count_cache:
            total += _count_cache[key]
            continue
        count = 0
        try:
            with open(path, "rb") as fh:
                for chunk in iter(lambda: fh.read(1 << 20), b""):
                    count += len(MSG_PAT.findall(chunk))
        except OSError:
            count = 0
        _count_cache[key] = count
        total += count
    return total


def side_state(row, uid: str) -> str:
    if row is None:
        return "absent"
    value = row.get("deleted_at")
    if value is not None and value != -1:
        return "deleted"
    owner = row.get("user_id") or ""
    if uid and owner and owner != uid:
        return "other"
    return "visible"


class Catalog:
    """一次扫描的结果：会话清单 + 空间账本 + 可选的云端列表。"""

    def __init__(self) -> None:
        self.cn = Edition("cn")
        self.intl = Edition("intl")
        self.cn_uid = ""
        self.intl_uid = ""
        self.rows: list[dict] = []          # 合并后的本地会话（两个版本按 id 去重）
        self.unified: list[dict] = []       # 表格行：来源 × 账号 × 会话
        self.cb_sessions: list[dict] = []
        self.cloud_rows: dict = {}          # {edition_key: [云端行]}，未拉取时为空
        self.orphans: list[dict] = []       # 没有任何会话行指向的正文文件
        self.space: dict = {}
        self.backups: list[dict] = []
        self.trash: list[dict] = []
        self.tomb_count = 0
        self.cloud_total = 0
        self.cn_total = 0
        self.intl_total = 0
        self.cn_visible = 0
        self.intl_visible = 0
        self.scanned_at = datetime.now()
        self.errors: list[str] = []

    @property
    def available(self) -> bool:
        return self.cn.exists or self.intl.exists

    @property
    def total_size(self) -> int:
        return sum(row["size"] for row in self.rows)


def build_inventory(cat: Catalog) -> None:
    cn_rows = {row["id"]: row for row in wbdb.list_local_sessions(cat.cn)}
    intl_rows = {row["id"]: row for row in wbdb.list_local_sessions(cat.intl)}
    cn_files = wbdb.project_files(cat.cn)
    intl_files = wbdb.project_files(cat.intl)

    merged = []
    for sid in set(cn_rows) | set(intl_rows):
        cn_row, intl_row = cn_rows.get(sid), intl_rows.get(sid)
        files_cn = cn_files.get(sid, [])
        files_intl = intl_files.get(sid, [])
        files = files_cn or files_intl
        base = cn_row or intl_row or {}
        size_cn = sum(path_size(f) for f in files_cn)
        size_intl = sum(path_size(f) for f in files_intl)
        merged.append({
            "id": sid,
            "title": base.get("custom_title") or base.get("title") or sid,
            "cn": side_state(cn_row, cat.cn_uid),
            "intl": side_state(intl_row, cat.intl_uid),
            "cn_row": cn_row,
            "intl_row": intl_row,
            "last": max(int((cn_row or {}).get("last_activity_at") or 0),
                        int((intl_row or {}).get("last_activity_at") or 0)),
            "size": max(size_cn, size_intl),
            "size_cn": size_cn,
            "size_intl": size_intl,
            "messages": count_messages(files),
            "files_cn": [str(f) for f in files_cn],
            "files_intl": [str(f) for f in files_intl],
            "cwd": base.get("cwd") or "",
            "slug": (files[0].parent.name if files else ""),
            "status": base.get("status") or "",
        })
    merged.sort(key=lambda r: r["last"], reverse=True)
    cat.rows = merged
    cat.cn_total = len(cn_rows)
    cat.intl_total = len(intl_rows)
    cat.cn_visible = sum(1 for r in merged if r["cn"] == "visible")
    cat.intl_visible = sum(1 for r in merged if r["intl"] == "visible")
    cat.cloud_total = wbdb.cloud_count(cat.cn) + wbdb.cloud_count(cat.intl)
    build_space(cat, cn_files, intl_files)


def build_space(cat: Catalog, cn_files: dict, intl_files: dict) -> None:
    """空间账本：正文总量、已删除仍占盘、没有会话行指向的孤立文件。"""
    space = {"files": 0, "bytes": 0, "deleted_rows": 0, "deleted_bytes": 0,
             "empty_rows": 0, "orphan_files": 0, "orphan_bytes": 0}
    cat.orphans = []
    for key, ep, index in (("cn", cat.cn, cn_files), ("intl", cat.intl, intl_files)):
        known = wbdb.all_session_ids(ep)
        for sid, files in index.items():
            size = sum(path_size(f) for f in files)
            space["files"] += len(files)
            space["bytes"] += size
            if sid in known:
                continue
            space["orphan_files"] += len(files)
            space["orphan_bytes"] += size
            cat.orphans.append({"edition": key, "sid": sid,
                                "files": [str(f) for f in files], "size": size})

    deleted = set()
    for row in cat.rows:
        if row["cn"] == "deleted":
            deleted.add(row["id"])
            space["deleted_bytes"] += row["size_cn"]
        if row["intl"] == "deleted":
            deleted.add(row["id"])
            space["deleted_bytes"] += row["size_intl"]
        if not row["files_cn"] and not row["files_intl"]:
            space["empty_rows"] += 1
    space["deleted_rows"] = len(deleted)
    cat.space = space


def _row_state(row: dict, uid: str) -> str:
    """归属判定：这一行在「当前账号的会话列表」里应该显示成什么。"""
    value = row.get("deleted_at")
    if value is not None and value != -1:
        return "已删除"
    owner = row.get("user_id") or ""
    if uid and owner and owner != uid:
        return "旧账号"
    if row.get("transport") == "cloud":
        return "云端"
    return "可见"


def build_unified(cat: Catalog) -> None:
    """把 WorkBuddy 两个版本、CodeBuddy 两个平台的会话摊平成一张列表。

    每条记录代表「某个来源、某个账号下的一条会话」，归属判定：
      · WorkBuddy：sessions.user_id 与该版本 account-snapshot 的 uid 比对，
        其他账号的行标「旧账号」，默认不进当前账号列表；
      · CodeBuddy：目录结构 <uid>/<平台>/<uid>/history 天然按账号隔离，
        目录名 uid 就是归属账号。
    云端行（transport='cloud' 或接口拉回的）没有本地正文，标「云端」。
    """
    rows = []
    info_by_id = {r["id"]: r for r in cat.rows}
    for key, label in (("cn", SOURCE_LABELS["cn"]), ("intl", SOURCE_LABELS["intl"])):
        ep = cat.cn if key == "cn" else cat.intl
        uid = cat.cn_uid if key == "cn" else cat.intl_uid
        seen_ids: set[str] = set()
        for row in wbdb.list_all_sessions(ep):
            seen_ids.add(row["id"])
            state = _row_state(row, uid)
            info = info_by_id.get(row["id"], {})
            owner = row.get("user_id") or ""
            rows.append({
                "kind": "workbuddy",
                "source_key": key,
                "source_label": label,
                "account": uid if state != "旧账号" else owner,
                "id": row["id"],
                "title": row.get("custom_title") or row.get("title") or row["id"],
                "last": int(row.get("last_activity_at") or row.get("created_at") or 0),
                "msgs": info.get("messages", 0),
                "size": (info.get("size_cn") if key == "cn"
                         else info.get("size_intl")) or 0,
                "state": state,
                "has_body": bool(info.get("files_cn") or info.get("files_intl")),
                "files": info.get("files_cn") if key == "cn" else info.get("files_intl"),
                "path": "",
                "cwd": row.get("cwd") or "",
                "client_status": row.get("status") or "",
                "raw": None,
            })
        # 服务端拉回、本地还没有的会话：只列条目（无正文，打开客户端后由它补全）
        for crow in (cat.cloud_rows.get(key) or []):
            if crow["id"] in seen_ids:
                continue
            rows.append({
                "kind": "workbuddy",
                "source_key": key,
                "source_label": label,
                "account": crow.get("user_id") or uid,
                "id": crow["id"],
                "title": crow.get("title") or crow["id"],
                "last": crow.get("last_activity_at") or crow.get("updated_at") or 0,
                "msgs": 0,
                "size": 0,
                "state": "云端",
                "has_body": False,
                "files": [],
                "path": "",
                "cwd": crow.get("work_dir") or "",
                "client_status": crow.get("status") or "",
                "raw": None,
            })
    for session in cat.cb_sessions:
        rows.append({
            "kind": "codebuddy",
            "source_key": session["platform"],
            "source_label": cb.PLATFORM_LABEL.get(session["platform"], session["platform"]),
            "account": session["uid"],
            "id": session["id"],
            "title": session["name"] or f"（未命名 {session['id'][:8]}）",
            "last": session["last"],
            "msgs": session["msgs"],
            "size": session["size"],
            "state": "有正文" if session["has_body"] else "无正文",
            "has_body": session["has_body"],
            "files": [],
            "path": session["path"],
            "cwd": "",
            "client_status": "",
            "raw": session,
        })
    rows.sort(key=lambda r: r["last"], reverse=True)
    cat.unified = rows


def pull_cloud(cat: Catalog, log=None) -> int:
    """按需拉取两个版本当前账号的云端会话列表。

    登录态失效/网络失败都只记日志：本地列表照常出，云端只是补充信息。
    """
    log = log or (lambda m, lv="info": None)
    cat.cloud_rows = {}
    for key, uid in (("cn", cat.cn_uid), ("intl", cat.intl_uid)):
        if not uid:
            cat.cloud_rows[key] = None
            continue
        account = cloud.find_account(uid)
        if account is None:
            cat.cloud_rows[key] = None
            continue
        cat.cloud_rows[key] = cloud.fetch_for_account(account, log)
    return sum(len(rows or []) for rows in cat.cloud_rows.values())


def build_catalog(with_cloud: bool = False, log=None) -> Catalog:
    cat = Catalog()
    cat.cn_uid = cat.cn.current_uid()[0]
    cat.intl_uid = cat.intl.current_uid()[0]
    if with_cloud:
        pull_cloud(cat, log)
    if cat.cn.exists or cat.intl.exists:
        build_inventory(cat)
    cat.cb_sessions = cb.scan()
    build_unified(cat)
    cat.backups = backup.list_backups()
    cat.trash = trash.list_trash()
    cat.tomb_count = len(trash.tombstone_ids())
    return cat


# ------------------------------------------------------------------ 写入动作
def close_clients(log) -> list[Edition]:
    """写入前必须关客户端：内存缓存会覆盖改动，WAL 也可能不落盘。"""
    closed = []
    for ep in (Edition("cn"), Edition("intl")):
        if not winproc.process_running(ep.proc):
            continue
        log(f"正在关闭 {ep.label}…")
        if winproc.kill_process(ep.proc):
            closed.append(ep)
        else:
            raise RuntimeError(f"{ep.label} 没能关闭，请手动退出后重试")
    return closed


def delete_ids(ids: list[str], log) -> dict:
    """删除 WorkBuddy 会话：数据库行 + 正文 + 任务/待办，文件先进回收站。"""
    if not ids:
        return {"deleted": 0, "files": 0, "bytes": 0, "trash": "", "ids": []}
    close_clients(log)
    stats = wbdb.delete_sessions(ids)
    log(f"🗑 删除完成：数据库 {stats['deleted']} 行，文件 {stats['files']} 个，"
        f"释放 {stats['bytes'] / 1048576:.1f} MB")
    return stats


def delete_codebuddy(sessions: list[dict], log) -> dict:
    """删除 CodeBuddy 会话：目录移入回收站，并从工作区索引里摘掉。"""
    stats = {"sessions": 0, "files": 0, "bytes": 0, "trash": ""}
    if not sessions:
        return stats
    tag = trash.new_tag(trash.TRASH_ROOT)
    items = []
    for session in sessions:
        try:
            item = cb.remove_session(session, tag)
        except OSError as exc:
            log(f"❌ {session.get('name') or session.get('id')}：{exc}", "error")
            continue
        if item:
            items.append(item)
            stats["sessions"] += 1
            stats["bytes"] += int(item.get("size") or 0)
    if items:
        trash.write_manifest(tag, items)
        stats["trash"] = str(trash.TRASH_ROOT / tag)
        log(f"🗑 CodeBuddy 会话 {stats['sessions']} 条，"
            f"释放 {stats['bytes'] / 1048576:.1f} MB")
    return stats


def purge_orphans(cat: Catalog, log) -> dict:
    """把没有会话行指向的正文文件移入回收站。

    客户端删会话时只清数据库行，projects 下的正文可能留下来；这些文件
    不会再被任何界面引用，是纯占用。
    """
    stats = {"groups": 0, "files": 0, "bytes": 0, "trash": ""}
    if not cat.orphans:
        return stats
    tag = trash.new_tag(trash.TRASH_ROOT)
    items = []
    for group in cat.orphans:
        ep = Edition(group["edition"])
        record = {"sid": group["sid"], "edition": ep.key, "files": [],
                  "size": 0, "row": None, "usage": None}
        for raw in group["files"]:
            path = Path(raw)
            try:
                rel = path.relative_to(ep.root)
            except ValueError:
                continue
            dest = trash.TRASH_ROOT / tag / ep.key / rel
            dest.parent.mkdir(parents=True, exist_ok=True)
            if dest.exists():
                continue
            try:
                shutil.move(str(path), str(dest))
            except OSError as exc:
                log(f"移动孤立文件失败：{exc}", "warn")
                continue
            record["files"].append(str(rel))
            record["size"] += path_size(dest)
            stats["files"] += 1
        if record["files"]:
            items.append(record)
            stats["groups"] += 1
            stats["bytes"] += record["size"]
    if items:
        trash.write_manifest(tag, items)
        stats["trash"] = str(trash.TRASH_ROOT / tag)
        log(f"🧹 清理孤立正文 {stats['files']} 个文件，"
            f"释放 {stats['bytes'] / 1048576:.1f} MB")
    return stats


def backup_now(log) -> str:
    """手动整库快照：同步去掉之后，回滚之前得先有东西可回。"""
    tag = trash.new_tag(backup.BACKUP_ROOT)
    log(f"正在备份会话库（{tag}）…")
    path = backup.make_snapshot(tag)
    log(f"备份完成：{path}")
    return str(path)


def cleanup_codebuddy(mode: str) -> dict:
    return cb.cleanup_empty(mode)


def rollback(tag: str) -> None:
    backup.rollback(tag)


def restore_trash(tag: str, log) -> int:
    close_clients(log)
    return trash.restore_trash(tag)


def purge_trash(tag: str | None = None) -> int:
    return trash.purge_trash(tag)


def clear_blacklist() -> int:
    return trash.clear_tombstones()


def save_profile(ep: Edition) -> Path:
    return profiles.save_current(ep)
