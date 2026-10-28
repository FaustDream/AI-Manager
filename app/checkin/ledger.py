# -*- coding: utf-8 -*-
"""签到积分台账：记录本工具成功领取的批次，用于推算到期日与快到期积分。

平台接口不返回历史批次，所以这里只统计「本工具亲眼看着发放」的积分。
批次带有效期时才计入到期统计；没有有效期的批次只保留金额。
"""
from __future__ import annotations

import json
from datetime import datetime, timedelta, timezone

from ..core import paths
from .base import CreditLot

SOON_DAYS = 7


def _load() -> dict:
    try:
        data = json.loads(paths.LEDGER_FILE.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return data if isinstance(data, dict) else {}


def _save(data: dict) -> None:
    try:
        paths.LEDGER_FILE.parent.mkdir(parents=True, exist_ok=True)
        paths.LEDGER_FILE.write_text(json.dumps(data, ensure_ascii=False, indent=2),
                                     encoding="utf-8")
    except OSError:
        pass


def _expiry(lot: dict) -> datetime | None:
    value = lot.get("expires")
    if not value:
        return None
    try:
        return datetime.fromisoformat(value)
    except ValueError:
        return None


def _prune(lots: list, now: datetime) -> list:
    keep = []
    for lot in lots:
        if not isinstance(lot, dict):
            continue
        expires = _expiry(lot)
        if expires and expires <= now:
            continue
        keep.append(lot)
    return keep


def add(iid: str, amount, validity_days=None, key: str | None = None) -> bool:
    """记录一笔新领取；key 相同的批次不会重复记录。"""
    try:
        value = float(amount)
    except (TypeError, ValueError):
        return False
    if value <= 0:
        return False

    now = datetime.now(timezone.utc)
    data = _load()
    lots = _prune(data.get(iid, []), now)
    if key and any(lot.get("key") == key for lot in lots):
        data[iid] = lots
        _save(data)
        return False
    expires = None
    if validity_days:
        expires = (now + timedelta(days=int(validity_days))).isoformat()
    lots.append({
        "amount": value,
        "granted": now.isoformat(),
        "expires": expires,
        "key": key or "",
    })
    data[iid] = lots
    _save(data)
    return True


def _summarize(lots: list, now: datetime, soon_days: int) -> tuple[str, float, int]:
    dated = [(lot, _expiry(lot)) for lot in lots]
    dated = [(lot, exp) for lot, exp in dated if exp]
    if not dated:
        return "", 0.0, len(lots)

    nearest = min(exp for _, exp in dated).astimezone()
    soon = 0.0
    for lot, exp in dated:
        if exp - now <= timedelta(days=soon_days):
            soon += float(lot.get("amount") or 0)
    return nearest.strftime("%Y-%m-%d"), soon, len(lots)


def summary(iid: str, soon_days: int = SOON_DAYS) -> tuple[str, float, int]:
    """返回 (最近到期日期, 快到期积分, 批次数)。"""
    now = datetime.now(timezone.utc)
    data = _load()
    raw = data.get(iid, [])
    lots = _prune(raw, now)
    if len(lots) != len(raw):
        data[iid] = lots
        _save(data)
    return _summarize(lots, now, soon_days)


def summaries(soon_days: int = SOON_DAYS) -> dict[str, tuple[str, float, int]]:
    """一次读完整个台账，返回 {iid: (最近到期, 快到期积分, 批次数)}。

    界面逐行调用 summary() 会重复读文件、重复 parse，行数一多就是 O(n²) 的磁盘读。
    """
    now = datetime.now(timezone.utc)
    data = _load()
    out = {}
    for iid, items in data.items():
        if not isinstance(items, list):
            continue
        out[iid] = _summarize(_prune(items, now), now, soon_days)
    return out


def soon_total(iids, soon_days: int = SOON_DAYS) -> float:
    return sum(summary(iid, soon_days)[1] for iid in iids)


def lots(iid: str) -> list[dict]:
    now = datetime.now(timezone.utc)
    return _prune(_load().get(iid, []), now)


def credit_lots(iid: str) -> list[CreditLot]:
    """台账批次转成 CreditLot，供 Qoder 这类不返回批次的接口补到期日。"""
    out = []
    for item in lots(iid):
        amount = float(item.get("amount") or 0)
        if amount <= 0:
            continue
        out.append(CreditLot(amount, _expiry(item), str(item.get("key") or "领取")))
    return out


def total_by_iid() -> dict:
    """一次性读出全部台账，避免界面逐行读文件。"""
    now = datetime.now(timezone.utc)
    return {iid: _prune(items, now) for iid, items in _load().items() if isinstance(items, list)}


def _granted(item: dict) -> datetime:
    """批次获得时刻；旧数据缺失或无时区时按 UTC 兜底，保证排序可比。"""
    try:
        at = datetime.fromisoformat(str(item.get("granted")))
    except (TypeError, ValueError):
        return datetime.min.replace(tzinfo=timezone.utc)
    return at if at.tzinfo else at.replace(tzinfo=timezone.utc)


def records(iid: str) -> list[dict]:
    """本工具记录的领取批次，最近的在前（含 granted / expires / amount）。"""
    return sorted(lots(iid), key=_granted, reverse=True)


def today_total(iid: str) -> float:
    """今日累计领取的积分。

    只统计本工具亲眼看到的发放：live_credit 平台（WorkBuddy / Trae）不写台账，
    这里返回 0 表示「无记录」，不等于今天没领过。
    """
    today = datetime.now(timezone.utc).astimezone().date()
    total = 0.0
    for item in lots(iid):
        if _granted(item).astimezone().date() == today:
            total += float(item.get("amount") or 0)
    return total
