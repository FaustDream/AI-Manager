# -*- coding: utf-8 -*-
"""WorkBuddy：解密 $wbEncrypted 登录态并调用积分接口。"""
from __future__ import annotations

import base64
import hashlib
import json
import os
import re
import struct
from datetime import datetime, timezone
from pathlib import Path

import requests
from cryptography.hazmat.primitives.ciphers.aead import AESGCM

from ..sessions.editions import Edition
from .base import (Account, ActionResult, CreditLot, CreditSnapshot, LogFn, Provider,
                   StatusResult, parse_iso)

API_BASE_CN = "https://copilot.tencent.com/v2/billing/meter"
API_BASE_INTL = "https://www.workbuddy.ai/v2/billing/meter"
# 兼容旧引用；实际请求会按账号域名选择基址。
API_BASE = API_BASE_CN
AUTH_DIR = Path(os.environ.get("LOCALAPPDATA", "")) / "CodeBuddyExtension" / "Data" / "Public" / "auth"

_AT_REST_SECRET = "Sik9U5aXhCdwTVEwsEySDOmDoB9r9ntFxHF1fst9LQI="
_WB_KEY = hashlib.sha256(_AT_REST_SECRET.encode("utf-8")).digest()
_WB_KEY_ID = hashlib.sha256(_WB_KEY).hexdigest()[:16]

_FRAMING_NUM = {"file": 1, "field": 2, "record": 3, "stream": 4}
_FRAMING_TAG = {"file": b"WBEF1", "field": b"WBEV1", "record": b"WBER1", "stream": b"WBES1"}

# 带时间戳的 .info 是历史备份，不参与扫描
_BACKUP_RE = re.compile(r"\.\d{4}-\d{2}-\d{2}T")
_UA = "Mozilla/5.0 CodeBuddy/5.6.0"


def _parse_local(value) -> datetime | None:
    if not value:
        return None
    try:
        return datetime.strptime(str(value), "%Y-%m-%d %H:%M:%S").astimezone()
    except ValueError:
        return None


def _number(value) -> float | None:
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


def _is_intl(account: Account) -> bool:
    return (account.meta.get("edition") == "intl"
            or str(account.meta.get("domain") or "").endswith(".ai"))


def _api_base(account: Account) -> str:
    return API_BASE_INTL if _is_intl(account) else API_BASE_CN


def _aad(framing: str, key_id: str, suite: int) -> bytes:
    return b"".join([
        b"WB-AAD\x00", b"\x01",
        struct.pack(">I", len(_FRAMING_TAG[framing])) + _FRAMING_TAG[framing],
        struct.pack(">I", 6) + b"sym-v1",
        struct.pack(">I", suite),
        struct.pack(">I", len(key_id)) + key_id.encode(),
        bytes([_FRAMING_NUM[framing]]),
        b"\x00",
        b"\x00",
    ])


def open_field(value):
    if not isinstance(value, dict) or value.get("$wbEncrypted") != 1:
        return value
    if value.get("scheme") not in (None, "sym-v1"):
        raise ValueError(f"不支持的加密方案 {value.get('scheme')}")
    envelope = json.loads(base64.b64decode(value["envelope"]))
    if envelope.get("keyId") != _WB_KEY_ID:
        raise ValueError("密钥不匹配，WorkBuddy 版本可能已更新")
    cipher = AESGCM(_WB_KEY)
    return cipher.decrypt(
        base64.b64decode(envelope["nonce"]),
        base64.b64decode(envelope["ciphertext"]) + base64.b64decode(envelope["authTag"]),
        _aad("field", envelope["keyId"], envelope["suite"]),
    ).decode("utf-8")


def seal_field(value: str) -> dict:
    """把明文封装成 WorkBuddy 的 $wbEncrypted 字段，供写回客户端登录态用。"""
    nonce = os.urandom(16)
    sealed = AESGCM(_WB_KEY).encrypt(
        nonce, str(value).encode("utf-8"), _aad("field", _WB_KEY_ID, 1))
    envelope = {
        "suite": 1,
        "keyId": _WB_KEY_ID,
        "nonce": base64.b64encode(nonce).decode("ascii"),
        "authTag": base64.b64encode(sealed[-16:]).decode("ascii"),
        "ciphertext": base64.b64encode(sealed[:-16]).decode("ascii"),
    }
    return {
        "$wbEncrypted": 1,
        "envelope": base64.b64encode(
            json.dumps(envelope, ensure_ascii=False).encode("utf-8")).decode("ascii"),
    }


class WorkBuddyProvider(Provider):
    key = "workbuddy"
    label = "WorkBuddy 国内版"
    live_credit = True

    def scan(self, log: LogFn, extra_paths: tuple = ()) -> list[Account]:
        extras = [Path(path) for path in extra_paths]
        files: list[Path] = []
        if AUTH_DIR.is_dir():
            files = [p for p in AUTH_DIR.glob("*.info") if not _BACKUP_RE.search(p.name)]
        elif not extras:
            log("[WorkBuddy] 未找到登录态目录，请先打开客户端并登录")
            return []
        for path in extras:
            if not path.exists():
                log(f"[WorkBuddy] 未找到 {path}")
            elif path.suffix.lower() != ".info":
                log(f"[WorkBuddy] 需要 .info 文件：{path.name}")
            else:
                files.append(path)

        try:
            files.sort(key=lambda p: p.stat().st_mtime, reverse=True)
        except OSError:
            pass

        accounts: list[Account] = []
        seen: set[tuple[str, str]] = set()
        for path in files:
            try:
                session = json.loads(path.read_text(encoding="utf-8"))
                auth = session.get("auth") or {}
                account = session.get("account") or {}
                uid = str(account.get("uid") or "")
                domain = str(auth.get("domain") or "")
                intl = domain.endswith(".ai") or path.name.endswith("-ai.info")
                token = open_field(auth.get("accessToken"))
                if not token or not uid:
                    log(f"[WorkBuddy] [{path.stem}] 登录态不完整（token 或 uid 缺失）")
                    continue
                if (domain, uid) in seen:
                    continue
                seen.add((domain, uid))

                nickname = open_field(account.get("nickname"))
                phone = open_field(account.get("phoneNumber"))
                refresh = ""
                try:
                    refresh = open_field(auth.get("refreshToken")) or ""
                except Exception:
                    pass
                # 国际版和国内版共用“workbuddy”平台键，靠 edition/domain 区分；
                # 加后缀避免同一 uid 在两个版本同时登录时主键冲突。
                account_id = f"{uid}@intl" if intl else uid
                accounts.append(Account(
                    platform=self.key,
                    platform_label="WorkBuddy 国际版" if intl else "WorkBuddy 国内版",
                    account_id=account_id,
                    name=str(nickname or phone or "未命名账号"),
                    token=token,
                    expires_at=parse_iso(auth.get("expiresAt")),
                    source=str(path),
                    meta={"domain": domain, "uid": uid,
                          "login_file": str(path), "edition": "intl" if intl else "cn",
                          "refresh_token": refresh, "account_info": account,
                          "phone": str(phone or "")},
                    requires_checkin=not intl,
                ))
            except Exception as exc:
                log(f"[WorkBuddy] 解析 [{path.name}] 失败：{exc}")
        return accounts

    @staticmethod
    def _headers(account: Account) -> dict:
        return {
            "Authorization": f"Bearer {account.token}",
            "X-User-Id": account.meta.get("uid", ""),
            "X-Domain": account.meta.get("domain", ""),
            "Content-Type": "application/json",
            "User-Agent": _UA,
        }

    @classmethod
    def _post(cls, path: str, account: Account) -> dict:
        headers = cls._headers(account)
        resp = requests.post(f"{_api_base(account)}/{path}", headers=headers,
                             json={}, timeout=20)
        if resp.status_code == 401:
            raise RuntimeError("HTTP 401（登录态可能过期）")
        if resp.status_code >= 500:
            raise RuntimeError(f"HTTP {resp.status_code}")
        try:
            return resp.json()
        except ValueError:
            raise RuntimeError(f"HTTP {resp.status_code}（返回内容不是 JSON）")

    @classmethod
    def _activity_status(cls, account: Account) -> dict | None:
        """新版签到看板；比旧 checkin-status 可靠，活动未开放时也不会全 0。"""
        resp = requests.post(f"{_api_base(account)}/checkin-activity-status",
                             headers=cls._headers(account), json={}, timeout=20)
        if resp.status_code == 404:
            return None
        if resp.status_code == 401:
            raise RuntimeError("HTTP 401（登录态可能过期）")
        if resp.status_code >= 400:
            return None
        try:
            body = resp.json()
        except ValueError:
            return None
        if body.get("code") == 0 and isinstance(body.get("data"), dict):
            return body["data"]
        return None

    def requires_checkin(self, account: Account) -> bool:
        return not _is_intl(account)

    def _resource_lots(self, account: Account) -> list[CreditLot]:
        # 不要带 PackageEndTimeRange：部分账号的活跃资源包不在这个区间里，
        # 服务端会只返回历史包，看起来就是“查不到积分”。
        body = {
            "PageNumber": 1,
            "PageSize": 200,
            "ProductCode": "p_tcaca",
            "Status": [0, 3],
        }
        resp = requests.post(f"{_api_base(account)}/get-user-resource",
                             headers=self._headers(account), json=body, timeout=20)
        if resp.status_code >= 400:
            raise RuntimeError(f"HTTP {resp.status_code}")
        payload = resp.json()
        data = ((payload.get("data") or {}).get("Response") or {}).get("Data") \
            or (payload.get("data") or {}).get("Data") or {}
        lots: list[CreditLot] = []
        for item in data.get("Accounts") or []:
            # Cycle* 是本周期剩余，Capacity* 是整个套餐周期的总容量：
            # 只有“本期已消耗”时两者才不同（Free Plan、国内体验版都是
            # Capacity 500、CycleUsed 500、CycleRemain 0）。所以必须先取
            # Cycle，字段缺失才回退到 Capacity，否则会把已经用完的额度当成剩余。
            amount = _number(item.get("CycleCapacityRemainPrecise"))
            if amount is None:
                amount = _number(item.get("CapacityRemainPrecise"))
            if amount is None:
                amount = _number(item.get("CycleCapacityRemain"))
            if amount is None:
                amount = _number(item.get("CapacityRemain"))
            if amount is None or amount <= 0:
                continue
            expires = (parse_iso(item.get("DeductionEndTime"))
                       or _parse_local(item.get("CycleEndTime"))
                       or parse_iso(item.get("ExpiredTime")))
            if expires and expires <= datetime.now(timezone.utc):
                continue
            lots.append(CreditLot(amount, expires,
                                  str(item.get("PackageName") or "积分包")))
        lots.sort(key=lambda lot: lot.expires_at or datetime.max.replace(tzinfo=timezone.utc))
        return lots

    def status(self, account: Account, log: LogFn) -> StatusResult:
        if account.expired:
            return StatusResult(None, "登录态已过期")
        # 国际版没有签到玩法：只读积分，不再请求 checkin-status。
        if not self.requires_checkin(account):
            snapshot = self.credits(account, log)
            return StatusResult(None, snapshot.summary, snapshot.balance, snapshot.lots)

        # 新版看板优先；旧接口在活动未开放时会返回 active=false、
        # today_checked_in=false，看起来像“今天没签”，实际是接口不返回。
        data = {}
        activity = None
        try:
            activity = self._activity_status(account)
        except Exception as exc:
            log(f"[WorkBuddy] 签到看板读取失败：{exc}")
        if activity is not None:
            data = activity
        else:
            resp = self._post("checkin-status", account)
            if resp.get("code") != 0:
                raise RuntimeError(str(resp.get("msg") or "状态查询失败"))
            data = resp.get("data") or {}

        lots = None
        try:
            lots = self._resource_lots(account)
        except Exception as exc:
            log(f"[WorkBuddy] 积分批次读取失败：{exc}")

        parts = []
        balance = None
        if lots is not None:
            total = sum(lot.amount for lot in lots)
            balance = total
            parts.append(f"剩余 {total:g} 积分")
        else:
            total_credits = data.get("total_credits")
            if isinstance(total_credits, (int, float)):
                balance = float(total_credits)
                parts.append(f"累计 {total_credits:g} 积分")
        if data.get("streak_days"):
            parts.append(f"连续 {data['streak_days']} 天")
        summary = "，".join(parts) or "无可用积分"

        checked = bool(data.get("today_checked_in"))
        if data.get("active") is False and not checked:
            return StatusResult(None, summary, balance, lots)
        return StatusResult(checked, summary, balance, lots)

    def credits(self, account: Account, log: LogFn) -> CreditSnapshot:
        # 只读积分批次，不请求 checkin-status，因此不会查询签到状态。
        if account.expired:
            return CreditSnapshot("登录态已过期")
        lots = self._resource_lots(account)
        total = sum(lot.amount for lot in lots)
        return CreditSnapshot(f"剩余 {total:g} 积分", total, lots)

    def checkin(self, account: Account, log: LogFn) -> ActionResult:
        if account.expired:
            return ActionResult(False, "登录态已过期，请打开客户端刷新")
        if not self.requires_checkin(account):
            return ActionResult(False, "WorkBuddy 国际版无需签到")
        resp = self._post("daily-checkin", account)
        code = resp.get("code")
        if code == 0:
            data = resp.get("data") or {}
            credit = data.get("credit")
            granted = float(credit) if isinstance(credit, (int, float)) else None
            days = data.get("validity_days") or data.get("expire_days")
            return ActionResult(True, f"签到成功，+{credit} 积分" if credit else "签到成功",
                                granted=granted,
                                validity_days=int(days) if isinstance(days, (int, float)) else None)
        if code == 10001:
            return ActionResult(True, "今日已签到")
        return ActionResult(False, str(resp.get("msg") or f"失败 code={code}"))

    def client_exe(self, account: Account | None = None) -> Path | None:
        # 按版本取客户端，不能跨版本按名字回退：国际版会先命中 CodeBuddy CN，
        # 国内版会命中 CodeBuddy IDE 而不是 WorkBuddy 桌面端。
        intl = account is not None and _is_intl(account)
        exe = Edition("intl" if intl else "cn").exe
        if exe.exists():
            return exe
        if intl:
            return None
        base = Path(os.environ.get("LOCALAPPDATA", "")) / "Programs"
        for name in ("CodeBuddy CN", "WorkBuddy", "CodeBuddy"):
            candidate = base / name / f"{name}.exe"
            if candidate.exists():
                return candidate
        return None
