# -*- coding: utf-8 -*-
"""ZCode 添加账号：官方 CLI OAuth 流程（init → 浏览器授权 → poll）。

和另外三个平台一样是「浏览器登录」，但 ZCode 下发的是 zcode 自己的 JWT + 一份
provider 配置（config.json 里的 apiKey 由 access token 换出来），两者都要存下来，
否则切过去客户端会因为缺 apiKey 用不了。
"""
from __future__ import annotations

import json
import secrets
import threading
import time
import uuid
from datetime import datetime, timedelta, timezone
from urllib.parse import parse_qs, quote, urlparse

import requests

from . import zcode, zquota
from .base import Account
from .logins import LoginError

INIT_URL = "https://zcode.z.ai/api/v1/oauth/cli/init"
API_KEY_NAME = "zcode-api-key"
BIGMODEL_BIZ_BASE = "https://bigmodel.cn"
ZAI_API_BASE = "https://api.z.ai"
ZAI_BUSINESS_LOGIN = "https://api.z.ai/api/auth/z/login"
BIGMODEL_ANTHROPIC = "https://open.bigmodel.cn/api/anthropic"
ZAI_ANTHROPIC = "https://api.z.ai/api/anthropic"
START_PLAN_ANTHROPIC = "https://zcode.z.ai/api/v1/zcode-plan/anthropic"

PROVIDERS = (("bigmodel", "BigModel（智谱开放平台）"), ("zai", "z.ai（国际站）"))


def _get(url: str, auth: str, timeout: int = 15) -> dict | None:
    try:
        resp = requests.get(url, headers={"Authorization": auth,
                                          "Content-Type": "application/json"}, timeout=timeout)
        data = resp.json()
    except (requests.RequestException, ValueError):
        return None
    return data if isinstance(data, dict) else None


def _pick_org_project(customer: dict) -> tuple[str, str] | None:
    root = customer.get("data") if isinstance(customer.get("data"), dict) else customer
    orgs = root.get("organizations")
    if not isinstance(orgs, list):
        return None
    candidates = []
    for org in orgs:
        if not isinstance(org, dict):
            continue
        org_id = org.get("organizationId")
        if org_id in (None, ""):
            continue
        projects = [item for item in (org.get("projects") or [])
                    if isinstance(item, dict) and str(item.get("projectType") or "") != "2"]
        if projects:
            candidates.append((org, str(org_id), projects))
    if not candidates:
        return None
    chosen = next((item for item in candidates
                   if "默认机构" in str(item[0].get("organizationName") or "")), candidates[0])
    projects = chosen[2]
    project = next((item for item in projects
                    if "默认项目" in str(item.get("projectName") or "")), projects[0])
    project_id = project.get("projectId")
    if project_id in (None, ""):
        return None
    return chosen[1], str(project_id)


def resolve_biz_api_key(base: str, auth: str, require_secret: bool) -> str | None:
    """拿 access token 换一个 anthropic 用的 apiKey（含 secret）。"""
    customer = _get(f"{base}/api/biz/customer/getCustomerInfo", auth)
    if not customer:
        return None
    pair = _pick_org_project(customer)
    if not pair:
        return None
    org, project = pair
    keys_url = f"{base}/api/biz/v1/organization/{org}/projects/{project}/api_keys"
    listing = _get(keys_url, auth) or {}
    rows = listing.get("data") if isinstance(listing.get("data"), list) else listing
    found = None
    if isinstance(rows, list):
        found = next((item for item in rows
                      if isinstance(item, dict) and item.get("name") == API_KEY_NAME), None)
    if found is None:
        try:
            resp = requests.post(keys_url, headers={"Authorization": auth,
                                                    "Content-Type": "application/json"},
                                 json={"name": API_KEY_NAME}, timeout=15)
            found = resp.json()
        except (requests.RequestException, ValueError):
            return None
    key = str((found or {}).get("apiKey") or "").strip()
    if not key:
        return None
    detail = _get(f"{keys_url}/copy/{quote(key, safe='')}", auth) or {}
    secret = str(detail.get("secretKey") or "").strip()
    if not secret:
        return None if require_secret else key
    return f"{key}.{secret}"


def resolve_zai_business_token(access_token: str) -> str | None:
    try:
        resp = requests.post(ZAI_BUSINESS_LOGIN, json={"token": access_token}, timeout=15)
        data = resp.json()
    except (requests.RequestException, ValueError):
        return None
    node = data.get("data") if isinstance(data.get("data"), dict) else {}
    for key in ("access_token", "accessToken"):
        value = str(node.get(key) or "").strip()
        if value:
            return value
    return None


def fetch_userinfo(provider: str, token: str) -> dict | None:
    """登录回调只给 user_id 时，再问一次账号接口补昵称/邮箱。

    不补的话账号只能叫「ZCode 账号」，同一个号重新登录后还会把原来的名字覆盖掉。
    """
    if not token:
        return None
    if provider == "bigmodel":
        url, bearer = "https://bigmodel.cn/api/biz/customer/getCustomerInfo", False
    elif provider == "zai":
        url, bearer = "https://chat.z.ai/api/oauth/userinfo", True
    else:
        return None
    headers = {"Authorization": f"Bearer {token}" if bearer else token,
               "Content-Type": "application/json"}
    try:
        resp = requests.get(url, headers=headers, timeout=15)
        data = resp.json()
    except (requests.RequestException, ValueError):
        return None
    if data.get("code") not in (0, 200, None):
        return None
    node = data.get("data") if isinstance(data.get("data"), dict) else data

    def pick(*keys: str) -> str:
        for key in keys:
            value = str(node.get(key) or "").strip()
            if value:
                return value
        return ""

    return {
        "id": pick("id", "customerNumber", "sub"),
        # BigModel 把昵称放在 customerName，z.ai 放在 username/name
        "username": pick("username", "name", "preferred_username", "email",
                         "customerName", "nickName"),
        "displayName": pick("displayName", "nickName", "username", "name", "customerName"),
        "email": pick("email"),
    }


def assemble_config(provider: str, jwt: str, access_token: str) -> dict:
    def entry(name: str, key: str, base: str) -> dict:
        key = key.strip()
        options = {"apiKey": key, "baseURL": base}
        if not key:
            options = {"apiKey": "", "apiKeyRequired": True, "baseURL": base}
        return {"name": name, "kind": "anthropic", "options": options,
                "enabled": bool(key), "source": "custom"}

    providers: dict[str, dict] = {}
    if provider == "bigmodel":
        key = resolve_biz_api_key(BIGMODEL_BIZ_BASE, access_token, False) if access_token else ""
        key = key or ""
        providers["builtin:bigmodel"] = entry("Bigmodel - API Key", key, BIGMODEL_ANTHROPIC)
        providers["builtin:bigmodel-coding-plan"] = entry("BigModel - Coding Plan", key, BIGMODEL_ANTHROPIC)
        providers["builtin:bigmodel-start-plan"] = entry("BigModel- Coding Plan", jwt, START_PLAN_ANTHROPIC)
    elif provider == "zai":
        providers["builtin:zai"] = entry("Z.ai - API Key", "", ZAI_ANTHROPIC)
        providers["builtin:zai-start-plan"] = entry("Z.ai - Coding Plan", jwt, START_PLAN_ANTHROPIC)
        key = ""
        if access_token:
            business = resolve_zai_business_token(access_token) or ""
            key = resolve_biz_api_key(ZAI_API_BASE, f"Bearer {business or access_token}", True) or ""
        providers["builtin:zai-coding-plan"] = entry("Z.ai - Coding Plan", key, ZAI_ANTHROPIC)
    return {"provider": providers}


class ZCodeLogin:
    """与 logins.LoginFlow 同形：有 .url，poll(stop) 返回 Account。"""

    platform = "zcode"
    provider = "bigmodel"

    def __init__(self, provider: str = "bigmodel", device_mid: str | None = None) -> None:
        if provider not in dict(PROVIDERS):
            provider = "bigmodel"
        self.provider = provider
        self.mid = device_mid or str(uuid.uuid4())
        self._poll_token = secrets.token_hex(32)
        self._stop = False
        self._init()

    def _init(self) -> None:
        headers = zquota.zai_headers(self._poll_token, self.mid)
        try:
            resp = requests.post(INIT_URL, headers=headers,
                                 json={"provider": self.provider}, timeout=20)
            body = resp.json()
        except (requests.RequestException, ValueError) as exc:
            raise LoginError(f"获取 ZCode 登录链接失败：{exc}") from exc
        if body.get("code") not in (0, None):
            raise LoginError(f"获取 ZCode 登录链接失败：{body.get('msg') or body.get('code')}")
        data = body.get("data") or {}
        authorize = str(data.get("authorize_url") or "").strip()
        flow_id = str(data.get("flow_id") or "").strip()
        if not authorize or not flow_id:
            raise LoginError("ZCode 没有返回登录地址")
        self.url = authorize
        self.flow_id = flow_id
        self._poll_token = str(data.get("poll_token") or self._poll_token)
        self.poll_url = f"https://zcode.z.ai/api/v1/oauth/cli/poll/{quote(flow_id, safe='')}"
        expires = float(data.get("expires_at") or 0)
        self.deadline = expires if expires else time.time() + 300
        interval = float(data.get("poll_interval_sec") or 2)
        self.interval = max(1.0, interval)
        # state 只用于诊断，登录结果以 poll 为准
        self.state = (parse_qs(urlparse(self.url).query).get("state") or [""])[0]

    def cancel(self) -> None:
        self._stop = True

    def poll(self, stop: threading.Event | None = None) -> Account:
        while not self._stop and time.time() < self.deadline:
            if stop is not None and stop.is_set():
                raise LoginError("已取消")
            data = self._poll_once()
            if data:
                return self._finish(data)
            time.sleep(self.interval)
        raise LoginError("ZCode 登录超时，请重新发起")

    def _poll_once(self) -> dict | None:
        try:
            resp = requests.get(self.poll_url, headers=zquota.zai_headers(self._poll_token, self.mid),
                                timeout=20)
        except requests.RequestException:
            return None
        if resp.status_code >= 400:
            if resp.status_code in (408, 429):
                return None
            try:
                body = resp.json()
            except ValueError:
                body = {}
            if body.get("code") == 3004:
                raise LoginError("登录二维码/链接已过期，请重新发起")
            raise LoginError(f"登录轮询失败：HTTP {resp.status_code}")
        try:
            body = resp.json()
        except ValueError:
            return None
        if body.get("code") not in (0, None):
            raise LoginError(f"登录失败：{body.get('msg') or body.get('code')}")
        data = body.get("data") or {}
        status = str(data.get("status") or "")
        if status == "pending":
            return None
        if status == "failed":
            raise LoginError("授权被拒绝或登录失败，请重试")
        if status != "ready":
            raise LoginError("登录返回了未知状态")
        return data

    def _finish(self, data: dict) -> Account:
        jwt = str(data.get("token") or "").strip()
        node = data.get("zai") or data.get("bigmodel") or {}
        access = str(node.get("access_token") or node.get("accessToken") or "").strip()
        user = data.get("user") or {}
        if not jwt or not access:
            raise LoginError("登录返回的数据不完整，请重试")
        user_info = {key: user.get(key) for key in
                     ("id", "user_id", "username", "displayName", "email") if user.get(key)}
        if user.get("user_id") and "id" not in user_info:
            user_info["id"] = user["user_id"]
        if not any(user_info.get(key) for key in ("username", "displayName", "email")):
            extra = fetch_userinfo(self.provider, access)
            if extra:
                user_info = {**extra, **{k: v for k, v in user_info.items() if v}}
        credentials = {
            "zcodejwttoken": jwt,
            "oauth:active_provider": self.provider,
            f"oauth:{self.provider}:access_token": access,
            f"oauth:{self.provider}:user_info": json.dumps(user_info, ensure_ascii=False),
        }
        refresh = str(node.get("refresh_token") or node.get("refreshToken") or "").strip()
        if refresh:
            credentials[f"oauth:{self.provider}:refresh_token"] = refresh
        config = assemble_config(self.provider, jwt, access)
        ident = zcode.identity(credentials)
        name = zcode.label_of(ident) or "ZCode 账号"
        expires = zcode.jwt_expiry(credentials) or (datetime.now(timezone.utc) + timedelta(days=7))
        return Account(
            platform=zcode.PLATFORM,
            platform_label=zcode.LABEL,
            account_id=zcode.account_id_of(credentials),
            name=name,
            token=jwt,
            expires_at=expires,
            source="浏览器登录",
            meta={"credentials": credentials, "config": config,
                  "device_mid": self.mid, "provider": self.provider,
                  "email": ident.get("email") or ""},
            requires_checkin=False,
        )


def create_login(provider: str = "bigmodel", device_mid: str | None = None) -> ZCodeLogin:
    return ZCodeLogin(provider, device_mid)
