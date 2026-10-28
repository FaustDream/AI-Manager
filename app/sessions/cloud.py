# -*- coding: utf-8 -*-
"""云端会话列表拉取：与客户端相同的接口 GET /v2/as/conversations/。

客户端登录另一个账号后，云端会话要等它自己同步才会出现在本地库里；
这里主动拉一份服务端列表，让「会话管理」不必等客户端就能看到全貌。

字段映射（服务端 → 本地会话行）：
    id / userId / name / status / conversationOrigin / isPlayground
    createdAt / updatedAt / lastActivityAt（秒或毫秒或 ISO 均兼容）

只读接口，不写任何服务端数据；失败静默降级为「云端列表不可用」。
"""
from __future__ import annotations

import json
from datetime import datetime

import requests

API_BASES = {
    "cn": "https://copilot.tencent.com",
    "intl": "https://www.workbuddy.ai",
}

PAGE_SIZE = 50
MAX_PAGES = 20          # 单账号最多拉 1000 条，防止会话特别多时卡住界面


def _to_ms(value) -> int:
    """服务端时间戳可能是秒/毫秒数字或 ISO 字符串，统一转毫秒。"""
    if value in (None, ""):
        return 0
    if isinstance(value, (int, float)):
        number = int(value)
        return number if number > 10 ** 12 else number * 1000
    try:
        text = str(value).replace("Z", "+00:00")
        return int(datetime.fromisoformat(text).timestamp() * 1000)
    except ValueError:
        return 0


def _normalize(record: dict, uid: str) -> dict:
    """把云端会话记录标准化成与本地会话行兼容的字段集。"""
    return {
        "id": str(record.get("id") or record.get("sessionId") or ""),
        "user_id": str(record.get("userId") or uid),
        "title": record.get("name") or record.get("title") or "",
        "status": record.get("status") or record.get("sessionStatus") or "",
        "conversation_origin": record.get("conversationOrigin") or "",
        "visibility": record.get("visibility") or "",
        "is_playground": bool(record.get("isPlayground")),
        "created_at": _to_ms(record.get("createdAt")),
        "updated_at": _to_ms(record.get("updatedAt")),
        "last_activity_at": _to_ms(record.get("lastActivityAt") or record.get("updatedAt")),
        "transport": "cloud",
    }


def fetch_conversations(account, log, max_pages: int = MAX_PAGES) -> list[dict]:
    """用某个账号的登录态分页拉取云端会话列表，返回标准化行。"""
    meta = getattr(account, "meta", {}) or {}
    uid = str(meta.get("uid") or "")
    edition = "intl" if (meta.get("edition") == "intl"
                         or str(meta.get("domain") or "").endswith(".ai")) else "cn"
    base = API_BASES[edition]
    headers = {
        "Authorization": f"Bearer {account.token}",
        "X-User-Id": uid,
        "X-Domain": meta.get("domain", ""),
        "Content-Type": "application/json",
    }
    sort = json.dumps({"order": "desc", "orderBy": "updatedAt"})
    rows: list[dict] = []
    total = None
    for page in range(1, max_pages + 1):
        resp = requests.get(
            f"{base}/v2/as/conversations/",
            params={"page": page, "size": PAGE_SIZE, "sort": sort},
            headers=headers, timeout=20)
        if resp.status_code in (401, 403):
            log(f"[云端会话] 登录态失效（HTTP {resp.status_code}），跳过云端拉取", "dim")
            return rows
        if resp.status_code >= 400:
            log(f"[云端会话] HTTP {resp.status_code}，跳过云端拉取", "dim")
            return rows
        try:
            body = resp.json()
        except ValueError:
            return rows
        data = body.get("data") or body
        if total is None:
            total = data.get("total")
        for record in data.get("conversations") or []:
            row = _normalize(record, uid)
            if row["id"]:
                rows.append(row)
        if not data.get("conversations"):
            break
        if isinstance(total, int) and len(rows) >= total:
            break
    return rows


def fetch_for_account(account, log) -> list[dict]:
    """对外入口：拉某个账号的云端会话（异常统一降级为空列表）。"""
    try:
        rows = fetch_conversations(account, log)
        if rows:
            log(f"[云端会话] 拉到 {len(rows)} 条", "dim")
        return rows
    except requests.RequestException as exc:
        log(f"[云端会话] 网络失败：{exc}", "dim")
        return []
    except Exception as exc:  # 任何解析异常都不应打断列表刷新
        log(f"[云端会话] 拉取失败：{exc}", "dim")
        return []


def find_account(uid: str):
    """从本机已存登录态里找 uid 对应的账号（拿 token 用），找不到返回 None。"""
    if not uid:
        return None
    try:
        from ..checkin import service as checkin_service
        accounts = checkin_service.collect({}, lambda m, lv="info": None)
    except Exception:
        return None
    for account in accounts:
        if getattr(account, "platform", "") != "workbuddy":
            continue
        meta = getattr(account, "meta", {}) or {}
        if str(meta.get("uid") or "") == uid:
            return account
    return None
