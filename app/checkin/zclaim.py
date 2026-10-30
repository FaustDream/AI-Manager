# -*- coding: utf-8 -*-
"""ZCode 活动套餐的查询与领取。

领取必须带阿里云验证码参数（上游强制），验证码在 zcaptcha 里用浏览器过，
拿到参数后由本模块提交。活动来源有两条：billing/preview（常规活动）与
marketing/touch（客户端弹窗活动），两条都查才不漏。
"""
from __future__ import annotations

import requests

from . import zcaptcha, zquota

API_BASE = "https://zcode.z.ai"
TIMEOUT = 25


class ClaimError(RuntimeError):
    def __init__(self, message: str, code: int = -1) -> None:
        super().__init__(message)
        self.code = code

    @property
    def challenge(self) -> bool:
        return self.code == -2

    @property
    def rejected(self) -> bool:
        """验证码被拒（3007）：上游视为风险信号，不自动换码重试。"""
        return self.code == 3007


def _token(creds: dict, config: dict | None) -> str:
    tokens = zquota.candidate_tokens(creds, config)
    if not tokens:
        raise ClaimError("没有可用的登录凭据，请先保存该账号的登录")
    return tokens[0]


def _get(url: str, token: str, mid: str | None, what: str) -> dict:
    try:
        resp = requests.get(url, headers=zquota.zai_headers(token, mid), timeout=TIMEOUT)
    except requests.RequestException as exc:
        raise ClaimError(f"{what}请求失败：{exc}") from exc
    if resp.status_code in (401, 403):
        raise ClaimError("登录态已失效，请重新登录该账号")
    if resp.status_code >= 400:
        raise ClaimError(f"{what}返回 HTTP {resp.status_code}")
    try:
        data = resp.json()
    except ValueError:
        raise ClaimError(f"{what}返回的不是 JSON")
    if not isinstance(data, dict):
        raise ClaimError(f"{what}返回格式异常")
    code = int(data.get("code") or 0)
    if code not in (0, 200):
        raise ClaimError(_failure_message(code, data), code)
    return data


def _failure_message(code: int, body: dict) -> str:
    base = {
        1001: "活动不存在或已结束",
        1002: "该活动不支持领取",
        1003: "领取条件未满足",
        1004: "该账号已领取过",
        1005: "领取过于频繁，请稍后再试",
        3001: "领取失败，请稍后重试",
        3007: "验证码校验未通过，请重新发起领取",
        401: "登录态已失效，请重新登录",
    }.get(code, f"领取失败（code {code}）")
    server = str(body.get("msg") or body.get("message") or "").strip()
    return f"{base}：{server}" if server else base


def _plans_of(data: dict) -> list[dict]:
    plans = (data.get("data") or {}).get("plans")
    return [item for item in plans if isinstance(item, dict)] if isinstance(plans, list) else []


def _fmt_units(value: float) -> str:
    if value >= 1e8:
        return f"{value / 1e8:.1f}".rstrip("0").rstrip(".") + "亿"
    if value >= 1e4:
        return f"{value / 1e4:.1f}".rstrip("0").rstrip(".") + "万"
    return f"{value:.0f}"


def _effective_at(plan: dict) -> int:
    """权益真正生效的时刻在 entitlement 上，plan.starts_at 只是活动开始时间。"""
    moments = [int(item["effective_at"]) for item in plan.get("entitlements") or []
               if isinstance(item, dict) and isinstance(item.get("effective_at"), (int, float))]
    return min(moments) * 1000 if moments else 0


def _parse_plan(plan: dict) -> dict | None:
    plan_id = str(plan.get("plan_id") or plan.get("planId") or "").strip()
    if not plan_id:
        return None
    grants = []
    for item in plan.get("entitlements") or []:
        if not isinstance(item, dict):
            continue
        if item.get("meter") != "model_usage" or item.get("unit_type") != "token":
            continue
        name = str(item.get("show_name") or item.get("showName") or "").strip()
        if not name:
            continue
        units = float(item.get("grant_units") or item.get("grantUnits") or 0)
        period = str(item.get("period") or "one_time")
        period_cn = {"daily": "每日", "weekly": "每周", "monthly": "每月"}.get(period, "一次性")
        grants.append(f"{name} · {_fmt_units(units)} Token（{period_cn}）")
    return {
        "plan_id": plan_id,
        "name": str(plan.get("name") or "").strip(),
        "description": str(plan.get("description") or "").strip(),
        "priority": int(plan.get("priority") or 0),
        "grants": grants,
        "effective_at": _effective_at(plan),
    }


# ------------------------------------------------------------------ 查询
def preview(creds: dict, config: dict | None, mid: str | None = None) -> list[dict]:
    token = _token(creds, config)
    url = (f"{API_BASE}/api/v1/zcode-plan/billing/preview"
           f"?app_version={zquota.CLIENT_APP_VERSION}&platform=win32-x64")
    data = _get(url, token, mid, "活动预览")
    plans = [plan for plan in (_parse_plan(item) for item in _plans_of(data)) if plan]
    plans.sort(key=lambda item: (-item["priority"], item["plan_id"]))
    return plans


def _walk_plans(node, depth: int = 0) -> list[dict]:
    found: list[dict] = []
    if depth > 8:
        return found
    if isinstance(node, dict):
        if node.get("plan_id") or node.get("planId"):
            found.append(node)
        for value in node.values():
            found.extend(_walk_plans(value, depth + 1))
    elif isinstance(node, list):
        for item in node:
            found.extend(_walk_plans(item, depth + 1))
    return found


def marketing_plans(creds: dict, config: dict | None, mid: str | None = None) -> list[dict]:
    """活动弹窗通道：只读 marketing/touch，失败一律当没有活动，不影响 preview。"""
    try:
        token = _token(creds, config)
        data = _get(f"{API_BASE}/api/v1/marketing/touch?seq=0", token, mid, "活动下发")
    except ClaimError:
        return []
    out: dict[str, dict] = {}
    for delivery in (data.get("data") or {}).get("deliveries") or []:
        if not isinstance(delivery, dict):
            continue
        campaign = {"title": str(delivery.get("title") or "").strip(),
                    "position": str(delivery.get("position") or delivery.get("type") or "").strip()}
        for payload in _walk_plans(delivery):
            parsed = _parse_plan(payload)
            if parsed:
                parsed["campaign"] = campaign
                out.setdefault(parsed["plan_id"], parsed)
    return list(out.values())


def claimable(creds: dict, config: dict | None, mid: str | None = None) -> list[dict]:
    """preview 为主、marketing 补充：同 id 以 preview 为准（字段更全）。"""
    plans = preview(creds, config, mid)
    seen = {item["plan_id"] for item in plans}
    for item in marketing_plans(creds, config, mid):
        if item["plan_id"] not in seen:
            plans.append(item)
    return plans


def active_plans(creds: dict, config: dict | None, mid: str | None = None) -> list[dict]:
    """已生效套餐，用来区分「没有活动」和「已领取、正在生效」。"""
    token = _token(creds, config)
    url = (f"{API_BASE}/api/v1/zcode-plan/billing/current"
           f"?app_version={zquota.CLIENT_APP_VERSION}&platform=win32-x64")
    out = []
    for plan in _plans_of(_get(url, token, mid, "已领取套餐")):
        out.append({
            "plan_id": str(plan.get("plan_id") or plan.get("planId") or ""),
            "name": str(plan.get("name") or ""),
            "status": str(plan.get("status") or ""),
            "starts_at": int(plan.get("starts_at") or 0) * 1000,
            "ends_at": int(plan.get("ends_at") or 0) * 1000,
            "effective_at": _effective_at(plan),
        })
    return out


# ------------------------------------------------------------------ 领取
def submit(creds: dict, config: dict | None, plan_id: str, param: str,
           region: str = "", mid: str | None = None) -> dict:
    if not param.strip():
        raise ClaimError("验证码参数为空")
    token = _token(creds, config)
    headers = zquota.zai_headers(token, mid)
    headers["X-Aliyun-Captcha-Verify-Param"] = param.strip()
    if region.strip():
        headers["X-Aliyun-Captcha-Verify-Region"] = region.strip()
    try:
        resp = requests.post(f"{API_BASE}/api/v1/zcode-plan/billing/claim",
                             headers=headers, json={"plan_id": plan_id}, timeout=TIMEOUT)
    except requests.RequestException as exc:
        raise ClaimError(f"领取请求失败：{exc}") from exc
    if resp.status_code in (401, 403):
        # 401/403 也可能是人机校验挑战，不能一律当成登录失效
        if resp.headers.get("x-aliyun-captcha-verify-param") or "captcha" in resp.text.lower():
            raise ClaimError("需要重新完成人机校验", -2)
        raise ClaimError("登录态已失效，请重新登录该账号")
    try:
        data = resp.json()
    except ValueError:
        raise ClaimError("领取接口返回的不是 JSON")
    code = int(data.get("code") or -1)
    if code not in (0, 200):
        raise ClaimError(_failure_message(code, data), code)
    return data


def claim(creds: dict, config: dict | None, plan_id: str, mid: str | None = None,
          log=None) -> dict:
    """完整领取：验证码 → 提交。验证码被拒（3007）不自动重试，交给用户重新发起。"""
    if log:
        log("[ZCode] 已在浏览器打开验证码页，完成后自动提交领取…")
    try:
        param, region = zcaptcha.request()
    except zcaptcha.CaptchaError as exc:
        raise ClaimError(str(exc)) from exc
    result = submit(creds, config, plan_id, param, region, mid)
    plan = ((result.get("data") or {}).get("plan") or {})
    return {
        "ok": True,
        "name": str(plan.get("name") or ""),
        "effective_at": int(plan.get("starts_at") or 0) * 1000,
        "ends_at": int(plan.get("ends_at") or 0) * 1000,
    }
