# -*- coding: utf-8 -*-
"""CodeBuddy IDE / JetBrains 插件的历史会话读写。

本机真实落盘位置（按账号分目录，这也就是“会话属于哪个账号”的来源）：
    %LOCALAPPDATA%\\CodeBuddyExtension\\Data\\<账号uid>\\<CodeBuddyIDE|JetBrains>\\<账号uid>\\history\\<工作区hash>\\
        index.json              {"conversations":[{id,name,createdAt,lastMessageAt,...}], "current": id}
        current.json            {"conversationId":..., "lastUpdated":...}
        <conversationId>\\
            index.json          {"messages":[{id,type,role,isComplete},...]}
            messages\\<msgId>.json  {"role":..., "message":"<内层JSON字符串>"}

两个插件的结构完全一致，各账号、各平台目录互相独立；WorkBuddy 用的是 SQLite，
格式不同，两边各管各的。
"""
from __future__ import annotations

import json
import os
import re
import shutil
import time
from datetime import datetime, timezone
from pathlib import Path

from ..core import paths

CB_ROOT = paths.user_local_dir() / "CodeBuddyExtension" / "Data"
PLATFORMS = ("CodeBuddyIDE", "JetBrains")
PLATFORM_LABEL = {"CodeBuddyIDE": "CodeBuddy IDE", "JetBrains": "IDEA 插件"}
UID_RE = re.compile(r"^(?:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-"
                    r"[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|default)$")

# 全量扫描要读几百个工作区索引，两秒起步；会话页和备份页共用这一份结果
_CACHE: tuple[float, list[dict]] | None = None
_CACHE_TTL = 60.0


def invalidate() -> None:
    """删改之后必须让缓存失效，否则刷新还会看到刚删掉的会话。"""
    global _CACHE
    _CACHE = None



def _valid_uid(name: str) -> bool:
    return bool(UID_RE.match(name))


def _iso_ms(value):
    if not value:
        return 0
    try:
        text = str(value).replace("Z", "+00:00")
        return int(datetime.fromisoformat(text).timestamp() * 1000)
    except ValueError:
        return 0


def _read_json(path: Path):
    try:
        return json.loads(path.read_text(encoding="utf-8", errors="replace"))
    except (OSError, ValueError):
        return None


def _write_json(path: Path, data):
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")
    os.replace(tmp, path)


def sources() -> list[tuple]:
    """(平台, 账号uid, history 目录) 三元组，已存在的才算。"""
    out = []
    if not CB_ROOT.exists():
        return out
    for uid_dir in sorted(CB_ROOT.iterdir()):
        if not uid_dir.is_dir():
            continue
        for platform in PLATFORMS:
            pdir = uid_dir / platform
            if not pdir.is_dir():
                continue
            for acc_dir in sorted(pdir.iterdir()):
                if not _valid_uid(acc_dir.name):
                    continue
                hist = acc_dir / "history"
                if hist.is_dir():
                    out.append((platform, acc_dir.name, hist))
    return out


def scan(force: bool = False) -> list[dict]:
    """扫描全部平台/账号/工作区，返回会话列表（结果缓存 60 秒）。"""
    global _CACHE
    now = time.monotonic()
    if not force and _CACHE is not None and now - _CACHE[0] < _CACHE_TTL:
        return [dict(item) for item in _CACHE[1]]

    sessions = []
    for platform, uid, hist in sources():
        try:
            workspace_dirs = sorted(hist.iterdir())
        except OSError:
            continue
        for ws_dir in workspace_dirs:
            if not ws_dir.is_dir():
                continue
            index = _read_json(ws_dir / "index.json")
            if not index:
                continue
            for conv in index.get("conversations") or []:
                cid = conv.get("id")
                if not cid:
                    continue
                cdir = ws_dir / cid
                msg_dir = cdir / "messages"
                if msg_dir.is_dir():
                    files = [f for f in msg_dir.iterdir() if f.is_file()]
                    msgs = len(files)
                    size = sum(f.stat().st_size for f in files)
                else:
                    msgs, size = 0, 0
                sessions.append({
                    "platform": platform,
                    "uid": uid,
                    "workspace": ws_dir.name,
                    "id": cid,
                    "name": conv.get("name") or "",
                    "created": _iso_ms(conv.get("createdAt")),
                    "last": _iso_ms(conv.get("lastMessageAt") or conv.get("createdAt")),
                    "msgs": msgs,
                    "size": size,
                    "path": str(cdir),
                    "meta": conv,
                    "has_body": msgs > 0,
                })
    sessions.sort(key=lambda s: s["last"], reverse=True)
    _CACHE = (now, sessions)
    return [dict(item) for item in sessions]


def _target_ws(platform, uid, workspace) -> Path:
    return CB_ROOT / uid / platform / uid / "history" / workspace


def _drop_recent(ws_dir: Path, cid: str) -> None:
    """recent.json 里也留着这条会话，不清掉客户端列表还会显示它。"""
    recent_path = ws_dir / "recent.json"
    recent = _read_json(recent_path)
    if not isinstance(recent, list):
        return
    kept = [x for x in recent
            if (x.get("id") if isinstance(x, dict) else x) != cid]
    if len(kept) != len(recent):
        _write_json(recent_path, kept)


def remove_session(session: dict, tag: str) -> dict | None:
    """把一条会话从工作区索引摘掉，目录移入回收站；返回回收站清单条目。"""
    cid = session.get("id") or ""
    if not cid:
        return None
    platform = session.get("platform") or ""
    uid = session.get("uid") or ""
    workspace = session.get("workspace") or ""
    cdir = Path(session.get("path") or "")
    ws_dir = cdir.parent if str(cdir) else None

    size = 0
    rel = Path("cb") / platform / uid / workspace / cid
    dest = paths.TRASH_ROOT / tag / rel
    if cdir.is_dir():
        try:
            size = sum(f.stat().st_size for f in cdir.rglob("*") if f.is_file())
        except OSError:
            size = 0
        dest.parent.mkdir(parents=True, exist_ok=True)
        if dest.exists():
            shutil.rmtree(dest)
        shutil.move(str(cdir), str(dest))

    if ws_dir is not None:
        index_path = ws_dir / "index.json"
        if index_path.exists():
            index = _read_json(index_path) or {}
            convs = [c for c in (index.get("conversations") or [])
                     if c.get("id") != cid]
            index["conversations"] = convs
            if index.get("current") == cid:
                index["current"] = convs[0]["id"] if convs else None
            _write_json(index_path, index)
            _drop_recent(ws_dir, cid)

    invalidate()
    return {"kind": "codebuddy", "sid": cid, "edition": "cb",
            "platform": platform, "uid": uid, "workspace": workspace,
            "conv": session.get("meta") or {"id": cid, "name": session.get("name") or ""},
            "trash_rel": str(rel), "size": size,
            "files": [], "row": None, "usage": None}


def restore_item(item: dict, tag_dir: Path) -> bool:
    """回收站恢复：目录搬回原位，并重新登记到工作区索引。"""
    platform = item.get("platform") or ""
    uid = item.get("uid") or ""
    workspace = item.get("workspace") or ""
    cid = item.get("sid") or ""
    if not (platform and uid and cid):
        return False
    src = tag_dir / str(item.get("trash_rel") or "")
    ws_dir = _target_ws(platform, uid, workspace)
    ws_dir.mkdir(parents=True, exist_ok=True)
    if src.is_dir():
        dest = ws_dir / cid
        if dest.exists():
            shutil.rmtree(dest)
        shutil.move(str(src), str(dest))

    index_path = ws_dir / "index.json"
    index = _read_json(index_path) or {"conversations": [], "current": None}
    convs = index.setdefault("conversations", [])
    conv = item.get("conv") or {"id": cid}
    if not any(c.get("id") == cid for c in convs):
        convs.append(conv)
    if not index.get("current"):
        index["current"] = cid
    _write_json(index_path, index)
    invalidate()
    return True



def cleanup_empty(mode="junk") -> dict:
    """永久清掉空壳会话，并删除清空后剩下的空工作区。

    mode:
        "nobody"  — 无正文的，全部删
        "unnamed" — 没有标题的，全部删
        "junk"    — 两个条件命中任一即删（只保留既有标题又有正文的）
    返回 {"sessions":n, "workspaces":n, "bytes":n, "failed":n}。
    """
    stats = {"sessions": 0, "workspaces": 0, "bytes": 0, "failed": 0}
    for _platform, _uid, hist in sources():
        try:
            workspace_dirs = list(hist.iterdir())
        except OSError:
            continue
        for ws_dir in workspace_dirs:
            if not ws_dir.is_dir():
                continue
            try:
                index_path = ws_dir / "index.json"
                index = _read_json(index_path)
                if not index:
                    continue
                if not (index.get("conversations") or []):
                    stats["workspaces"] += 1
                    stats["bytes"] += sum(f.stat().st_size
                                          for f in ws_dir.rglob("*") if f.is_file())
                    shutil.rmtree(ws_dir)
                    continue
                keep = []
                removed_ids = []
                for conv in index.get("conversations") or []:
                    cid = conv.get("id")
                    cdir = ws_dir / cid if cid else None
                    msg_dir = cdir / "messages" if cdir else None
                    has_body = bool(msg_dir and msg_dir.is_dir()
                                    and any(f.is_file() for f in msg_dir.iterdir()))
                    named = bool((conv.get("name") or "").strip())
                    drop = {
                        "nobody": not has_body,
                        "unnamed": not named,
                        "junk": not (named and has_body),
                    }[mode]
                    if cid and drop:
                        if cdir and cdir.exists():
                            stats["bytes"] += sum(
                                f.stat().st_size for f in cdir.rglob("*") if f.is_file())
                            shutil.rmtree(cdir)
                        removed_ids.append(cid)
                        stats["sessions"] += 1
                    else:
                        keep.append(conv)
                if not removed_ids:
                    continue
                index["conversations"] = keep
                if index.get("current") in removed_ids:
                    index["current"] = keep[0]["id"] if keep else None
                _write_json(index_path, index)

                recent_path = ws_dir / "recent.json"
                recent = _read_json(recent_path)
                if recent is not None and isinstance(recent, list):
                    filtered = [x for x in recent
                                if (x.get("id") if isinstance(x, dict) else x) not in removed_ids]
                    if len(filtered) != len(recent):
                        _write_json(recent_path, filtered)

                if not keep:
                    stats["workspaces"] += 1
                    shutil.rmtree(ws_dir)
                else:
                    current_path = ws_dir / "current.json"
                    if current_path.exists() and index.get("current"):
                        _write_json(current_path, {
                            "conversationId": index["current"],
                            "lastUpdated": datetime.now(timezone.utc)
                            .isoformat(timespec="milliseconds")
                            .replace("+00:00", "Z")})
            except OSError:
                # IDE 正在写索引时可能短暂占用文件，跳过这个工作区，继续处理下一个
                stats["failed"] += 1
    if stats["sessions"] or stats["workspaces"]:
        invalidate()
    return stats
