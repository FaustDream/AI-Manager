# -*- coding: utf-8 -*-
"""CodeBuddy IDE / JetBrains 插件的历史会话读写。

本机真实落盘位置（按账号分目录，这也就是“会话属于哪个账号”的来源）：
    %LOCALAPPDATA%\\CodeBuddyExtension\\Data\\<账号uid>\\<CodeBuddyIDE|JetBrains>\\<账号uid>\\history\\<工作区hash>\\
        index.json              {"conversations":[{id,name,createdAt,lastMessageAt,...}], "current": id}
        current.json            {"conversationId":..., "lastUpdated":...}
        <conversationId>\\
            index.json          {"messages":[{id,type,role,isComplete},...]}
            messages\\<msgId>.json  {"role":..., "message":"<内层JSON字符串>"}

两个插件的结构完全一致，所以可以在两边互相同步；WorkBuddy 用的是 SQLite，
格式不同，只能各管各的。
"""
from __future__ import annotations

import json
import os
import re
import shutil
from datetime import datetime, timezone
from pathlib import Path

CB_ROOT = Path(os.environ.get("LOCALAPPDATA", str(Path.home()))) / "CodeBuddyExtension" / "Data"
PLATFORMS = ("CodeBuddyIDE", "JetBrains")
PLATFORM_LABEL = {"CodeBuddyIDE": "CodeBuddy IDE", "JetBrains": "IDEA 插件"}
UID_RE = re.compile(r"^(?:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-"
                    r"[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|default)$")


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


def accounts() -> list[str]:
    """所有出现过的账号 uid（来自目录名）。"""
    if not CB_ROOT.exists():
        return []
    seen = []
    for uid_dir in sorted(CB_ROOT.iterdir()):
        if uid_dir.is_dir() and _valid_uid(uid_dir.name) and uid_dir.name not in seen:
            seen.append(uid_dir.name)
    return seen


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


def scan() -> list[dict]:
    """扫描全部平台/账号/工作区，返回会话列表。"""
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
    return sessions


def _target_ws(platform, uid, workspace) -> Path:
    return CB_ROOT / uid / platform / uid / "history" / workspace


def sync_session(session, dst_platform, dst_uid) -> tuple[bool, str]:
    """把一条会话复制到另一个平台/账号，返回 (ok, message)。"""
    if not session.get("has_body"):
        return False, "源会话没有正文（0 条消息），跳过"
    src_dir = Path(session["path"])
    if not src_dir.is_dir():
        return False, "源会话目录不存在"

    ws_dir = _target_ws(dst_platform, dst_uid, session["workspace"])
    ws_dir.mkdir(parents=True, exist_ok=True)
    index_path = ws_dir / "index.json"
    index = _read_json(index_path) or {"conversations": [], "current": None}
    index.setdefault("conversations", [])

    existing = next((c for c in index["conversations"]
                     if c.get("id") == session["id"]), None)
    if existing and _iso_ms(existing.get("lastMessageAt")) >= session["last"]:
        if (ws_dir / session["id"]).is_dir():
            return False, "目标已存在且不旧于源，跳过"

    if existing:
        index["conversations"] = [c for c in index["conversations"]
                                  if c.get("id") != session["id"]]
    index["conversations"].append(session["meta"])
    if not index.get("current"):
        index["current"] = session["id"]

    dst_conv = ws_dir / session["id"]
    if dst_conv.exists():
        shutil.rmtree(dst_conv)
    shutil.copytree(src_dir, dst_conv)
    _write_json(index_path, index)
    return True, f"已写入 {PLATFORM_LABEL.get(dst_platform, dst_platform)}"


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
    return stats


def targets() -> list[tuple]:
    """可选的同步目标（已有或可创建的平台×账号组合）。"""
    return [(platform, uid) for uid in accounts() for platform in PLATFORMS]


def ws_label(session) -> str:
    return session["workspace"][:8]
