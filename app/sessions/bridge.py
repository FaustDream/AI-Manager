# -*- coding: utf-8 -*-
"""把 CodeBuddy IDE / IDEA 插件的会话转成 WorkBuddy 能读的会话。

插件是「一条消息一个 json 文件」，WorkBuddy 是「一条 jsonl 事件流」，两边没有
对应关系，只能重建：保留 user / assistant 的文本，丢掉工具调用、推理过程与附件——
这些在两边的结构完全不同，硬塞进去只会让客户端解析失败。

导入是纯新增：生成新的会话行和新正文，插件那边一个字节都不动。会话 id 由插件
id 稳定映射成 uuid，所以重复导入同一条只会算「已存在」，不会出现两份。

插件的工作区目录名是哈希，还原不出真实路径，导入的会话统一放在
projects/codebuddy-import/ 下，cwd 留空——客户端里会集中显示成同一个项目。
"""
from __future__ import annotations

import json
import sqlite3
import time
import uuid
from datetime import datetime
from pathlib import Path

from . import indexdb, lifecycle
from .editions import Edition, connect_rw, table_columns

SLUG = "codebuddy-import"
TEXT_KINDS = ("text", "input_text", "output_text")
TITLE_MAX = 60


def _ms(value) -> int:
    """ISO 时间 → 毫秒时间戳；解析不了给 0，调用方按 0 处理。"""
    try:
        return int(datetime.fromisoformat(str(value).replace("Z", "+00:00")).timestamp() * 1000)
    except (TypeError, ValueError):
        return 0


def _uuid(value: str) -> str:
    """插件的 id → 标准 uuid：同一会话永远映射到同一个值，重复导入才幂等。"""
    text = str(value or "")
    try:
        return str(uuid.UUID(text))
    except (ValueError, AttributeError):
        return str(uuid.uuid5(uuid.NAMESPACE_OID, text or "unknown"))


def _text(content) -> str:
    if isinstance(content, str):
        return content
    if not isinstance(content, list):
        return ""
    return "\n".join(str(part.get("text") or "") for part in content
                     if isinstance(part, dict) and part.get("type") in TEXT_KINDS)


def _message(path: Path) -> dict | None:
    try:
        record = json.loads(path.read_text(encoding="utf-8", errors="replace"))
    except (OSError, ValueError):
        return None
    role = record.get("role")
    if role not in ("user", "assistant"):
        return None
    body = record.get("message")
    if isinstance(body, str):
        try:
            body = json.loads(body)
        except ValueError:
            body = {"content": body}
    text = _text(body.get("content") if isinstance(body, dict) else body).strip()
    if not text:
        return None
    return {"role": role, "text": text, "id": str(record.get("id") or ""),
            "ts": _ms(record.get("createdAt"))}


def load_messages(path: Path) -> list[dict]:
    """读一条插件会话的全部消息：优先按 index.json 的顺序。"""
    folder = path / "messages"
    if not folder.is_dir():
        return []
    order: list[str] = []
    try:
        index = json.loads((path / "index.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        index = None
    if isinstance(index, dict):
        order = [str(item.get("id")) for item in (index.get("messages") or [])
                 if isinstance(item, dict) and item.get("id")]
    files = [folder / f"{mid}.json" for mid in order]
    files = [item for item in files if item.is_file()]
    if not files:
        # 索引缺失或对不上时退回按写入时间排：至少顺序还是对话顺序
        try:
            files = sorted(folder.glob("*.json"), key=lambda item: item.stat().st_mtime)
        except OSError:
            return []
    return [msg for msg in (_message(item) for item in files) if msg is not None]


def _dump(obj: dict) -> str:
    return json.dumps(obj, ensure_ascii=False)


def build_lines(messages: list[dict], sid: str, title: str) -> list[str]:
    """拼出 WorkBuddy 的 jsonl：session-meta + 每轮 message + ai-title。"""
    first = messages[0]["ts"] or int(time.time() * 1000)
    lines = [_dump({"type": "session-meta", "id": str(uuid.uuid4()),
                    "sessionId": sid, "timestamp": str(first),
                    "meta": {"codebuddy.ai/hostKind": "unopted"}})]
    for msg in messages:
        mid = _uuid(msg["id"]) if msg["id"] else str(uuid.uuid4())
        ts = msg["ts"] or first
        lines.append(_dump({
            "id": mid, "timestamp": ts, "type": "message", "role": msg["role"],
            "content": [{"type": "text", "text": msg["text"]}],
            # 客户端的会话列表靠 startsNewUserRequest 切分请求边界，缺了会显示成一整块
            "providerData": {"startsNewUserRequest": msg["role"] == "user",
                             "agent": "cli",
                             "conversationRequestId": uuid.UUID(mid).hex},
            "sessionId": sid, "cwd": "",
        }))
    if title:
        lines.append(_dump({"id": str(uuid.uuid4()), "timestamp": first + 1,
                            "type": "ai-title", "aiTitle": title,
                            "sessionId": sid, "cwd": ""}))
    return lines


def import_sessions(sessions: list[dict], ep: Edition, uid: str, log,
                    allow_cross: bool = False) -> dict:
    """把选中的插件会话导入 ep，归属到 uid。跨软件转换默认禁用。

    跨软件的格式转换对会话数据的影响尚未验证，必须由调用方显式放行；
    万一将来验证出问题，关掉设置里的开关就能整体下线。
    """
    if not allow_cross:
        raise RuntimeError("跨软件导入未启用：格式转换对会话数据的影响尚未验证")
    client = lifecycle.Client(ep.proc, ep.label, ep.exe)
    with lifecycle.client_guard([client], log):
        stats = _import_locked(sessions, ep, uid, log)
    indexdb.drop_sources(["cn", "intl"])
    return stats


def _import_locked(sessions: list[dict], ep: Edition, uid: str, log) -> dict:
    """实际写入；客户端的关闭 / 重启由 import_sessions 的闸门负责。"""
    stats = {"imported": 0, "exists": 0, "skipped": 0, "failed": 0}
    if not sessions:
        return stats

    target = ep.projects / SLUG
    conn = connect_rw(ep.db)
    try:
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        cols = set(table_columns(conn, "sessions"))
        for session in sessions:
            if not str(session.get("id") or ""):
                stats["skipped"] += 1
                continue
            sid = _uuid(str(session.get("id")))
            if conn.execute("SELECT 1 FROM sessions WHERE id = ?", (sid,)).fetchone():
                stats["exists"] += 1
                continue
            messages = load_messages(Path(str(session.get("path") or "")))
            if not messages:
                stats["skipped"] += 1
                continue
            title = str(session.get("name") or "").strip()[:TITLE_MAX] or "导入的会话"
            try:
                target.mkdir(parents=True, exist_ok=True)
                body = build_lines(messages, sid, title)
                (target / f"{sid}.jsonl").write_text("\n".join(body) + "\n",
                                                     encoding="utf-8")
            except OSError as exc:
                log(f"写入会话正文失败：{exc}", "error")
                stats["failed"] += 1
                continue

            first = messages[0]["ts"] or int(time.time() * 1000)
            last = messages[-1]["ts"] or first
            row = {
                "id": sid, "cwd": "", "user_id": uid, "title": title,
                "custom_title": None, "status": "completed",
                "created_at": first, "updated_at": last, "last_activity_at": last,
                "deleted_at": None, "is_playground": 0,
                "source_mode": "craft", "mode": "craft",
                "permission_mode": "fullAccess", "use_sandbox_cli": 1,
                "context_window": 1000000, "thought_level": "max",
                "transport": "local", "unread": 0,
            }
            names = [name for name in row if name in cols]
            try:
                conn.execute(
                    f"INSERT OR IGNORE INTO sessions ({','.join(names)}) "
                    f"VALUES ({','.join('?' * len(names))})",
                    [row[name] for name in names])
            except sqlite3.Error as exc:
                log(f"写入会话行失败：{exc}", "error")
                stats["failed"] += 1
                continue
            stats["imported"] += 1
        conn.commit()
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    except Exception:
        conn.rollback()
        raise
    finally:
        conn.close()
    return stats
