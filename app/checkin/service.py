# -*- coding: utf-8 -*-
"""签到编排层：扫描 → 查询状态 → 签到 → 记录台账。

不依赖任何界面框架，界面只负责把这里的返回值画出来。
"""
from __future__ import annotations

import json
from datetime import datetime, time, timedelta

from ..core import paths, tasks
from . import ledger, logins, store
from .base import Account, ActionResult, CreditSnapshot, StatusResult
from .registry import PROVIDERS, PROVIDER_MAP

# 记录的是「上次成功签到的时刻」，不是日期：Qoder 按滚动 24 小时算，
# 只存日期会把「昨天 20:00 签的」当成「今天可以签」。
_TIME_FMT = "%Y-%m-%dT%H:%M:%S"
_KEEP_DAYS = 3


# ------------------------------------------------------------------ 今日记录
def _today() -> str:
    return datetime.now().strftime("%Y-%m-%d")


def _parse_checked(value) -> datetime | None:
    """兼容旧格式（只存日期，按当天 0 点算）。"""
    if not value:
        return None
    text = str(value)
    for fmt in (_TIME_FMT, "%Y-%m-%d"):
        try:
            return datetime.strptime(text, fmt)
        except ValueError:
            continue
    return None


def _load_checked() -> dict:
    try:
        data = json.loads(paths.CHECKED_FILE.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return data if isinstance(data, dict) else {}


def checked_at(iid: str) -> datetime | None:
    """本工具上次为该账号成功签到的时刻；没记录返回 None。"""
    return _parse_checked(_load_checked().get(iid))


def checked_today(iid: str) -> bool:
    """本工具今天是否已为该账号成功签到。"""
    at = checked_at(iid)
    return bool(at and at.date() == datetime.now().date())


def mark_checked(iid: str) -> None:
    """记录签到成功的时刻；只保留最近几天的记录，避免文件无限增长。"""
    now = datetime.now()
    data: dict = {}
    for key, value in _load_checked().items():
        at = _parse_checked(value)
        if key != iid and at and (now - at).days < _KEEP_DAYS:
            data[key] = value
    data[iid] = now.strftime(_TIME_FMT)
    try:
        paths.CHECKED_FILE.parent.mkdir(parents=True, exist_ok=True)
        paths.CHECKED_FILE.write_text(json.dumps(data, ensure_ascii=False, indent=2),
                                      encoding="utf-8")
    except OSError:
        pass


def checked_ids() -> set[str]:
    """今天已经签过的账号（自然日口径，用于汇总文案）。"""
    today = datetime.now().date()
    out = set()
    for key, value in _load_checked().items():
        at = _parse_checked(value)
        if at and at.date() == today:
            out.add(key)
    return out


# ------------------------------------------------------------- 可签时机
def cooldown_hours(platform: str) -> int:
    provider = PROVIDER_MAP.get(platform)
    return int(getattr(provider, "cooldown_hours", 0) or 0)


def checkin_due(account: Account, now: datetime | None = None) -> bool:
    """现在是否已经可以签到。

    24 小时制的平台（Qoder）看上次签到时刻 + 冷却；其余平台按自然日，
    过 0 点就算新的一天。
    """
    moment = now or datetime.now()
    at = checked_at(account.iid)
    hours = cooldown_hours(account.platform)
    if hours:
        return at is None or moment - at >= timedelta(hours=hours)
    return not (at and at.date() == moment.date())


def cooldown_left(account: Account) -> str:
    """剩余冷却时长的简短文案；不需要等待或已到期时返回空串。"""
    hours = cooldown_hours(account.platform)
    if not hours:
        return ""
    at = checked_at(account.iid)
    if at is None:
        return ""
    left = at + timedelta(hours=hours) - datetime.now()
    seconds = left.total_seconds()
    if seconds <= 0:
        return ""
    minutes = int(seconds // 60)
    if minutes >= 60:
        return f"{minutes // 60}h{minutes % 60:02d}m"
    return f"{max(minutes, 1)}m"


def next_checkin_at(account: Account, now: datetime | None = None) -> datetime | None:
    """下一次可以签到的本地时刻；现在就能签、或本就不需要签到，返回 None。

    自然日制看次日 0 点，滚动制看上次签到 + 冷却小时数。只按本地记录推算，
    接口若另有口径以接口为准——所以界面文案用「约」而不是承诺。
    """
    if not account.requires_checkin:
        return None
    moment = now or datetime.now()
    at = checked_at(account.iid)
    hours = cooldown_hours(account.platform)
    if hours:
        if at is None:
            return None
        target = at + timedelta(hours=hours)
    else:
        if at is None or at.date() != moment.date():
            return None
        target = datetime.combine(at.date() + timedelta(days=1), time.min)
    return target if target > moment else None


def countdown_text(target: datetime | None, now: datetime | None = None) -> str:
    """倒计时文案；target 为空表示现在就能签。"""
    if target is None:
        return "现在可签"
    moment = now or datetime.now()
    seconds = int((target - moment).total_seconds())
    if seconds <= 0:
        return "现在可签"
    hours, rest = divmod(seconds, 3600)
    minutes, secs = divmod(rest, 60)
    if hours:
        return f"还剩 {hours}h {minutes:02d}m {secs:02d}s"
    if minutes:
        return f"还剩 {minutes}m {secs:02d}s"
    return f"还剩 {secs}s"


# --------------------------------------------------------------------- 扫描
def collect(cfg: dict, log) -> list[Account]:
    """扫官方客户端的本地登录态，再合并浏览器登录进来的账号。"""
    accounts: list[Account] = []
    extras = cfg.get("extra_sources") or {}
    for provider in PROVIDERS:
        log(f"[{provider.label}] 读取登录态")
        try:
            found = provider.scan(lambda message: log(message),
                                  tuple(extras.get(provider.key, [])))
        except Exception as exc:
            log(f"[{provider.label}] 读取失败：{exc}", "error")
            continue
        if not found:
            log(f"[{provider.label}] 没有可用账号", "dim")
        accounts.extend(found)

    seen = {account.iid for account in accounts}
    added = 0
    for account in store.as_accounts():
        if account.iid in seen:
            continue
        accounts.append(account)
        seen.add(account.iid)
        added += 1
    if added:
        log(f"浏览器登录账号：{added} 个", "dim")

    order = {"trae": 0, "qoder": 1, "qoder_intl": 2, "workbuddy": 3}
    accounts.sort(key=lambda a: (order.get(a.platform, 9), a.name.lower()))
    return accounts


# ----------------------------------------------------------------- 单账号动作
def query_one(account: Account, skip: bool, log) -> tuple[str, str, object]:
    """返回 (类型, iid, 结果)。类型为 status / credit。

    免签到的账号只查积分，不触发签到状态查询——避免无意义地"看"签到状态。
    平台本身没有签到玩法（如 WorkBuddy 国际版）时同样只查积分。
    """
    account = logins.ensure_fresh(account, log)
    provider = PROVIDER_MAP[account.platform]
    iid = account.iid
    if skip or not provider.requires_checkin(account):
        try:
            return "credit", iid, provider.credits(account, lambda m: log(m))
        except Exception as exc:
            return "credit", iid, CreditSnapshot(f"积分读取失败：{exc}")
    if account.expired:
        return "status", iid, StatusResult(None, "登录态已过期")
    try:
        return "status", iid, provider.status(account, lambda m: log(m))
    except Exception as exc:
        return "status", iid, StatusResult(None, f"查询失败：{exc}")


def query_credit(account: Account, log) -> CreditSnapshot:
    """只查积分（含批次到期），不查签到状态。

    供「刷新积分」按钮和定时轮询使用：比 query_one 轻，
    不触发签到状态查询，适合高频刷新。
    """
    account = logins.ensure_fresh(account, log)
    provider = PROVIDER_MAP[account.platform]
    try:
        return provider.credits(account, lambda m: log(m))
    except Exception as exc:
        return CreditSnapshot(f"积分读取失败：{exc}")


def query_credits(accounts: list[Account], log) -> dict[str, object]:
    """并发查一批账号的积分，返回 {iid: CreditSnapshot}（供轮询广播用）。"""
    results: dict[str, object] = {}

    def one(account: Account) -> None:
        results[account.iid] = query_credit(account, log)

    tasks.parallel_map(accounts, one)
    return results


def checkin_one(account: Account, log) -> ActionResult:
    account = logins.ensure_fresh(account, log)
    if account.expired:
        return ActionResult(False, "登录态已过期")
    provider = PROVIDER_MAP[account.platform]
    if not provider.requires_checkin(account):
        return ActionResult(False, f"{account.platform_label} 无需签到")
    try:
        return provider.checkin(account, lambda m: log(m))
    except Exception as exc:
        return ActionResult(False, str(exc))


def record_grant(account: Account, result: ActionResult) -> None:
    """把接口返回的发放额写进台账（只对不返回批次的平台有意义）。"""
    if not (result.ok and result.granted):
        return
    provider = PROVIDER_MAP[account.platform]
    if provider.live_credit:
        return
    key = f"{account.iid}:{datetime.now():%Y-%m-%d %H:%M:%S}:{result.granted:g}"
    ledger.add(account.iid, result.granted, result.validity_days, key)


def refresh_status_after_checkin(account: Account, log) -> StatusResult | None:
    provider = PROVIDER_MAP[account.platform]
    if not provider.requires_checkin(account):
        return None
    try:
        return provider.status(account, lambda m: log(m))
    except Exception:
        return None


def client_exe(platform: str, account: Account | None = None):
    provider = PROVIDER_MAP.get(platform)
    return provider.client_exe(account) if provider else None


# ---------------------------------------------------------------------- 自检
def selftest() -> int:
    """只读自检：扫描登录态 + 查询状态，结果写入 data/logs/selftest.log。"""
    lines: list[str] = []
    ok = True

    def note(message: str, level: str = "info") -> None:
        nonlocal ok
        if level == "error":
            ok = False
        lines.append(message)

    note(f"AI Manager 自检 {datetime.now():%Y-%m-%d %H:%M:%S}")
    from ..core import config
    cfg = config.load()
    skip = set(cfg.get("skip") or [])
    for provider in PROVIDERS:
        note(f"\n--- {provider.label} ---")
        try:
            accounts = provider.scan(lambda m: note(m),
                                     tuple((cfg.get("extra_sources") or {}).get(provider.key, [])))
        except Exception as exc:
            note(f"扫描失败：{exc}", "error")
            continue
        # 自检也要覆盖浏览器登录账号，否则账号中心能看到、自检却漏掉
        seen = {account.iid for account in accounts}
        for stored in store.as_accounts():
            if stored.platform == provider.key and stored.iid not in seen:
                accounts.append(stored)
                seen.add(stored.iid)
        if not accounts:
            note("未发现账号")
            continue
        for account in accounts:
            note(f"{account.name} | 有效期 {account.expires_text()} | 已过期 {account.expired}")
            try:
                if account.iid in skip or not provider.requires_checkin(account):
                    snapshot = provider.credits(account, lambda m: note(m))
                    note(f"  免签到，积分={snapshot.summary}")
                    continue
                result = provider.status(account, lambda m: note(m))
                note(f"  今日签到={result.checked_in} 额度={result.summary}")
            except Exception as exc:
                note(f"  查询失败：{exc}", "error")

    note("\n自检完成" if ok else "\n自检存在失败项")
    try:
        target = paths.LOG_DIR / "selftest.log"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text("\n".join(lines), encoding="utf-8")
    except OSError:
        pass
    try:
        print("\n".join(lines))
    except (AttributeError, OSError, ValueError):
        # pythonw 下 sys.stdout 为 None，print 会抛 AttributeError；
        # 结果已经写进 selftest.log，这里静默即可，别为了控制台输出崩掉进程
        pass
    return 0 if ok else 1
