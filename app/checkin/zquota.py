# -*- coding: utf-8 -*-
"""ZCode 额度查询。

两条通道：BigModel 走 monitor/usage/quota/limit（返回 5 小时窗口 + 周额度），
z.ai 走 zcode-plan/billing/balance（返回套餐 + 各计量池）。两者都按「额度」口径
归一化，跟本工具其他平台的「积分」是两套概念，不要混着算。
"""
from __future__ import annotations

import time
import uuid
from datetime import datetime

import requests

from . import zcode

QUOTA_LIMIT_URL = "https://open.bigmodel.cn/api/monitor/usage/quota/limit"
SUBSCRIPTION_URL = "https://open.bigmodel.cn/api/biz/subscription/list"
BILLING_BALANCE_URL = "https://zcode.z.ai/api/v1/zcode-plan/billing/balance"
CLIENT_APP_VERSION = "3.16.3"
TIMEOUT = 20

_ZONE_NAMES = {8: "Asia/Shanghai", 9: "Asia/Tokyo", 0: "UTC"}


def _timezone() -> str:
    offset = -time.timezone // 3600 if time.daylight == 0 else -time.altzone // 3600
    return _ZONE_NAMES.get(offset, "unknown")


def _os_version() -> str:
    try:
        import winreg
        with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE,
                            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion") as key:
            build = str(winreg.QueryValueEx(key, "CurrentBuildNumber")[0])
        return f"10.0.{build}"
    except (OSError, ValueError):
        return ""


def _client_platform() -> str:
    return "win32-x64"


def zai_headers(token: str, mid: str | None = None) -> dict:
    headers = {
        "User-Agent": f"ZCode/{CLIENT_APP_VERSION}",
        "HTTP-Referer": "https://zcode.z.ai",
        "X-Title": "Z Code@electron",
        "X-ZCode-App-Version": CLIENT_APP_VERSION,
        "X-Platform": _client_platform(),
        "X-Release-Channel": "stable",
        "X-Client-Language": "zh-CN",
        "X-Client-Timezone": _timezone(),
        "X-Os-Category": "windows",
        "Authorization": f"Bearer {token}",
        "x-request-id": str(uuid.uuid4()),
    }
    os_version = _os_version()
    if os_version:
        headers["X-Os-Version"] = os_version
    if mid:
        headers["X-Device-Mid"] = mid
    return headers


def _bigmodel_headers(token: str) -> dict:
    return {
        "Authorization": f"Bearer {token}",
        "User-Agent": f"ZCode/{CLIENT_APP_VERSION}",
        "x-request-id": str(uuid.uuid4()),
    }


# ------------------------------------------------------------------ 取 token
def candidate_tokens(creds: dict, config: dict | None) -> list[str]:
    """按「客户端真实会用的顺序」给出候选 token：JWT 优先，其次 provider 的 access token。"""
    tokens: list[str] = []

    def add(value: str | None) -> None:
        if value and len(value.strip()) > 20 and value not in tokens:
            tokens.append(value)

    add(zcode.field(creds, "zcodejwttoken"))
    provider = zcode.field(creds, "oauth:active_provider") or "zai"
    for key in (f"oauth:{provider}:access_token", "oauth:bigmodel:access_token",
                "oauth:zai:access_token"):
        add(zcode.field(creds, key))
    # 新版把 coding-plan 的 apiKey 直接放在 config.json 的 provider 表里
    providers = (config or {}).get("provider")
    if isinstance(providers, dict):
        for entry in providers.values():
            if not isinstance(entry, dict) or "coding-plan" not in str(entry.get("name", "")):
                continue
            key = (entry.get("options") or {}).get("apiKey")
            if isinstance(key, str) and not key.startswith("enc:"):
                add(key)
    return tokens


# ------------------------------------------------------------------ 归一化
def _number(value) -> float | None:
    if isinstance(value, bool) or value is None:
        return None
    if isinstance(value, (int, float)):
        return float(value)
    try:
        return float(str(value).strip())
    except (TypeError, ValueError):
        return None


def _reset_text(ms) -> str:
    value = _number(ms)
    if not value:
        return ""
    seconds = value / 1000 if value > 1e11 else value
    try:
        at = datetime.fromtimestamp(seconds)
    except (OSError, OverflowError, ValueError):
        return ""
    return f"{at:%m-%d %H:%M}"


def _unit_label(unit, number) -> str:
    """unit 1/3/5 + number 组合出「5 小时」「每周」这类人话窗口名。"""
    unit = int(unit) if isinstance(unit, (int, float)) else 0
    count = int(number) if isinstance(number, (int, float)) else 0
    if unit == 1:      # 分钟
        return f"{count} 分钟" if count and count < 60 else "小时"
    if unit == 3:      # 小时
        return f"{count} 小时" if count else "小时"
    if unit == 5:      # 天
        return f"{count} 天" if count else "天"
    return ""


def _tier_from_level(level: str) -> str:
    text = (level or "").strip()
    lowered = text.lower()
    if "max" in lowered:
        return "Max"
    if "pro" in lowered:
        return "Pro"
    if "lite" in lowered:
        return "Lite"
    if "free" in lowered:
        return "Free"
    return text or ""


def _business_ok(data: dict) -> bool:
    code = data.get("code")
    if code is not None and code not in (0, 200):
        return False
    return data.get("success") is not False


def _plan_expire(entry: dict) -> str:
    for key in ("ends_at", "end_time", "expire_time", "expires_at", "endTime"):
        value = entry.get(key)
        if isinstance(value, str) and value.strip():
            return value.strip()[:19]
        number = _number(value)
        if number:
            seconds = number / 1000 if number > 1e11 else number
            try:
                return f"{datetime.fromtimestamp(seconds):%Y-%m-%d %H:%M}"
            except (OSError, OverflowError, ValueError):
                continue
    return ""


def _normalize_limit(data: dict, subscription: dict | None) -> dict:
    body = data.get("data") or {}
    limits = body.get("limits") or []
    items = []
    for entry in limits:
        if not isinstance(entry, dict):
            continue
        kind = entry.get("type") or ""
        total = _number(entry.get("usage"))
        used = _number(entry.get("currentValue"))
        remaining = _number(entry.get("remaining"))
        percent = _number(entry.get("percentage"))
        if total and used is not None and total > 0:
            percent = min(100.0, max(0.0, used / total * 100.0))
        window = _unit_label(entry.get("unit"), entry.get("number"))
        name = {"TOKENS_LIMIT": "Token 额度", "TIME_LIMIT": "使用时长"}.get(kind, kind or "额度")
        items.append({
            "name": f"{name}（{window}）" if window else name,
            "kind": "time" if kind == "TIME_LIMIT" else "tokens",
            "total": total,
            "used": used,
            "remaining": remaining,
            "percent_used": percent,
            "window": window,
            "unit": "分钟" if kind == "TIME_LIMIT" else "次",
            "reset": _reset_text(entry.get("nextResetTime")),
        })

    tier = _tier_from_level(str(body.get("level") or ""))
    expire = ""
    if subscription and _business_ok(subscription):
        rows = subscription.get("data")
        if isinstance(rows, list) and rows:
            current = next((row for row in rows if isinstance(row, dict)
                            and str(row.get("status") or "").upper() == "VALID"), rows[0])
            if isinstance(current, dict):
                name = str(current.get("productName") or "").strip()
                if name:
                    tier = _tier_from_level(name) or name
                expire = _plan_expire(current)

    main = next((item for item in items if item["kind"] == "time" and item["percent_used"] is not None), None)
    main = main or next((item for item in items if item["percent_used"] is not None), None)
    return {
        "items": items,
        "plan_tier": tier,
        "plan_expire": expire,
        "percent_used": main["percent_used"] if main else None,
        "main_window": main["window"] if main else "",
        "reset": main["reset"] if main else "",
        "is_empty": not items,
        "source": "bigmodel",
    }


def _normalize_balance(data: dict) -> dict:
    body = data.get("data") if isinstance(data.get("data"), dict) else data
    items = []
    for entry in body.get("balances") or []:
        if not isinstance(entry, dict):
            continue
        total = _number(entry.get("total_units"))
        used = _number(entry.get("used_units"))
        remaining = _number(entry.get("remaining_units")) or _number(entry.get("available_units"))
        percent = None
        if total and used is not None and total > 0:
            percent = min(100.0, max(0.0, used / total * 100.0))
        items.append({
            "name": str(entry.get("show_name") or entry.get("name") or "额度"),
            "kind": "tokens",
            "total": total,
            "used": used,
            "remaining": remaining,
            "percent_used": percent,
            "window": "",
            "unit": str(entry.get("unit_type") or entry.get("meter") or ""),
            "reset": _reset_text(entry.get("next_reset_time") or entry.get("reset_time")),
        })

    tier, expire = "", ""
    plans = body.get("plans") or []
    for plan in plans:
        if not isinstance(plan, dict):
            continue
        if str(plan.get("status") or "").lower() != "active":
            continue
        name = str(plan.get("name") or plan.get("plan_id") or "")
        tier = _tier_from_level(name) or name
        expire = _plan_expire(plan)
        break

    main = next((item for item in items if item["percent_used"] is not None), None)
    return {
        "items": items,
        "plan_tier": tier,
        "plan_expire": expire,
        "percent_used": main["percent_used"] if main else None,
        "main_window": main["name"] if main else "",
        "reset": main["reset"] if main else "",
        "is_empty": not items,
        "source": "zai",
    }


# ------------------------------------------------------------------ 查询
class QuotaError(RuntimeError):
    pass


def _get(url: str, token: str, mid: str | None) -> dict:
    headers = zai_headers(token, mid) if "zcode.z.ai" in url else _bigmodel_headers(token)
    last = ""
    for attempt, delay in enumerate((0, 0.5, 1.5, 4.0)):
        if delay:
            time.sleep(delay)
        try:
            resp = requests.get(url, headers=headers, timeout=TIMEOUT)
        except requests.RequestException as exc:
            raise QuotaError(f"网络错误：{exc}") from exc
        if resp.status_code == 429:
            last = "额度接口限流（HTTP 429）"
            continue
        if resp.status_code in (401, 403):
            raise QuotaError(f"登录态已失效（HTTP {resp.status_code}）")
        if resp.status_code >= 400:
            raise QuotaError(f"额度接口返回 HTTP {resp.status_code}")
        try:
            data = resp.json()
        except ValueError:
            raise QuotaError("额度接口返回的不是 JSON")
        if not isinstance(data, dict):
            raise QuotaError("额度接口返回格式异常")
        return data
    raise QuotaError(last or "额度接口无响应")


def query(creds: dict, config: dict | None, mid: str | None = None) -> dict:
    """返回归一化后的额度；一条通道失败就换下一条，全失败才抛错。"""
    tokens = candidate_tokens(creds, config)
    if not tokens:
        raise QuotaError("没有可用的登录凭据")
    errors: list[str] = []
    no_plan = False
    auth_fail = 0
    for token in tokens:
        try:
            data = _get(QUOTA_LIMIT_URL, token, mid)
        except QuotaError as exc:
            if "401" in str(exc):
                auth_fail += 1
            errors.append(str(exc))
            continue
        if _business_ok(data):
            try:
                subscription = _get(SUBSCRIPTION_URL, token, mid)
            except QuotaError:
                subscription = None
            return _normalize_limit(data, subscription)
        code = data.get("code")
        message = str(data.get("msg") or data.get("message") or "")
        if code == 401:
            auth_fail += 1
        if "coding plan" in message or "没有资格" in message:
            # 这个 token 在 BigModel 侧没有 coding plan，但 z.ai 账号本来就走余额通道，
            # 不能就此判定「无额度」——继续往下试 billing/balance
            no_plan = True
            continue
        errors.append(message or f"接口返回 code={code}")
    if auth_fail >= len(tokens) and not no_plan:
        # 全部 token 都 401：余额通道对过期 token 也会返回空数据，继续问下去
        # 只会把「没有套餐」显示成「读取失败」。结论按「无额度」给，
        # 另外打一个 stale 标记，让界面提示「登录态可能已过期」。
        return {"items": [], "plan_tier": "", "plan_expire": "", "percent_used": None,
                "main_window": "", "reset": "", "is_empty": True, "source": "no_plan",
                "stale": True}
    for token in tokens:
        try:
            data = _get(f"{BILLING_BALANCE_URL}?app_version={CLIENT_APP_VERSION}", token, mid)
        except QuotaError as exc:
            errors.append(str(exc))
            continue
        if _business_ok(data):
            return _normalize_balance(data)
    if no_plan:
        return {"items": [], "plan_tier": "Free", "plan_expire": "", "percent_used": None,
                "main_window": "", "reset": "", "is_empty": True, "source": "no_plan",
                "stale": auth_fail >= len(tokens)}
    raise QuotaError(errors[0] if errors else "额度查询失败")


def summary_text(quota: dict) -> str:
    """表格/提示用的一句话：只说最紧的那条窗口，细节在明细面板里。"""
    if not quota or quota.get("is_empty"):
        tier = (quota or {}).get("plan_tier") or ""
        return f"{tier} · 无额度数据" if tier else "无额度数据"
    parts = []
    for item in quota.get("items") or []:
        if item.get("percent_used") is None:
            continue
        window = item.get("window") or item.get("name") or ""
        remain = max(0.0, 100.0 - float(item["percent_used"]))
        parts.append(f"{window}剩 {remain:.0f}%" if window else f"剩 {remain:.0f}%")
    if not parts:
        return "额度未提供"
    tier = quota.get("plan_tier") or ""
    text = " · ".join(parts[:2])
    return f"{tier} · {text}" if tier else text
