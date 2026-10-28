# -*- coding: utf-8 -*-
"""会话正文预览：把两种客户端的会话文件读成人能看的对话。

列表只关心标题与时间，正文各有各的格式：WorkBuddy 是 jsonl 事件流
（type=message + content 片段），CodeBuddy 是一条消息一个 json 文件。
格式知识集中在这里，编排层与界面都不用关心。

只读、只取头尾：一条会话可能有几十 MB，全量读进内存会把界面拖住。
按「消息条数」而不是字节数截取——单条工具结果动辄几十 KB，按字节切
经常只切到两三条。
"""
from __future__ import annotations

import json
import re
from pathlib import Path

HEAD_MESSAGES = 8
TAIL_MESSAGES = 16
TAIL_BYTES = 512 * 1024
CB_SIDE_FILES = 40
MAX_MESSAGES = 24
MAX_CHARS = 2000
TEXT_KINDS = ("text", "input_text", "output_text")

# 工具注入的上下文块会整段塞进首条用户消息，展示时要去掉
_SYSTEM_BLOCK = re.compile(r"<system-reminder\b.*?</system-reminder>", re.S | re.I)
_USER_INFO = re.compile(r"<user_info>.*?</user_info>", re.S | re.I)
_PROJECT_CONTEXT = re.compile(r"<project_context>.*?</project_context>", re.S | re.I)
_USER_TAGS = re.compile(r"</?user_query>", re.I)


def _clean(text: str) -> str:
    text = _SYSTEM_BLOCK.sub("", text)
    text = _USER_INFO.sub("", text)
    text = _PROJECT_CONTEXT.sub("", text)
    text = _USER_TAGS.sub("", text)
    return text.strip()


def _clip(text: str, limit: int = MAX_CHARS) -> str:
    text = text.strip()
    return text if len(text) <= limit else text[:limit].rstrip() + " …"


def _join(content) -> str:
    """content 可能是字符串，也可能是 [{type: text, text: ...}] 片段列表。"""
    if isinstance(content, str):
        return content
    if not isinstance(content, list):
        return ""
    parts = []
    for part in content:
        if isinstance(part, str):
            parts.append(part)
        elif isinstance(part, dict) and part.get("type") in TEXT_KINDS:
            text = part.get("text")
            if text:
                parts.append(str(text))
    return "\n".join(parts)


def _thin(messages: list[dict], limit: int) -> tuple[list[dict], int]:
    """太长时保留开头和结尾，中间用一条占位说明。"""
    if len(messages) <= limit:
        return messages, 0
    keep_head = max(2, limit // 3)
    keep_tail = limit - keep_head
    dropped = len(messages) - keep_head - keep_tail
    thinned = messages[:keep_head]
    thinned.append({"role": "gap", "text": f"中间省略 {dropped} 条"})
    thinned.extend(messages[-keep_tail:])
    return thinned, dropped


def _result(messages: list[dict], truncated: bool, note: str = "") -> dict:
    shown, dropped = _thin(messages, MAX_MESSAGES)
    if truncated and not note:
        note = "文件较大，只读取了开头与结尾"
    return {"messages": shown, "total": len(messages), "dropped": dropped,
            "truncated": truncated, "note": note}


# ------------------------------------------------------------------ WorkBuddy
def _parse_line(line: bytes) -> dict | None:
    if b'"message"' not in line:
        return None
    try:
        obj = json.loads(line)
    except ValueError:
        return None
    if obj.get("type") != "message":
        return None
    role = obj.get("role")
    if role not in ("user", "assistant"):
        return None
    text = _clean(_join(obj.get("content")))
    if not text:
        return None
    return {"role": role, "text": _clip(text), "id": str(obj.get("id") or ""),
            "time": int(obj.get("timestamp") or 0)}


def _head_messages(path: Path, limit: int) -> list[dict]:
    messages: list[dict] = []
    try:
        with path.open("rb") as fh:
            for line in fh:
                message = _parse_line(line)
                if message is not None:
                    messages.append(message)
                    if len(messages) >= limit:
                        break
    except OSError:
        return messages
    return messages


def _tail_lines(path: Path) -> list[bytes]:
    size = path.stat().st_size
    with path.open("rb") as fh:
        if size > TAIL_BYTES:
            fh.seek(size - TAIL_BYTES)
            fh.readline()            # 第一行从中间开始，丢掉
        return fh.read().splitlines()


def load_workbuddy(files) -> dict:
    """读 WorkBuddy 的 <sid>.jsonl：只认 type=message 的 user / assistant。"""
    paths = [Path(p) for p in files or []]
    target = next((p for p in paths if p.suffix == ".jsonl" and p.is_file()), None)
    if target is None:
        return _result([], False, "没有正文文件")
    try:
        size = target.stat().st_size
        if size <= TAIL_BYTES:
            messages = [m for m in (_parse_line(line) for line in target.read_bytes().splitlines())
                        if m is not None]
            return _result(messages, False)
        head = _head_messages(target, HEAD_MESSAGES)
        tail = [m for m in (_parse_line(line) for line in _tail_lines(target))
                if m is not None][-TAIL_MESSAGES:]
    except OSError as exc:
        return _result([], False, f"读取失败：{exc}")

    seen = {m["id"] for m in head if m["id"]}
    merged = head + [m for m in tail if not (m["id"] and m["id"] in seen)]
    return _result(merged, True)


# ------------------------------------------------------------------ CodeBuddy
def _parse_codebuddy(file: Path) -> dict | None:
    try:
        record = json.loads(file.read_text(encoding="utf-8", errors="replace"))
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
    content = body.get("content") if isinstance(body, dict) else body
    text = _clean(_join(content))
    if not text:
        return None
    return {"role": role, "text": _clip(text), "id": str(record.get("id") or ""),
            "time": 0}


def load_codebuddy(path: str) -> dict:
    """读 CodeBuddy 的会话目录：messages/<id>.json，按写入时间排序。"""
    folder = Path(path) / "messages" if path else None
    if folder is None or not folder.is_dir():
        return _result([], False, "没有正文目录")
    try:
        files = sorted((p for p in folder.glob("*.json") if p.is_file()),
                       key=lambda p: p.stat().st_mtime)
    except OSError as exc:
        return _result([], False, f"读取失败：{exc}")

    truncated = len(files) > CB_SIDE_FILES * 2
    if len(files) > CB_SIDE_FILES * 2:
        files = files[:CB_SIDE_FILES] + files[-CB_SIDE_FILES:]

    messages = [m for m in (_parse_codebuddy(f) for f in files) if m is not None]
    return _result(messages, truncated)


def load(row: dict) -> dict:
    """按行类型分派：界面只拿 row，不关心底下是哪种格式。"""
    if row.get("kind") == "codebuddy":
        return load_codebuddy(row.get("path") or "")
    return load_workbuddy(row.get("files") or [])
