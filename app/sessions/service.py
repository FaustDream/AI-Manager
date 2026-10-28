# -*- coding: utf-8 -*-
"""会话编排层：把两个版本、两个 CodeBuddy 平台的会话摊平成一张表，并提供同步动作。"""
from __future__ import annotations

import re
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


class Snapshot:
    def __init__(self) -> None:
        self.cn = Edition("cn")
        self.intl = Edition("intl")
        self.cn_uid = ""
        self.intl_uid = ""
        self.rows: list[dict] = []
        self.unified: list[dict] = []
        self.plans: list[dict] = []
        self.restores: dict = {}
        self.backups: list[dict] = []
        self.trash: list[dict] = []
        self.cb_sessions: list[dict] = []
        self.cloud_rows: dict = {}      # {edition_key: [标准化云端行]}，拉取失败为 None
        self.pending = 0
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


def build_inventory(snap: Snapshot) -> None:
    cn_rows = {row["id"]: row for row in wbdb.list_local_sessions(snap.cn)}
    intl_rows = {row["id"]: row for row in wbdb.list_local_sessions(snap.intl)}
    merged = []
    for sid in set(cn_rows) | set(intl_rows):
        cn_row, intl_row = cn_rows.get(sid), intl_rows.get(sid)
        files_cn = wbdb.find_session_files(snap.cn, sid)
        files_intl = wbdb.find_session_files(snap.intl, sid)
        files = files_cn or files_intl
        base = cn_row or intl_row or {}
        merged.append({
            "id": sid,
            "title": base.get("custom_title") or base.get("title") or sid,
            "cn": side_state(cn_row, snap.cn_uid),
            "intl": side_state(intl_row, snap.intl_uid),
            "cn_row": cn_row,
            "intl_row": intl_row,
            "last": max(int((cn_row or {}).get("last_activity_at") or 0),
                        int((intl_row or {}).get("last_activity_at") or 0)),
            "size": max(sum(path_size(f) for f in files_cn),
                        sum(path_size(f) for f in files_intl)),
            "messages": count_messages(files),
            "files_cn": [str(f) for f in files_cn],
            "files_intl": [str(f) for f in files_intl],
            "cwd": base.get("cwd") or "",
            "slug": (files[0].parent.name if files else ""),
            "status": base.get("status") or "",
        })
    merged.sort(key=lambda r: r["last"], reverse=True)
    snap.rows = merged
    snap.cn_total = len(cn_rows)
    snap.intl_total = len(intl_rows)
    snap.cloud_total = wbdb.cloud_count(snap.cn) + wbdb.cloud_count(snap.intl)
    snap.cn_visible = sum(1 for r in merged if r["cn"] == "visible")
    snap.intl_visible = sum(1 for r in merged if r["intl"] == "visible")


def _is_deleted(row: dict) -> bool:
    # 云端行用 deleted_at = -1 表示“未删除”（与客户端视图口径一致）
    value = row.get("deleted_at")
    return value is not None and value != -1


def _row_state(row: dict, uid: str) -> str:
    """归属判定：这一行在‘当前账号的会话列表’里应该显示成什么。"""
    if _is_deleted(row):
        return "已删除"
    owner = row.get("user_id") or ""
    if uid and owner and owner != uid:
        return "旧账号"
    if row.get("transport") == "cloud":
        return "云端"
    return "可见"


def build_unified(snap: Snapshot) -> None:
    """把 WorkBuddy 两个版本、CodeBuddy 两个平台的会话摊平成一张列表。

    每条记录代表“某个来源、某个账号下的一条会话”，归属判定：
      · WorkBuddy：sessions.user_id 与该版本 account-snapshot 的 uid 比对，
        旧账号的行保留在本地但标「旧账号」，默认不进当前账号列表；
      · CodeBuddy：目录结构 <uid>/<平台>/<uid>/history 天然按账号隔离，
        目录名 uid 就是归属账号。
    云端行（transport='cloud' 或接口拉回的）没有本地正文，标「云端」。
    """
    rows = []
    info_by_id = {r["id"]: r for r in snap.rows}
    for key, label in (("cn", SOURCE_LABELS["cn"]), ("intl", SOURCE_LABELS["intl"])):
        ep = snap.cn if key == "cn" else snap.intl
        uid = snap.cn_uid if key == "cn" else snap.intl_uid
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
                "size": info.get("size", 0),
                "state": state,
                "has_body": bool(info.get("files_cn") or info.get("files_intl")),
                "files": info.get("files_cn") if key == "cn" else info.get("files_intl"),
                "path": "",
                "cwd": row.get("cwd") or "",
                "client_status": row.get("status") or "",
                "raw": None,
            })
        # 服务端拉回、本地还没有的会话：只列条目（无正文，打开客户端后由它补全）
        for crow in (snap.cloud_rows.get(key) or []):
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
    for session in snap.cb_sessions:
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
    snap.unified = rows


def fetch_cloud_rows(snap: Snapshot, log=None) -> None:
    """为每个版本的当前账号拉取云端会话列表，写入 snap.cloud_rows。

    同步时机之一：账号切换检测到 uid 变化、或用户点「刷新」时触发。
    登录态失效/网络失败都只记日志，本地列表照常出。
    """
    log = log or (lambda m, lv="info": None)
    snap.cloud_rows = {}
    for key, uid in (("cn", snap.cn_uid), ("intl", snap.intl_uid)):
        if not uid:
            snap.cloud_rows[key] = None
            continue
        account = cloud.find_account(uid)
        if account is None:
            snap.cloud_rows[key] = None
            continue
        snap.cloud_rows[key] = cloud.fetch_for_account(account, log)


def build_snapshot(include_deleted: bool = True, with_cloud: bool = True,
                   log=None) -> Snapshot:
    snap = Snapshot()
    snap.cn_uid = snap.cn.current_uid()[0]
    snap.intl_uid = snap.intl.current_uid()[0]
    if with_cloud:
        fetch_cloud_rows(snap, log)
    if snap.cn.exists or snap.intl.exists:
        build_inventory(snap)
        for src, dst in ((snap.cn, snap.intl), (snap.intl, snap.cn)):
            if not (src.exists and dst.exists):
                continue
            items, _skipped = wbdb.build_plan(src, dst, include_deleted, True)
            snap.plans.append({"src": src, "dst": dst, "items": items})
        for ep in (snap.cn, snap.intl):
            if not ep.exists:
                continue
            uid = snap.cn_uid if ep.key == "cn" else snap.intl_uid
            rows = wbdb.build_local_restore(ep, uid)
            if rows:
                snap.restores[ep.key] = rows
        snap.pending = sum(len(plan["items"]) for plan in snap.plans)
    snap.cb_sessions = cb.scan()
    build_unified(snap)
    snap.backups = backup.list_backups()
    snap.trash = trash.list_trash()
    snap.tomb_count = len(trash.tombstone_ids())
    return snap


# ------------------------------------------------------------------ 同步动作
def directions(mode: str) -> list[tuple[str, str]]:
    if mode == "cn2intl":
        return [("cn", "intl")]
    if mode == "intl2cn":
        return [("intl", "cn")]
    return [("cn", "intl"), ("intl", "cn")]


def ensure_ready() -> None:
    cn, intl = Edition("cn"), Edition("intl")
    missing = [ep.label for ep in (cn, intl) if not ep.exists]
    if missing:
        raise RuntimeError("找不到以下版本的数据目录：" + "、".join(missing))


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


def perform_sync(mode: str, include_deleted: bool, do_restore: bool,
                 log, restart: bool = True) -> dict:
    ensure_ready()
    cn, intl = Edition("cn"), Edition("intl")
    pairs = []
    for src_key, dst_key in directions(mode):
        src = cn if src_key == "cn" else intl
        dst = intl if src_key == "cn" else cn
        items, skipped = wbdb.build_plan(src, dst, include_deleted, True)
        pairs.append((src, dst, items, skipped))

    restores = {}
    if do_restore:
        for ep in (cn, intl):
            rows = wbdb.build_local_restore(ep, ep.current_uid()[0])
            if rows:
                restores[ep.key] = (ep, rows)

    total = sum(len(items) for _, _, items, _ in pairs)
    restore_total = sum(len(rows) for _, rows in restores.values())
    if total == 0 and restore_total == 0:
        return {"skipped": True, "moved": 0, "restored": 0, "failed": 0,
                "tag": "", "backup": "", "restore_total": 0}

    closed = close_clients(log)
    tag = trash.new_tag(backup.BACKUP_ROOT)
    log(f"开始备份（{tag}）…")
    bdir = backup.make_backup(pairs, tag)
    log(f"备份完成：{bdir}")

    moved = failed = 0
    for key, (ep, rows) in restores.items():
        uid = ep.current_uid()[0]
        try:
            count = wbdb.apply_local_restore(ep, rows, uid)
            log(f"♻️ {ep.label} 恢复 {count} 条会话可见")
        except Exception as exc:
            failed += 1
            log(f"❌ {ep.label} 恢复失败：{exc}", "error")

    for src, dst, items, _ in pairs:
        if not items:
            continue
        target_uid = dst.current_uid()[0]
        log(f"🔄 {src.label} → {dst.label}（{len(items)} 条）")
        for item in items:
            try:
                wbdb.apply_item(src, dst, item, target_uid)
                moved += 1
                log(f"   ✅ {item.title or item.sid}")
            except Exception as exc:
                failed += 1
                log(f"   ❌ {item.title or item.sid}：{exc}", "error")

    if restart:
        for ep in closed:
            log(f"重新启动 {ep.label}…")
            winproc.start_exe(ep.exe)

    return {"skipped": False, "moved": moved, "restored": restore_total,
            "failed": failed, "tag": tag, "backup": str(bdir)}


def copy_selected(rows: list[dict], kind: str, target, log) -> dict:
    """把选中的会话搬到指定目标（WorkBuddy 版本 或 CodeBuddy 平台+账号）。"""
    if not rows:
        return {"ok": 0, "skip": 0, "fail": 0}
    ok = skip = fail = 0
    if kind == "workbuddy":
        closed = close_clients(log)
        for row in rows:
            if row["source_key"] == target:
                skip += 1
                continue
            try:
                done, message = wbdb.copy_single_session(row["source_key"], target, row["id"])
            except Exception as exc:
                fail += 1
                log(f"   ❌ {row['title']}：{exc}", "error")
                continue
            if done:
                ok += 1
                log(f"   ✅ {row['title']} → {message}")
            else:
                skip += 1
                log(f"   ↷ {row['title']}：{message}")
        for ep in closed:
            winproc.start_exe(ep.exe)
        return {"ok": ok, "skip": skip, "fail": fail}

    dst_platform, dst_uid = target
    for row in rows:
        if (row["source_key"], row["account"]) == (dst_platform, dst_uid):
            skip += 1
            continue
        try:
            done, message = cb.sync_session(row["raw"], dst_platform, dst_uid)
        except Exception as exc:
            fail += 1
            log(f"   ❌ {row['title']}：{exc}", "error")
            continue
        if done:
            ok += 1
            log(f"   ✅ {row['title']} → {message}")
        else:
            skip += 1
            log(f"   ↷ {row['title']}：{message}")
    return {"ok": ok, "skip": skip, "fail": fail}


def delete_ids(ids: list[str], log) -> dict:
    if not ids:
        return {"deleted": 0, "files": 0, "bytes": 0, "trash": ""}
    close_clients(log)
    stats = wbdb.delete_sessions(ids)
    log(f"🗑 删除完成：数据库 {stats['deleted']} 行，文件 {stats['files']} 个，"
        f"释放 {stats['bytes'] / 1048576:.1f} MB")
    return stats


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


def all_targets(snap: Snapshot) -> list[dict]:
    """可选同步目标：WorkBuddy 两个版本 + CodeBuddy 两平台 × 各账号。"""
    targets = [
        {"kind": "workbuddy", "key": "cn",
         "label": "WorkBuddy 国内版"},
        {"kind": "workbuddy", "key": "intl",
         "label": "WorkBuddy 国际版"},
    ]
    seen = set()
    for session in snap.cb_sessions:
        key = (session["platform"], session["uid"])
        if key in seen:
            continue
        seen.add(key)
        targets.append({
            "kind": "codebuddy", "key": key,
            "label": f'{cb.PLATFORM_LABEL.get(session["platform"], session["platform"])}'
                     f' · {session["uid"][:8]}',
        })
    return targets


def save_profile(ep: Edition) -> Path:
    return profiles.save_current(ep)
