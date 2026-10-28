# -*- coding: utf-8 -*-
"""Qoder CN / Qoder 国际版：DPAPI 解出客户端凭证，读取额度并领取签到奖励。

两版的差异只有三处：API 基址、客户端配置目录、安装目录。
签到活动（/sash/api/v1/me/campaigns）从 2026-09-26 起要求请求携带
Cosy-Machine* 设备标识，否则服务端不会下发每日活动。设备标识不伪造，
直接运行 Qoder 客户端自带的 resources/umid/runtime-info.exe 获取。

签到的时机同样只认接口：服务端只下发当前这一轮的 startAt / endAt / claimStatus，
没有“下一轮排期”字段，客户端自己也是每 5 分钟轮询一次。所以下次可签时刻由
本轮窗口推算（startAt + 24 小时），实际开放时间以接口下发为准。
"""
from __future__ import annotations

import base64
import ctypes
import ctypes.wintypes as wt
import json
import os
import platform
import socket
import time
from datetime import datetime, timedelta
from pathlib import Path

import requests
from cryptography.hazmat.primitives.ciphers.aead import AESGCM

from ..core import winproc
from .base import (Account, ActionResult, CreditLot, CreditSnapshot, LogFn, Provider,
                   StatusResult, parse_iso)

API_CN = "https://openapi.qoder.com.cn"
API_INTL = "https://openapi.qoder.sh"
PROFILE_CN = Path(os.environ.get("APPDATA", "")) / "com.qodercn.app.stable"
PROFILE_INTL = Path(os.environ.get("APPDATA", "")) / "com.qoder.app.stable"
CLIENT_TYPE = "10"
CLIENT_VERSION = "0.3.4"
TIMEOUT = 20

_RISK_CACHE: dict[str, tuple[float, dict]] = {}
_RISK_TTL = 1800


class _Blob(ctypes.Structure):
    _fields_ = [("cbData", wt.DWORD), ("pbData", ctypes.POINTER(ctypes.c_char))]


def dpapi_unprotect(data: bytes) -> bytes:
    kernel32 = ctypes.windll.kernel32
    blob_in, blob_out = _Blob(), _Blob()
    blob_in.cbData = len(data)
    blob_in.pbData = ctypes.create_string_buffer(data)
    ok = ctypes.windll.crypt32.CryptUnprotectData(
        ctypes.byref(blob_in), None, None, None, None, 0, ctypes.byref(blob_out))
    if not ok:
        raise OSError(f"CryptUnprotectData 失败，错误码 {kernel32.GetLastError()}")
    try:
        return ctypes.string_at(blob_out.pbData, blob_out.cbData)
    finally:
        kernel32.LocalFree(blob_out.pbData)


def load_auth(path: Path | None = None) -> dict:
    path = Path(path) if path else PROFILE_CN / "auth.v1.dat"
    state = json.loads((path.parent / "Local State").read_text(encoding="utf-8"))
    raw_key = base64.b64decode(state["os_crypt"]["encrypted_key"])
    if raw_key[:5] != b"DPAPI":
        raise ValueError("encrypted_key 前缀异常")
    key = dpapi_unprotect(raw_key[5:])
    blob = path.read_bytes()
    if blob[:3] != b"v10":
        raise ValueError("auth.v1.dat 不是 v10 信封")
    return json.loads(AESGCM(key).decrypt(blob[3:15], blob[15:], None))


def _install_dir(intl: bool) -> Path | None:
    base = Path(os.environ.get("LOCALAPPDATA", "")) / "Programs"
    names = ("Qoder",) if intl else ("Qoder CN", "Qoder")
    for name in names:
        root = base / name
        if (root / "resources" / "umid" / "runtime-info.exe").exists():
            return root
    return None


def _machine_id(profile: Path) -> str:
    try:
        return (profile / "auth.machine-id").read_text(encoding="utf-8").strip()
    except OSError:
        return ""


def _risk_headers(intl: bool) -> dict:
    """用客户端自带组件生成设备风控头；失败时退回最小可用头。"""
    key = "intl" if intl else "cn"
    now = time.time()
    cached = _RISK_CACHE.get(key)
    if cached and now - cached[0] < _RISK_TTL:
        return dict(cached[1])

    headers: dict[str, str] = {"Cosy-ClientType": CLIENT_TYPE}
    root = _install_dir(intl)
    profile = PROFILE_INTL if intl else PROFILE_CN
    if root is not None:
        exe = root / "resources" / "umid" / "runtime-info.exe"
        try:
            proc = winproc.run_hidden([str(exe)], timeout=12)
            data = json.loads((proc.stdout or "").strip() or "{}")
        except Exception:
            data = {}
        arch = {"amd64": "x86_64", "x86_64": "x86_64",
                "aarch64": "aarch64", "arm64": "aarch64"}.get(
                    platform.machine().lower(), "x86_64")
        headers["Cosy-MachineOS"] = f"{arch}_windows"
        headers["Cosy-MachineHostname"] = socket.gethostname()
        try:
            manifest = json.loads(
                (root / "resources" / "build-manifest.json").read_text(encoding="utf-8"))
            version = str(manifest.get("productVersion") or "")
            if version:
                headers["Cosy-Version"] = version
        except Exception:
            headers.setdefault("Cosy-Version", CLIENT_VERSION)
        machine_id = _machine_id(profile)
        if machine_id:
            headers["Cosy-MachineId"] = machine_id
        for source, target in (("machineToken", "Cosy-MachineToken"),
                               ("machineCode", "Cosy-MachineCode"),
                               ("machineType", "Cosy-MachineType")):
            value = str(data.get(source) or "")
            if value:
                headers[target] = value

    _RISK_CACHE[key] = (now, headers)
    return dict(headers)


class _QoderBase(Provider):
    key = "qoder"
    label = "Qoder CN"
    base = API_CN
    profile = PROFILE_CN
    # 签到窗口由活动接口下发，本地不推算（滚动 24 小时、固定 10:00 都不成立）
    next_from_api = True
    intl = False

    def _accounts_from(self, auth: dict, source: Path) -> Account:
        token = auth.get("token")
        if not token:
            raise ValueError("登录态中没有 token")
        user = auth.get("user") or {}
        return Account(
            platform=self.key,
            platform_label=self.label,
            account_id=str(user.get("id") or token[-16:]),
            name=user.get("name") or user.get("email") or "Qoder 用户",
            token=token,
            expires_at=parse_iso(auth.get("expiresAt")),
            source=str(source),
            meta={
                "refresh_token": auth.get("refreshToken", ""),
                "refresh_expires_at": auth.get("refreshTokenExpiresAt"),
                "email": user.get("email", ""),
                "phone": user.get("phone", ""),
                "region": "intl" if self.intl else "cn",
                "profile": str(source.parent),
            },
            requires_checkin=True,
        )

    def scan(self, log: LogFn, extra_paths: tuple = ()) -> list[Account]:
        extras = [Path(path) for path in extra_paths]
        sources = [self.profile / "auth.v1.dat"] + extras
        accounts: list[Account] = []
        seen: set[str] = set()
        for source in sources:
            is_extra = source in extras
            if not source.exists():
                if is_extra:
                    log(f"[{self.label}] 未找到 {source}")
                continue
            try:
                if source.suffix.lower() == ".json":
                    data = json.loads(source.read_text(encoding="utf-8"))
                    entries = data if isinstance(data, list) else [data]
                else:
                    entries = [load_auth(source)]
                for auth in entries:
                    if not isinstance(auth, dict):
                        continue
                    account = self._accounts_from(auth, source)
                    if account.token in seen:
                        continue
                    seen.add(account.token)
                    accounts.append(account)
            except Exception as exc:
                log(f"[{self.label}] 解析 [{source.name}] 失败：{exc}")
        return accounts

    def _session(self, token: str) -> requests.Session:
        session = requests.Session()
        session.headers.update({
            "Accept": "application/json",
            "User-Agent": "Qoder",
            "Authorization": f"Bearer {token}",
            "Cosy-ClientType": CLIENT_TYPE,
            "Cosy-Version": CLIENT_VERSION,
        })
        session.headers.update(_risk_headers(self.intl))
        return session

    def _get(self, session: requests.Session, path: str) -> dict:
        resp = session.get(self.base + path, timeout=TIMEOUT)
        if resp.status_code == 401:
            raise RuntimeError("HTTP 401（登录态可能过期）")
        if resp.status_code != 200:
            raise RuntimeError(f"HTTP {resp.status_code}")
        try:
            return resp.json()
        except ValueError:
            raise RuntimeError(f"HTTP {resp.status_code}（返回内容不是 JSON）")

    def _campaigns(self, session: requests.Session) -> list[dict]:
        doc = self._get(session, "/sash/api/v1/me/campaigns")
        return doc.get("campaigns") or []

    @staticmethod
    def _benefits(campaigns: list[dict], now: float | None = None) -> list[dict]:
        """只看发币型活动，并限定在活动窗口内。

        VIEW_DETAILS 之类的横幅不是签到，不能拿它推断“今天已签”。
        """
        now = time.time() if now is None else now
        out = []
        for item in campaigns:
            if not isinstance(item, dict) or item.get("actionType") != "CLAIM_BENEFIT":
                continue
            start = item.get("startAt")
            end = item.get("endAt")
            if isinstance(start, (int, float)) and now < start:
                continue
            if isinstance(end, (int, float)) and now >= end:
                continue
            out.append(item)
        return out

    # 每日签到窗口正好 23 小时 59 分（10:00:00 → 次日 09:59:00），
    # 一次性活动不会是这个长度，靠它把两类发币活动区分开。
    _DAILY_SPAN = (86300, 86400)

    @classmethod
    def _daily(cls, benefits: list[dict]) -> dict | None:
        """当前窗口里的每日签到活动；只有一次性活动时返回 None。"""
        for item in benefits:
            start = item.get("startAt")
            end = item.get("endAt")
            if not isinstance(start, (int, float)) or not isinstance(end, (int, float)):
                continue
            if cls._DAILY_SPAN[0] <= end - start <= cls._DAILY_SPAN[1]:
                return item
        return None

    @staticmethod
    def _opened_at(campaign: dict) -> datetime | None:
        """活动窗口的开放时刻（本地 naive）。"""
        moment = parse_iso(campaign.get("startAt"))
        return moment.astimezone().replace(tzinfo=None) if moment else None

    @classmethod
    def _next_window(cls, benefits: list[dict]) -> datetime | None:
        """本轮已签后，下一次可签的本地时刻。

        接口只下发当前窗口，没有“下一轮排期”字段（客户端也是 5 分钟轮询等它出现），
        所以按同一节奏用 startAt + 24 小时推算；实际开放时间会漂，以接口下发为准。
        """
        daily = cls._daily(benefits)
        opened = parse_iso(daily.get("startAt")) if daily else None
        if opened is None:
            return None
        # 先在 UTC 上加一天再换本地时区，跨夏令时也不会偏一小时
        return (opened + timedelta(hours=24)).astimezone().replace(tzinfo=None)

    def _quota(self, session: requests.Session) -> tuple[str, float | None, list[CreditLot]]:
        payload = self._get(session, "/api/v2/quota/usage")
        if isinstance(payload.get("qoderUsage"), dict):
            payload = payload["qoderUsage"]

        parts: list[str] = []
        lots: list[CreditLot] = []
        balance = 0.0
        counted = False
        plan_expiry = parse_iso(payload.get("expiresAt"))
        if plan_expiry and plan_expiry.year >= 2100:
            plan_expiry = None

        for label, key in (("套餐", "userQuota"), ("资源包", "addOnQuota")):
            item = payload.get(key) or {}
            total = _number(item.get("total")) or 0.0
            remaining = _number(item.get("remaining"))
            if remaining is None:
                continue
            remaining = max(0.0, remaining)
            if total or remaining:
                parts.append(f"{label} {remaining:g}/{total:g}")
                balance += remaining
                counted = True
            if remaining > 0:
                expires = plan_expiry if key == "userQuota" else None
                lots.append(CreditLot(remaining, expires, label))

        # 某些企业/组织账号会返回独立资源包，带上各自的到期时间。
        for item in payload.get("dedicatedResourcePackages") or []:
            if not isinstance(item, dict):
                continue
            remaining = _number(item.get("remaining"))
            if remaining is None or remaining <= 0:
                continue
            expires = parse_iso(item.get("expiresAt") or item.get("endTime"))
            lots.append(CreditLot(remaining, expires,
                                  str(item.get("name") or "资源包")))
            balance += remaining
            counted = True

        if payload.get("isQuotaExceeded"):
            parts.append("已超额")
        summary = "，".join(parts) or "无可用额度"
        return summary, (balance if counted else None), lots

    def credits(self, account: Account, log: LogFn) -> CreditSnapshot:
        # 只拉额度接口，不请求 campaigns，因此不会查询签到状态。
        summary, balance, lots = self._quota(self._session(account.token))
        return CreditSnapshot(summary, balance, lots)

    def status(self, account: Account, log: LogFn) -> StatusResult:
        session = self._session(account.token)
        summary, balance, lots = self._quota(session)

        checked: bool | None = None
        next_at: datetime | None = None
        try:
            benefits = self._benefits(self._campaigns(session))
            pending = [c for c in benefits if c.get("claimStatus") == "CLAIMABLE"]
            daily = self._daily(benefits)
            today = datetime.now().date()
            if pending:
                checked = False
                summary += f"；可领 {len(pending)} 项"
            elif daily is not None:
                opened = self._opened_at(daily)
                claim_status = daily.get("claimStatus")
                if claim_status == "CLAIMABLE":
                    checked = False
                elif claim_status == "CLAIMED" and opened is not None:
                    if opened.date() == today:
                        checked = True
                        next_at = self._next_window(benefits)
                    elif opened.date() < today:
                        # 上次签的还是昨天 10:00 那一轮，今天这轮还没下发：
                        # “已领取”只能说明昨天领过，不能说今天已签
                        checked = False
                        next_at = self._next_window(benefits)
            # 其余情况保持 None：活动未下发或状态异常，都不猜“今天已签”。
        except Exception:
            pass
        return StatusResult(checked, summary, balance, lots, next_at)

    def checkin(self, account: Account, log: LogFn) -> ActionResult:
        session = self._session(account.token)
        campaigns = self._campaigns(session)
        benefits = self._benefits(campaigns)
        targets = [c for c in benefits if c.get("claimStatus") == "CLAIMABLE"]
        if not targets:
            daily = self._daily(benefits)
            opened = self._opened_at(daily) if daily else None
            if opened is not None and opened.date() < datetime.now().date():
                nxt = self._next_window(benefits)
                hint = f"{nxt:%H:%M}" if nxt else "10:00"
                return ActionResult(True, f"本轮已领取，今日 {hint} 后开放下一轮",
                                    already=True)
            if benefits:
                return ActionResult(True, "今日已领取", already=True)
            return ActionResult(False, "签到活动未下发（通常 10:00 UTC+8 后开放）")

        claimed = 0
        granted = 0.0
        validity_days: int | None = None
        failed: list[str] = []
        for campaign in targets:
            campaign_id = campaign.get("campaignId")
            benefit = campaign.get("benefit") or {}
            resp = session.post(
                f"{self.base}/sash/api/v1/me/campaigns/{campaign_id}/claim",
                timeout=TIMEOUT)
            try:
                body = resp.json()
            except ValueError:
                body = {}
            data = body.get("data") if isinstance(body.get("data"), dict) else body
            ok = resp.status_code == 200 and isinstance(data, dict) and (
                data.get("status") == "CLAIMED" or data.get("replayed"))
            if ok:
                claimed += 1
                if not data.get("replayed"):
                    amount = benefit.get("amount")
                    if isinstance(amount, (int, float)):
                        granted += float(amount)
                    validity = benefit.get("validity") or {}
                    if validity.get("mode") == "RELATIVE_DAYS" and validity.get("days"):
                        validity_days = int(validity["days"])
            else:
                failed.append(str((data or {}).get("message")
                                  or body.get("message")
                                  or f"HTTP {resp.status_code}"))

        if failed:
            return ActionResult(claimed > 0, f"领取 {claimed}/{len(targets)} 项，失败：{failed[0]}",
                                granted=granted or None, validity_days=validity_days)
        try:
            summary = self.credits(account, log).summary
        except Exception:
            summary = ""
        text = f"领取 {claimed} 项，+{granted:g} 积分" if granted else f"领取 {claimed} 项"
        return ActionResult(True, f"{text}，{summary}" if summary else text,
                            granted=granted or None, validity_days=validity_days)

    def client_exe(self, account: Account | None = None) -> Path | None:
        root = _install_dir(self.intl)
        if root is None:
            return None
        base = Path(os.environ.get("LOCALAPPDATA", "")) / "Programs"
        name = root.name
        exe = base / name / f"{name}.exe"
        return exe if exe.exists() else None


class QoderProvider(_QoderBase):
    key = "qoder"
    label = "Qoder CN"
    base = API_CN
    profile = PROFILE_CN
    intl = False


class QoderIntlProvider(_QoderBase):
    key = "qoder_intl"
    label = "Qoder 国际版"
    base = API_INTL
    profile = PROFILE_INTL
    intl = True


def _number(value) -> float | None:
    try:
        return float(value)
    except (TypeError, ValueError):
        return None
