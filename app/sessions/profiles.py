# -*- coding: utf-8 -*-
"""WorkBuddy 账号档案：把登录态与本机会话快照打包，一键换回。

登录态在 `<数据目录>/app/session`（Chromium 的 Cookies / Local Storage），
账号标识在 `storage/skeleton/account-snapshot.json`，会话在 workbuddy.db。
切换账号只换登录态与 storage，**不再替换 workbuddy.db**：会话由
sessions/restore.py 按 user_id 引用恢复，整库替换会让其他账号的会话消失。
档案里仍保留一份 workbuddy.db 快照，仅作为旧数据补回与回滚的依据。
"""
from __future__ import annotations

import json
import shutil
import sqlite3
from datetime import datetime
from pathlib import Path

from ..core import paths, winproc
from .editions import Edition, connect_ro, path_size
from .editions import remove_path

PACK_ROOT = paths.PROFILE_ROOT
STATE_DIRS = ("app/session", "storage")
DB_FILES = ("workbuddy.db", "workbuddy.db-wal", "workbuddy.db-shm")


def _pack_dir(ep: Edition, uid: str) -> Path:
    return PACK_ROOT / ep.key / uid


def _sessions_count(ep: Edition) -> int:
    """会话数只是档案的元信息，读不到时按 0 记，不能因此让整个保存失败。"""
    conn = connect_ro(ep.db)
    if conn is None:
        return 0
    try:
        return conn.execute(
            "SELECT COUNT(*) FROM sessions WHERE transport = 'local'").fetchone()[0]
    except sqlite3.Error:
        return 0
    finally:
        conn.close()


def sessions_count(ep: Edition) -> int:
    return _sessions_count(ep)


def current_uid(ep: Edition) -> str:
    try:
        data = json.loads((ep.root / "storage/skeleton/account-snapshot.json")
                          .read_text(encoding="utf-8"))
        return (data.get("primary") or {}).get("uid", "")
    except (OSError, ValueError):
        return ""


def list_packs(ep: Edition) -> list[dict]:
    base = PACK_ROOT / ep.key
    if not base.exists():
        return []
    out = []
    for entry in base.iterdir():
        # 自动保存写到一半的 <uid>.tmp 目录不能当成档案列出来
        if not entry.is_dir() or entry.name.endswith(".tmp"):
            continue
        meta = {}
        try:
            meta = json.loads((entry / "meta.json").read_text(encoding="utf-8"))
        except (OSError, ValueError):
            pass
        out.append({
            "uid": entry.name,
            "saved_at": meta.get("saved_at", ""),
            "sessions": meta.get("sessions", 0),
            "size": path_size(entry),
            "path": str(entry),
        })
    out.sort(key=lambda x: x["saved_at"], reverse=True)
    return out


def save_current(ep: Edition, uid: str = "") -> Path:
    """把当前登录态、会话库打包到 data/profiles/<版本>/<uid>/。

    自动保存会在切页面、退出时反复调用，所以先写 <uid>.tmp 再整体换名：
    拷贝中途失败（文件被占用、磁盘满）只会留下临时目录，不会毁掉已有档案。
    """
    uid = uid or current_uid(ep) or ep.current_uid()[0]
    if not uid:
        raise RuntimeError("读不到当前账号，客户端可能还没登录")
    dst = _pack_dir(ep, uid)
    tmp = dst.with_name(dst.name + ".tmp")
    if tmp.exists():
        remove_path(tmp)
    tmp.mkdir(parents=True, exist_ok=True)
    try:
        for rel in STATE_DIRS:
            src = ep.root / rel
            if src.exists():
                shutil.copytree(src, tmp / rel)
        for name in DB_FILES:
            src = ep.root / name
            if src.exists():
                shutil.copy2(src, tmp / name)
        meta = {
            "uid": uid,
            "label": ep.label,
            "saved_at": datetime.now().isoformat(timespec="seconds"),
            "sessions": _sessions_count(ep),
        }
        (tmp / "meta.json").write_text(json.dumps(meta, ensure_ascii=False, indent=2),
                                       encoding="utf-8")
    except Exception:
        remove_path(tmp)
        raise
    if dst.exists():
        remove_path(dst)
    tmp.rename(dst)
    return dst


def has_pack(ep: Edition, uid: str) -> bool:
    return bool(uid) and _pack_dir(ep, uid).exists()


def list_pack_uids(ep: Edition) -> set[str]:
    """只返回有档案的 uid，不统计体积。

    账号中心每刷一次都要判断「这个账号有没有登录态档案」，而 list_packs()
    会遍历几十 MB 的文件算大小，放在首屏会明显拖慢档案列的填充。
    """
    base = PACK_ROOT / ep.key
    if not base.exists():
        return set()
    return {entry.name for entry in base.iterdir()
            if entry.is_dir() and not entry.name.endswith(".tmp")
            and (entry / "meta.json").exists()}


def restore_pack(ep: Edition, uid: str) -> None:
    """把指定账号的档案恢复到客户端数据目录（登录态 + storage）。

    与 switch() 不同：不负责保存当前账号，调用方自行保证已存档后再调用。
    会话库不在这里替换——见 sessions/restore.py：按归属引用恢复，正文不搬。
    """
    if not has_pack(ep, uid):
        raise RuntimeError("找不到该账号的档案")
    _restore(ep, uid)


def _restore(ep: Edition, uid: str) -> None:
    src = _pack_dir(ep, uid)
    if not (src / "app/session").exists():
        raise RuntimeError("这个档案里没有登录态，无法免登录切换")
    # 登录态是全局的，必须整体替换
    session_src = src / "app/session"
    session_dst = ep.root / "app/session"
    if session_dst.exists():
        remove_path(session_dst)
    shutil.copytree(session_src, session_dst)
    # storage 里有每个账号自己的 user-<uid>-personal：合并而不是整体替换，
    # 否则会把其他账号的本地数据一起删掉（旧实现就是这么丢的）
    storage_src = src / "storage"
    if storage_src.exists():
        storage_dst = ep.root / "storage"
        storage_dst.mkdir(parents=True, exist_ok=True)
        shutil.copytree(storage_src, storage_dst, dirs_exist_ok=True)
    # workbuddy.db 保持原样：会话由 restore 模块按 user_id 引用恢复，
    # 整库替换会让其他账号的会话从当前库里消失


def switch(ep: Edition, uid: str) -> tuple[bool, str]:
    """切到指定账号：先保存当前账号，再换入目标档案（调用方负责先关客户端）。"""
    if winproc.process_running(ep.proc):
        return False, "客户端还在运行，先关闭后再切换"
    if uid == current_uid(ep):
        return False, "已经是这个账号了"
    if not _pack_dir(ep, uid).exists():
        return False, "找不到该账号的档案"
    save_current(ep)
    _restore(ep, uid)
    return True, f"已切换到 {uid[:8]}"


def logout_and_restart(ep: Edition) -> tuple[bool, str]:
    """保存当前账号后清空登录态，让客户端回到登录页（用于登录另一个账号）。"""
    if winproc.process_running(ep.proc):
        return False, "客户端还在运行，先关闭后再试"
    save_current(ep)
    session = ep.root / "app/session"
    if session.exists():
        remove_path(session)
    snapshot = ep.root / "storage/skeleton/account-snapshot.json"
    if snapshot.exists():
        remove_path(snapshot)
    winproc.start_exe(ep.exe)
    return True, "已退出登录，客户端会停在登录页"


def delete_pack(ep: Edition, uid: str) -> bool:
    path = _pack_dir(ep, uid)
    if not path.exists():
        return False
    return remove_path(path)


def ensure_closed(ep: Edition) -> bool:
    if not winproc.process_running(ep.proc):
        return True
    return winproc.kill_process(ep.proc)


def close_app(ep: Edition) -> bool:
    return winproc.kill_process(ep.proc)


def start_app(ep: Edition) -> bool:
    if not ep.exe.exists():
        return False
    return winproc.start_exe(ep.exe)


def pack_total_size(ep: Edition) -> int:
    base = PACK_ROOT / ep.key
    return path_size(base) if base.exists() else 0
