# -*- coding: utf-8 -*-
"""provider 公共数据结构与接口定义。"""
from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Callable

LogFn = Callable[[str], None]


def parse_iso(value) -> datetime | None:
    """解析 ISO 时间和毫秒时间戳，统一返回 UTC aware datetime。"""
    if value is None or value == "":
        return None
    if isinstance(value, (int, float)):
        seconds = float(value) / 1000 if value > 1e11 else float(value)
        try:
            return datetime.fromtimestamp(seconds, tz=timezone.utc)
        except (OSError, OverflowError, ValueError):
            return None
    text = str(value).strip().replace("Z", "+00:00")
    try:
        dt = datetime.fromisoformat(text)
    except ValueError:
        return None
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt


@dataclass
class Account:
    platform: str
    platform_label: str
    account_id: str
    name: str
    token: str
    expires_at: datetime | None = None
    source: str = ""
    meta: dict = field(default_factory=dict)
    # 平台本身没有签到玩法（例如 WorkBuddy 国际版）时为 False。
    # 这类账号只查积分，不参与批量/自动签到，界面上也不显示“待签到”。
    requires_checkin: bool = True

    @property
    def iid(self) -> str:
        return f"{self.platform}:{self.account_id}"

    @property
    def expired(self) -> bool:
        if not self.expires_at:
            return False
        return self.expires_at <= datetime.now(timezone.utc)

    def expires_text(self) -> str:
        if not self.expires_at:
            return "未知"
        return self.expires_at.astimezone().strftime("%Y-%m-%d")

    def expires_short(self) -> str:
        """同年只显示月-日，界面更紧凑。"""
        if not self.expires_at:
            return "有效"
        local = self.expires_at.astimezone()
        text = local.strftime("%Y-%m-%d") if local.year != datetime.now().year \
            else local.strftime("%m-%d")
        return f"已过期 {text}" if self.expired else f"有效 {text}"


@dataclass
class CreditLot:
    amount: float
    expires_at: datetime | None = None
    label: str = ""


@dataclass
class StatusResult:
    checked_in: bool | None
    summary: str
    balance: float | None = None
    lots: list[CreditLot] | None = None
    # 下一次可签的本地时刻：只由平台接口下发，None 表示现在就能签或接口没给。
    # 不能用本地时钟推算，Qoder 的活动窗口是服务端排期、每天下发时间会漂。
    next_at: datetime | None = None


@dataclass
class CreditSnapshot:
    summary: str
    balance: float | None = None
    lots: list[CreditLot] | None = None


@dataclass
class ActionResult:
    ok: bool
    message: str
    granted: float | None = None
    validity_days: int | None = None
    # 本轮早就领过、这次没有任何新发放：调用不算失败，但不能当成“今天签过了”。
    # 例如 Qoder 08:30 去签，签的还是昨天 10:00 那一轮。
    already: bool = False


class ProviderError(RuntimeError):
    """接口调用失败。"""


class UnsupportedAccount(ProviderError):
    """账号类型暂时无法适配（例如国际版域名）。"""


def summarize_lots(lots, soon_days: int = 7) -> tuple[str, float, float, float]:
    """返回 (最近到期日期, 临期积分, 有到期日的积分总额, 最近到期日当天到期积分)。

    只统计带到期日的批次；没有到期日的额度（如 Qoder 资源包）不参与临期计算。
    """
    if not lots:
        return "", 0.0, 0.0, 0.0
    now = datetime.now(timezone.utc)
    dated = [(lot, lot.expires_at) for lot in lots if lot.expires_at]
    if not dated:
        return "", 0.0, 0.0, 0.0
    nearest_dt = min(exp for _, exp in dated).astimezone()
    nearest_day = nearest_dt.date()
    soon = sum(float(lot.amount or 0) for lot, exp in dated
               if exp - now <= timedelta(days=soon_days))
    total = sum(float(lot.amount or 0) for lot, _exp in dated)
    nearest_amount = sum(float(lot.amount or 0) for lot, exp in dated
                         if exp.astimezone().date() == nearest_day)
    return nearest_dt.strftime("%Y-%m-%d"), soon, total, nearest_amount


class Provider:
    key = ""
    label = ""
    live_credit = False
    # 下次可签时刻只认接口下发（活动制平台）：本地按自然日或固定时刻推算都不成立
    next_from_api = False

    def requires_checkin(self, account: Account) -> bool:
        return getattr(account, "requires_checkin", True)

    def scan(self, log: LogFn, extra_paths: tuple = ()) -> list[Account]:
        raise NotImplementedError

    def status(self, account: Account, log: LogFn) -> StatusResult:
        raise NotImplementedError

    def credits(self, account: Account, log: LogFn) -> CreditSnapshot:
        """只查积分，不触碰签到状态。"""
        raise NotImplementedError

    def checkin(self, account: Account, log: LogFn) -> ActionResult:
        raise NotImplementedError

    def client_exe(self, account: Account | None = None) -> Path | None:
        return None
