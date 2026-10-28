# -*- coding: utf-8 -*-
"""三个平台的浏览器登录流程。

打开官方登录页，轮询 / 接收回调，拿回 token 与账号信息，写入本机账号库。
协议参考 magpie-community/plugins（MIT）与 magpie 的 WorkBuddy / Qoder / Trae provider。
"""
from __future__ import annotations

import base64
import hashlib
import json
import secrets
import threading
import time
import uuid
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlencode, urlparse

import requests

from . import store
from .base import Account, parse_iso


class LoginError(Exception):
    pass


def _b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).decode("ascii").rstrip("=")


def _json_obj(value) -> dict:
    if not value:
        return {}
    try:
        data = json.loads(value)
    except (ValueError, TypeError):
        return {}
    return data if isinstance(data, dict) else {}


def _number(value):
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return value
    if isinstance(value, str) and value.strip().isdigit():
        return int(value.strip())
    return None


def _expiry_at(ms_value, seconds_value):
    ms = _number(ms_value)
    if ms and ms > 0:
        return datetime.fromtimestamp(ms / 1000, tz=timezone.utc)
    sec = _number(seconds_value)
    if sec and sec > 0:
        return datetime.now(timezone.utc) + timedelta(seconds=sec)
    return None


def _expiry_from_ms(value):
    ms = _number(value)
    if ms and ms > 0:
        return datetime.now(timezone.utc) + timedelta(milliseconds=ms)
    return None


def _device_expiry(data: dict):
    expires = parse_iso(data.get("expires_at"))
    if expires:
        return expires
    sec = _number(data.get("expires_in"))
    if sec and sec > 0:
        return datetime.now(timezone.utc) + timedelta(seconds=sec)
    return datetime.now(timezone.utc) + timedelta(days=1)


def _fallback_id(token: str) -> str:
    return "acct-" + hashlib.sha1(token.encode("utf-8")).hexdigest()[:12]


class LoginFlow:
    platform = ""
    url = ""

    def poll(self, stop: threading.Event) -> Account:
        raise NotImplementedError

    def cancel(self) -> None:
        pass


# ---------------------------------------------------------------- WorkBuddy
WB_ENDPOINT_CN = "https://copilot.tencent.com"
WB_ENDPOINT_INTL = "https://www.workbuddy.ai"
WB_ENDPOINT = WB_ENDPOINT_CN  # 兼容旧引用
WB_UA = "WorkBuddy/5.5.6"
WB_APP_VERSION = "2.0.0"
WB_LOGIN_TIMEOUT = 5 * 60


class WorkBuddyLogin(LoginFlow):
    platform = "workbuddy"

    def __init__(self, edition: str = "cn") -> None:
        self._intl = edition == "intl"
        self._endpoint = WB_ENDPOINT_INTL if self._intl else WB_ENDPOINT_CN
        headers = {
            "User-Agent": WB_UA,
            "X-No-Authorization": "true",
            "X-No-User-Id": "true",
            "X-No-Enterprise-Id": "true",
            "X-No-Department-Info": "true",
        }
        try:
            resp = requests.post(f"{self._endpoint}/v2/plugin/auth/state",
                                 params={"platform": "workbuddy"}, headers=headers,
                                 json={}, timeout=20)
            body = resp.json()
        except (requests.RequestException, ValueError) as exc:
            raise LoginError(f"获取 WorkBuddy 登录链接失败：{exc}")
        if resp.status_code >= 400:
            raise LoginError(f"获取 WorkBuddy 登录链接失败：HTTP {resp.status_code}")
        if int(body.get("code") or 0) != 0:
            raise LoginError(f"获取 WorkBuddy 登录链接失败：{body.get('msg') or body.get('code')}")
        data = body.get("data") or {}
        self._state = str(data.get("state") or "")
        auth_url = str(data.get("authUrl") or "")
        if not self._state or not auth_url:
            raise LoginError("WorkBuddy 没有返回登录地址")
        sep = "&" if "?" in auth_url else "?"
        self.url = (f"{auth_url}{sep}version={WB_APP_VERSION}"
                    f"&loginSessionId={secrets.token_hex(16)}")

    def poll(self, stop: threading.Event) -> Account:
        deadline = time.time() + WB_LOGIN_TIMEOUT
        token_data = None
        while time.time() < deadline and not stop.is_set():
            try:
                resp = requests.get(f"{self._endpoint}/v2/plugin/auth/token",
                                    params={"state": self._state},
                                    headers={"User-Agent": WB_UA}, timeout=20)
                body = resp.json()
                code = int(body.get("code") or 0)
                if code == 11217 or resp.status_code in (408, 429):
                    pass
                elif code == 0 and isinstance(body.get("data"), dict) \
                        and body["data"].get("accessToken"):
                    token_data = body["data"]
                    break
                else:
                    raise LoginError(f"WorkBuddy 登录失败：{body.get('msg') or code}")
            except (requests.RequestException, ValueError):
                pass
            stop.wait(1)
        if stop.is_set():
            raise LoginError("已取消")
        if not token_data:
            raise LoginError("WorkBuddy 登录超时")

        access = str(token_data["accessToken"])
        refresh = str(token_data.get("refreshToken") or "")
        domain = str(token_data.get("domain") or "")
        expires = _expiry_at(token_data.get("expiresAt"), token_data.get("expiresIn"))

        who: dict = {}
        account_headers = {
            "Authorization": f"Bearer {access}",
            "X-Domain": domain,
            "X-No-User-Id": "true",
            "X-No-Enterprise-Id": "true",
            "User-Agent": WB_UA,
        }
        who_deadline = time.time() + WB_LOGIN_TIMEOUT
        while time.time() < who_deadline and not stop.is_set():
            try:
                resp = requests.get(f"{self._endpoint}/v2/plugin/login/account",
                                    params={"state": self._state},
                                    headers=account_headers, timeout=20)
                body = resp.json()
                code = int(body.get("code") or 0)
                if code == 12151:
                    pass
                elif code == 0 and isinstance(body.get("data"), dict):
                    who = body["data"]
                    break
                else:
                    raise LoginError(f"读取 WorkBuddy 账号失败：{body.get('msg') or code}")
            except (requests.RequestException, ValueError):
                pass
            stop.wait(1)

        uid = str(who.get("uid") or "")
        name = str(who.get("nickname") or who.get("phoneNumber") or uid or "WorkBuddy 账号")
        # 与客户端扫描保持同一主键规则：国际版加 @intl 后缀，避免同 uid 两版冲突
        account_id = f"{uid}@intl" if (self._intl and uid) else (uid or _fallback_id(access))
        return Account(
            platform=self.platform,
            platform_label="WorkBuddy 国际版" if self._intl else "WorkBuddy 国内版",
            account_id=account_id,
            name=name,
            token=access,
            expires_at=expires,
            source="浏览器登录",
            meta={
                "refresh_token": refresh,
                "domain": domain,
                "uid": uid,
                "edition": "intl" if self._intl else "cn",
                "enterprise_id": str(who.get("enterpriseId") or ""),
                "account_info": who,
            },
            requires_checkin=not self._intl,
        )


# --------------------------------------------------------------------- Qoder
QODER_WEB = "https://qoder.cn"
QODER_OPENAPI = "https://openapi.qoder.com.cn"
QODER_INTL_OPENAPI = "https://openapi.qoder.sh"
QODER_CLIENT_ID = "e883ade2-e6e3-4d6d-adf7-f92ceff5fdcb"
QODER_LOGIN_TIMEOUT = 15 * 60


class QoderLogin(LoginFlow):
    platform = "qoder"

    def __init__(self) -> None:
        self._verifier = _b64url(secrets.token_bytes(64))
        challenge = _b64url(hashlib.sha256(self._verifier.encode("ascii")).digest())
        self._nonce = str(uuid.uuid4())
        self._machine_id = str(uuid.uuid4())
        params = {
            "challenge": challenge,
            "challenge_method": "S256",
            "nonce": self._nonce,
            "machine_id": self._machine_id,
            "client_id": QODER_CLIENT_ID,
        }
        self.url = f"{QODER_WEB}/device/selectAccounts?{urlencode(params)}"

    def poll(self, stop: threading.Event) -> Account:
        deadline = time.time() + QODER_LOGIN_TIMEOUT
        device = None
        while time.time() < deadline and not stop.is_set():
            try:
                resp = requests.get(f"{QODER_OPENAPI}/api/v1/deviceToken/poll",
                                    params={"nonce": self._nonce,
                                            "verifier": self._verifier,
                                            "challenge_method": "S256"}, timeout=20)
                if resp.status_code == 200:
                    body = resp.json()
                    if str(body.get("token") or "").strip():
                        device = body
                        break
            except (requests.RequestException, ValueError):
                pass
            stop.wait(2)
        if stop.is_set():
            raise LoginError("已取消")
        if not device:
            raise LoginError("Qoder 登录超时")

        access, refresh = "", ""
        expires = None
        try:
            job = requests.post(f"{QODER_OPENAPI}/api/v1/me/jobToken",
                                headers={"Authorization": f"Bearer {device['token']}",
                                         "Content-Type": "application/json"},
                                json={"clientId": QODER_CLIENT_ID}, timeout=20)
            if job.status_code == 200:
                data = job.json()
                access = str(data.get("token") or "")
                refresh = str(data.get("refresh_token") or "")
                if not access:
                    raise LoginError("Qoder 返回的登录凭证为空")
                expires = _expiry_from_ms(data.get("expires_in"))
            elif 400 <= job.status_code < 500:
                # Qoder CN 的 CLI 不发 job token，设备 token 本身就能用
                access = str(device["token"])
                refresh = str(device.get("refresh_token") or "")
                expires = _device_expiry(device)
            else:
                raise LoginError(f"换取 Qoder 登录凭证失败：HTTP {job.status_code}")
        except (requests.RequestException, ValueError) as exc:
            raise LoginError(f"换取 Qoder 登录凭证失败：{exc}")

        uid = str(device.get("user_id") or "")
        name = str(device.get("user_name") or "")
        email = ""
        try:
            info = requests.get(f"{QODER_OPENAPI}/api/v1/userinfo",
                                headers={"Authorization": f"Bearer {device['token']}"},
                                timeout=20)
            if 200 <= info.status_code < 300:
                user = info.json()
                email = str(user.get("email") or "")
                name = str(user.get("name") or name)
                uid = str(user.get("id") or user.get("user_id") or user.get("uid") or uid)
        except (requests.RequestException, ValueError):
            pass

        return Account(
            platform=self.platform,
            platform_label="Qoder CN",
            account_id=uid or email or _fallback_id(access),
            name=name or email or uid or "Qoder 账号",
            token=access,
            expires_at=expires,
            source="浏览器登录",
            meta={
                "refresh_token": refresh,
                "machine_id": self._machine_id,
                "device_token": str(device.get("token") or ""),
                "device_refresh": str(device.get("refresh_token") or ""),
                "email": email,
            },
        )


# ---------------------------------------------------------------------- Trae
TRAE_WEB = "https://www.trae.cn"
TRAE_AUTH = "https://api.trae.cn"
TRAE_CLIENT_ID = "ono9krqynydwx5"
TRAE_PLUGIN_VERSION = "2.3.24254"
TRAE_IDE_VERSION = "3.3.65"
TRAE_BRAND = "ASUS TUF Gaming A15 FA507RM_FA507RM"
TRAE_LOGIN_TIMEOUT = 10 * 60


class TraeLogin(LoginFlow):
    platform = "trae"

    def __init__(self) -> None:
        self._device_id = str(secrets.randbelow(9) + 1) + "".join(
            str(secrets.randbelow(10)) for _ in range(15))
        self._machine_id = secrets.token_hex(16)
        self._event = threading.Event()
        self._query: dict = {}
        outer = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def _cors(self):
                self.send_header("Access-Control-Allow-Origin", "*")
                self.send_header("Access-Control-Allow-Methods", "GET, OPTIONS")
                self.send_header("Access-Control-Allow-Headers", "Content-Type")

            def do_OPTIONS(self):
                self.send_response(204)
                self._cors()
                self.end_headers()

            def do_GET(self):
                parsed = urlparse(self.path)
                if parsed.path != "/authorize":
                    self.send_error(404)
                    return
                outer._query = {key: value[0]
                                for key, value in parse_qs(parsed.query).items()}
                outer._event.set()
                body = ("<html><meta charset='utf-8'><body style='font-family:sans-serif;"
                        "padding:40px'>登录已完成，可以关闭此页面并回到 AI Manager。</body></html>")
                self.send_response(200)
                self._cors()
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                self.wfile.write(body.encode("utf-8"))

        self._server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()
        port = self._server.server_address[1]
        params = {
            "login_version": "1",
            "auth_from": "solo",
            "login_channel": "native_ide",
            "plugin_version": TRAE_PLUGIN_VERSION,
            "auth_type": "local",
            "client_id": TRAE_CLIENT_ID,
            "redirect": "0",
            "login_trace_id": str(uuid.uuid4()),
            "auth_callback_url": f"http://127.0.0.1:{port}/authorize",
            "machine_id": self._machine_id,
            "device_id": self._device_id,
            "x_device_id": self._device_id,
            "x_machine_id": self._machine_id,
            "x_device_brand": TRAE_BRAND,
            "x_device_type": "windows",
            "x_os_version": "Windows 10 Pro",
            "x_env": "",
            "x_app_version": TRAE_IDE_VERSION,
            "x_app_type": "stable",
            "hide_saas_login": "false",
        }
        self.url = f"{TRAE_WEB}/authorization?{urlencode(params)}"

    def poll(self, stop: threading.Event) -> Account:
        deadline = time.time() + TRAE_LOGIN_TIMEOUT
        while time.time() < deadline and not stop.is_set():
            if self._event.wait(0.5):
                break
        if stop.is_set():
            raise LoginError("已取消")
        if not self._event.is_set():
            raise LoginError("Trae 登录超时")

        query = self._query
        if query.get("error"):
            raise LoginError(str(query.get("error_description") or query["error"]))
        jwt = _json_obj(query.get("userJwt"))
        info = _json_obj(query.get("userInfo"))
        token = str(jwt.get("Token") or jwt.get("token") or "")
        refresh = str(jwt.get("RefreshToken") or jwt.get("refreshToken")
                      or query.get("refreshToken") or "")
        expires = parse_iso(jwt.get("TokenExpireAt") or jwt.get("tokenExpireAt"))
        uid = str(info.get("UserID") or info.get("userId") or query.get("userId") or "")
        name = str(info.get("ScreenName") or info.get("screenName") or uid)

        if not token and refresh:
            token, refresh, expires = self._exchange(refresh)
        if not token:
            raise LoginError("Trae 没有返回登录凭证")

        return Account(
            platform=self.platform,
            platform_label="Trae CN",
            account_id=uid or _fallback_id(token),
            name=name or uid or "Trae 账号",
            token=token,
            expires_at=expires,
            source="浏览器登录",
            meta={
                "refresh_token": refresh,
                "device_id": self._device_id,
                "machine_id": self._machine_id,
            },
        )

    @staticmethod
    def _exchange(refresh: str):
        try:
            resp = requests.post(
                f"{TRAE_AUTH}/cloudide/api/v3/trae/oauth/ExchangeToken",
                json={"ClientID": TRAE_CLIENT_ID, "RefreshToken": refresh,
                      "ClientSecret": "-", "UserID": ""}, timeout=20)
            body = resp.json()
        except (requests.RequestException, ValueError) as exc:
            raise LoginError(f"换取 Trae 登录凭证失败：{exc}")
        result = body.get("Result") or body.get("result") or body
        if not isinstance(result, dict):
            raise LoginError("换取 Trae 登录凭证失败：返回格式异常")
        token = str(result.get("Token") or result.get("token") or "")
        new_refresh = str(result.get("RefreshToken") or result.get("refreshToken") or refresh)
        expires = parse_iso(result.get("TokenExpireAt") or result.get("tokenExpireAt"))
        if not token:
            raise LoginError("换取 Trae 登录凭证失败：token 为空")
        return token, new_refresh, expires

    def cancel(self) -> None:
        try:
            self._server.shutdown()
        except Exception:
            pass


def create_login(platform: str) -> LoginFlow:
    if platform == "workbuddy":
        return WorkBuddyLogin("cn")
    if platform == "workbuddy_intl":
        return WorkBuddyLogin("intl")
    if platform == "qoder":
        return QoderLogin()
    if platform == "trae":
        return TraeLogin()
    raise LoginError(f"不支持的平台：{platform}")


# ------------------------------------------------------------------- 续期
def _refresh_workbuddy(account: Account, refresh: str) -> dict:
    base = "https://www.workbuddy.ai" if str(
        account.meta.get("domain") or "").endswith(".ai") else WB_ENDPOINT
    headers = {
        "X-Refresh-Token": refresh,
        "X-Auth-Refresh-Source": "plugin",
        "X-Domain": account.meta.get("domain", ""),
        "User-Agent": WB_UA,
        "Content-Type": "application/json",
    }
    resp = requests.post(f"{base}/v2/plugin/auth/token/refresh",
                         headers=headers, json={}, timeout=20)
    body = resp.json()
    if int(body.get("code") or 0) != 0:
        raise RuntimeError(str(body.get("msg") or "刷新被拒绝"))
    data = body.get("data") or {}
    if not data.get("accessToken"):
        raise RuntimeError("没有返回新 token")
    return {
        "token": str(data["accessToken"]),
        "refresh": str(data.get("refreshToken") or refresh),
        "expires": _expiry_at(data.get("expiresAt"), data.get("expiresIn")),
    }


def _refresh_qoder(refresh: str, base: str = QODER_OPENAPI) -> dict:
    resp = requests.post(f"{base}/api/v1/jobToken/refresh",
                         json={"refresh_token": refresh}, timeout=20)
    if resp.status_code in (401, 403):
        raise RuntimeError("refresh token 已失效，请重新登录")
    if resp.status_code != 200:
        raise RuntimeError(f"HTTP {resp.status_code}")
    data = resp.json()
    token = str(data.get("token") or "")
    new_refresh = str(data.get("refresh_token") or "")
    if not token or not new_refresh:
        raise RuntimeError("返回的凭证不完整")
    return {"token": token, "refresh": new_refresh,
            "expires": _expiry_from_ms(data.get("expires_in"))}


def ensure_fresh(account: Account, log) -> Account:
    """浏览器登录的账号在过期前 5 分钟自动续期；客户端扫码来的登录态不动。"""
    if not account.meta.get("stored"):
        return account
    now = datetime.now(timezone.utc)
    if account.expires_at and account.expires_at - now > timedelta(minutes=5):
        return account
    refresh = account.meta.get("refresh_token") or ""
    if not refresh:
        return account
    try:
        if account.platform == "workbuddy":
            data = _refresh_workbuddy(account, refresh)
        elif account.platform in ("qoder", "qoder_intl"):
            base = QODER_INTL_OPENAPI if account.platform == "qoder_intl" else QODER_OPENAPI
            data = _refresh_qoder(refresh, base)
        else:
            return account
    except Exception as exc:
        log(f"[{account.platform_label}] 登录态续期失败：{exc}")
        return account
    account.token = data["token"]
    if data.get("refresh"):
        account.meta["refresh_token"] = data["refresh"]
    if data.get("expires"):
        account.expires_at = data["expires"]
    store.upsert(store.to_item(account))
    log(f"[{account.platform_label}] {account.name} 登录态已续期")
    return account
