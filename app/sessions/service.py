# -*- coding: utf-8 -*-
"""会话编排层：把各软件的会话摊平成一份可管理的清单。

清单走 data/catalog.db 的轻量索引——只存标题、时间、条数、体积这些索引信息，
正文一律留在客户端原存储里，选中会话才读。源指纹（root_fp）没变的来源整体复用
缓存，消息条数按「文件 mtime:size」缓存，所以刷新不用每次全量重扫。
写入动作统一由 lifecycle 关客户端、写完自动重启，之后让对应来源的索引失效。
"""
from __future__ import annotations

import json
import re
from pathlib import Path

from ..core import tasks
from . import backup, codebuddy as cb, indexdb, lifecycle, others, profiles, trash, wbdb
from .editions import Edition, path_size

MSG_PAT = re.compile(rb'"type"\s*:\s*"message"')

SOURCE_LABELS = {
    "cn": "WorkBuddy 国内版",
    "intl": "WorkBuddy 国际版",
    "CodeBuddyIDE": "CodeBuddy IDE",
    "JetBrains": "IDEA 插件",
}

STALE_DAYS = 180


def source_labels() -> list[str]:
    """软件筛选下拉的取值，与 unified 行的 source_label 一一对应。"""
    return [SOURCE_LABELS["cn"], SOURCE_LABELS["intl"],
            cb.PLATFORM_LABEL["CodeBuddyIDE"], cb.PLATFORM_LABEL["JetBrains"],
            *others.labels()]


# ------------------------------------------------------------------ 昵称
def account_names(log=None) -> dict[str, str]:
    """uid → 昵称：建索引时解析一次，界面只显示昵称，不摆 uid。

    两个来源：账号库（当前保存的账号）+ 账号档案里的 account-snapshot
    （已经不在账号库里、但会话还留着的旧账号，快照里存着昵称）。
    """
    log = log or (lambda m, level="info": None)
    names: dict[str, str] = {}
    try:
        from ..checkin import store as account_store
        for item in account_store.load():
            if item.get("platform") != "workbuddy":
                continue
            meta = item.get("meta") or {}
            uid = str(meta.get("uid") or item.get("account_id") or "")
            name = str(item.get("name") or "").strip()
            if uid and name:
                names[uid] = name
    except Exception as exc:                      # noqa: BLE001 - 昵称解析失败不挡列表
        log(f"读取账号库失败：{exc}", "dim")

    try:
        from ..core import paths
        for snap in paths.PROFILE_ROOT.glob("*/*/storage/skeleton/account-snapshot.json"):
            uid = snap.parents[2].name
            if uid in names:
                continue
            try:
                primary = json.loads(snap.read_text(encoding="utf-8")).get("primary") or {}
            except (OSError, ValueError):
                continue
            nickname = str(primary.get("nickname") or "").strip()
            if nickname:
                names[str(primary.get("uid") or uid)] = nickname
    except OSError as exc:
        log(f"读取账号档案失败：{exc}", "dim")
    return names


def _account_name(row: dict, names: dict[str, str], cn_uid: str, intl_uid: str) -> str:
    """账号列的显示名：昵称优先，界面任何位置都不出现 uid。"""
    if row["kind"] in ("qoder", "zcode", "trae"):
        return "本机"                      # 这些库没有账号字段，换号共用一份
    uid = row.get("account") or ""
    if not uid or uid == "default":
        return "未登录"
    tag = "国内版" if uid == cn_uid else ("国际版" if uid == intl_uid else "")
    name = names.get(uid)
    if name:
        return f"{name}（{tag}）" if tag else name
    return tag or "未知账号"


# ------------------------------------------------------------------ 消息计数
def count_messages(files, index: indexdb.Index | None = None) -> int:
    """jsonl 正文里的消息条数；按文件指纹缓存，没变的文件不重读内容。"""
    total = 0
    for path in files:
        path = Path(path)
        if path.suffix != ".jsonl" or not path.is_file():
            continue
        try:
            stat = path.stat()
        except OSError:
            continue
        cache_key = f"msg:{path}"
        fp = f"{stat.st_mtime_ns}:{stat.st_size}"
        if index is not None:
            hit = index.count(cache_key, fp)
            if hit is not None:
                total += int(hit)
                continue
        count = 0
        try:
            with open(path, "rb") as fh:
                for chunk in iter(lambda: fh.read(1 << 20), b""):
                    count += len(MSG_PAT.findall(chunk))
        except OSError:
            count = 0
        if index is not None:
            index.put_count(cache_key, fp, str(count))
        total += count
    return total


class Catalog:
    """一次扫描的结果：会话清单 + 备份 / 回收站快照。"""

    def __init__(self) -> None:
        self.cn = Edition("cn")
        self.intl = Edition("intl")
        self.cn_uid = ""
        self.intl_uid = ""
        self.rows: list[dict] = []          # WorkBuddy 会话（按 id 去重，清空用）
        self.unified: list[dict] = []       # 表格行：来源 × 账号 × 会话
        self.cb_sessions: list[dict] = []
        self.others: list[dict] = []
        self.backups: list[dict] = []
        self.trash: list[dict] = []
        self.tomb_count = 0
        self.cn_total = 0
        self.intl_total = 0
        self.errors: list[str] = []


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


def _wb_rows(ep: Edition, key: str, uid: str, names: dict, cat: Catalog,
             index: indexdb.Index, errors: list[str]) -> list[dict]:
    """单个 WorkBuddy 版本摊平出的统一行。"""
    out = []
    files_by_id = wbdb.project_files(ep)
    for row in wbdb.list_all_sessions(ep, errors):
        sid = row.get("id") or ""
        files = [str(item) for item in files_by_id.get(sid, [])]
        state = _row_state(row, uid)
        record = {
            "kind": "workbuddy",
            "source_key": key,
            "source_label": SOURCE_LABELS[key],
            "account": uid if state != "旧账号" else (row.get("user_id") or ""),
            "id": sid,
            "title": row.get("custom_title") or row.get("title") or "（未命名会话）",
            "last": int(row.get("last_activity_at") or row.get("created_at") or 0),
            "msgs": count_messages(files, index),
            "size": sum(path_size(Path(item)) for item in files),
            "state": state,
            "has_body": bool(files),
            "files": files,
            "path": "",
            "cwd": row.get("cwd") or "",
            "client_status": row.get("status") or "",
            "raw": None,
        }
        record["account_name"] = _account_name(record, names, cat.cn_uid, cat.intl_uid)
        out.append(record)
    return out


def _cb_rows(sessions: list[dict], names: dict[str, str]) -> list[dict]:
    out = []
    for session in sessions:
        record = {
            "kind": "codebuddy",
            "source_key": session["platform"],
            "source_label": cb.PLATFORM_LABEL.get(session["platform"], session["platform"]),
            "account": session["uid"],
            "id": session["id"],
            "title": session["name"] or "（未命名会话）",
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
        }
        record["account_name"] = _account_name(record, names, "", "")
        out.append(record)
    return out


def _other_label(key: str) -> str:
    from . import qoder, trae, zcode
    if key in qoder.EDITIONS:
        return qoder.label(key)
    if key == "zcode":
        return zcode.LABEL
    for folder, label in trae.APPS:
        if key == folder:
            return label
    return key


def _fill_catalog(cat: Catalog, rows: list[dict]) -> None:
    cat.unified = rows
    cat.cb_sessions = [r["raw"] for r in rows if r["kind"] == "codebuddy" and r.get("raw")]
    cat.others = [r for r in rows if r["kind"] in ("qoder", "zcode")]
    cat.rows = list({r["id"]: r for r in rows if r["kind"] == "workbuddy"}.values())
    cat.cn_total = sum(1 for r in rows if r["source_key"] == "cn")
    cat.intl_total = sum(1 for r in rows if r["source_key"] == "intl")


def build_catalog(log=None) -> Catalog:
    """增量扫描：源指纹没变的来源整源复用索引，变了才重扫并落回索引。"""
    log = log or (lambda m, level="info": None)
    index = indexdb.load()
    names = account_names(log)
    cat = Catalog()
    cat.cn_uid = cat.cn.current_uid()[0]
    cat.intl_uid = cat.intl.current_uid()[0]
    rows: list[dict] = []

    # WorkBuddy：指纹带 uid，换号后归属判定会变，必须重算
    tasks.report(0.1, "扫描 WorkBuddy 会话")
    for ep, key, uid in ((cat.cn, "cn", cat.cn_uid), (cat.intl, "intl", cat.intl_uid)):
        fp = f"{wbdb.root_fp(ep)}|uid:{uid}"
        if index.hit(key, fp):
            rows.extend(index.source_rows(key))
            continue
        built = _wb_rows(ep, key, uid, names, cat, index, cat.errors)
        index.store(key, SOURCE_LABELS[key], fp, built)
        rows.extend(built)

    # CodeBuddy 两个平台
    tasks.report(0.5, "扫描 CodeBuddy 会话")
    cb_fp = {platform: cb.root_fp(platform) for platform in cb.PLATFORMS}
    if all(index.hit(platform, cb_fp[platform]) for platform in cb.PLATFORMS):
        for platform in cb.PLATFORMS:
            rows.extend(index.source_rows(platform))
    else:
        grouped: dict[str, list[dict]] = {}
        for session in cb.scan():
            grouped.setdefault(session["platform"], []).append(session)
        for platform in cb.PLATFORMS:
            built = _cb_rows(grouped.get(platform, []), names)
            index.store(platform, cb.PLATFORM_LABEL[platform], cb_fp[platform], built)
            rows.extend(built)

    # 其余软件（Qoder / ZCode / Trae）
    tasks.report(0.8, "扫描 Qoder / ZCode / Trae 会话")
    other_fp = others.root_fps()
    if all(index.hit(key, fp) for key, fp in other_fp.items()):
        for key in other_fp:
            rows.extend(index.source_rows(key))
    else:
        grouped = {}
        for row in others.scan():
            row["account_name"] = _account_name(row, names, cat.cn_uid, cat.intl_uid)
            grouped.setdefault(row["source_key"], []).append(row)
        for key, fp in other_fp.items():
            built = grouped.get(key, [])
            index.store(key, _other_label(key), fp, built)
            rows.extend(built)

    known = set(other_fp) | set(cb.PLATFORMS) | {"cn", "intl"}
    for key in list(index.fps):
        if key not in known:
            index.drop_source(key)

    rows.sort(key=lambda r: r["last"], reverse=True)
    _fill_catalog(cat, rows)
    index.flush()
    return cat


def cached_rows() -> list[dict]:
    """索引快照：首屏先上屏用，不碰任何客户端存储。"""
    index = indexdb.load()
    rows = [row for group in index.rows.values() for row in group]
    rows.sort(key=lambda r: r["last"], reverse=True)
    return rows


def catalog_from_rows(rows: list[dict]) -> Catalog:
    cat = Catalog()
    _fill_catalog(cat, list(rows))
    return cat


# ------------------------------------------------------------------ 写入动作
def delete_ids(ids: list[str], log) -> dict:
    """删除 WorkBuddy 会话：数据库行 + 正文 + 任务/待办，文件先进回收站。"""
    if not ids:
        return {"deleted": 0, "files": 0, "bytes": 0, "trash": "", "ids": []}
    with lifecycle.client_guard(lifecycle.workbuddy_clients(), log):
        stats = wbdb.delete_sessions(ids)
    indexdb.drop_sources(["cn", "intl"])
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
        indexdb.drop_sources({str(s.get("platform") or "") for s in sessions})
        log(f"🗑 CodeBuddy 会话 {stats['sessions']} 条，"
            f"释放 {stats['bytes'] / 1048576:.1f} MB")
    return stats


def delete_others(rows: list[dict], log) -> dict:
    """删除 Qoder / ZCode 会话：会话行导出到回收站后再从库里移除。"""
    stats = others.delete(rows, log)
    indexdb.drop_sources({row["source_key"] for row in rows})
    return stats


def sync_rows(rows: list[dict], target: str, log) -> dict:
    """同软件复制：Qoder 国内版 ↔ 国际版、CodeBuddy 账号之间。

    跨软件的格式转换不做——两边的正文结构和索引规则没有稳定对应关系，
    硬转出来的会话客户端未必认，反而把原库搅乱。
    """
    if not rows:
        return {"copied": 0}
    kind = rows[0]["kind"]
    if kind == "qoder":
        from . import qoder
        stats = qoder.copy_to(target, rows, log)
        indexdb.drop_sources(list(qoder.EDITIONS))
        return stats
    if kind == "codebuddy":
        stats = cb.copy_to(target, [row["raw"] for row in rows], log)
        indexdb.drop_sources({row["source_key"] for row in rows})
        return stats
    raise RuntimeError("这类会话没有可复制的本地数据")


def backup_now(log) -> str:
    """手动整库快照：回滚之前得先有东西可回。"""
    tag = trash.new_tag(backup.BACKUP_ROOT)
    log(f"正在备份会话库（{tag}）…")
    path = backup.make_snapshot(tag)
    log(f"备份完成：{path}")
    return str(path)


def cleanup_codebuddy(mode: str) -> dict:
    stats = cb.cleanup_empty(mode)
    indexdb.drop_sources(list(cb.PLATFORMS))
    return stats


def rollback(tag: str) -> None:
    backup.rollback(tag)
    indexdb.clear()


def restore_trash(tag: str, log) -> int:
    from . import qoder, zcode
    clients = (lifecycle.workbuddy_clients()
               + [qoder.client(key) for key in qoder.EDITIONS]
               + [zcode.client()])
    with lifecycle.client_guard(clients, log):
        restored = trash.restore_trash(tag)
    indexdb.clear()
    return restored


def purge_trash(tag: str | None = None) -> int:
    return trash.purge_trash(tag)


def clear_blacklist() -> int:
    return trash.clear_tombstones()


def save_profile(ep: Edition) -> Path:
    return profiles.save_current(ep)
