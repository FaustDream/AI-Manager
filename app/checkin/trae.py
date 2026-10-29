# -*- coding: utf-8 -*-
"""Trae CN / TRAE SOLO CN：读取 iCube 登录态并调用签到接口。"""
from __future__ import annotations

import base64
import hashlib
import json
import os
import re
from datetime import datetime, timezone
from pathlib import Path

import requests
from cryptography.hazmat.primitives import padding
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes

from ..core import paths
from .base import (Account, ActionResult, CreditLot, CreditSnapshot, LogFn, Provider,
                   StatusResult, parse_iso)

AUTH_KEY = "iCubeAuthInfo://icube.cloudide"
API_BASE = "https://api.trae.cn/trae/api/v2/ug/checkin_credits"
CLIENT_DIRS = ("Trae CN", "TRAE SOLO CN")

# iCube 登录态加密格式
HDR_LEN = 6
KEY_LEN = 32
HMAC_LEN = 64
URE = bytes([82, 9, 106, 213, 48, 54, 165, 56, 191, 64, 163, 158, 129, 243, 215, 251, 124, 227, 57,
             130, 155, 47, 255, 135, 52, 142, 67, 68, 196, 222, 233, 203, 84, 123, 148, 50, 166, 194,
             35, 61, 238, 76, 149, 11, 66, 250, 195, 78, 8, 46, 161, 102, 40, 217, 36, 178, 118, 91,
             162, 73, 109, 139, 209, 37])
DRE = bytes([31, 221, 168, 51, 136, 7, 199, 49, 177, 18, 16, 89, 39, 128, 236, 95, 96, 81, 127, 169,
             25, 181, 74, 13, 45, 229, 122, 159, 147, 201, 156, 239, 160, 224, 59, 77, 174, 42, 245,
             176, 200, 235, 187, 60, 131, 83, 153, 97, 23, 43, 4, 126, 186, 119, 214, 38, 225, 105,
             20, 99, 85, 33, 12, 125])


def decrypt_auth(b64_text: str) -> dict:
    raw = base64.b64decode(b64_text)
    key = raw[HDR_LEN:HDR_LEN + KEY_LEN]
    seed = hashlib.sha512(hashlib.sha512(key).digest()
                          + bytes(a ^ b for a, b in zip(URE, DRE))).digest()
    decryptor = Cipher(algorithms.AES(seed[:16]), modes.CBC(seed[16:32])).decryptor()
    plain = decryptor.update(raw[HDR_LEN + KEY_LEN:]) + decryptor.finalize()
    unpadder = padding.PKCS7(128).unpadder()
    plain = unpadder.update(plain) + unpadder.finalize()
    return json.loads(plain[HMAC_LEN:].decode("utf-8"))


def jwt_exp(token: str):
    parts = token.split(".")
    if len(parts) != 3:
        return None
    try:
        payload = parts[1] + "=" * (-len(parts[1]) % 4)
        data = json.loads(base64.urlsafe_b64decode(payload))
    except (ValueError, TypeError):
        return None
    exp = data.get("exp")
    return parse_iso(int(exp) * 1000) if exp else None


def device_id(app_dir: Path) -> str:
    config = app_dir / "ahanet" / "tt_net_config.config"
    try:
        match = re.search(r"device_id[^\w]*(\d+)", config.read_text("utf-8", errors="ignore"))
        if match:
            return match.group(1)
    except OSError:
        pass
    logs_dir = app_dir / "logs"
    if logs_dir.is_dir():
        try:
            logs = sorted(logs_dir.glob("*/main.log"), key=lambda p: p.stat().st_mtime, reverse=True)
        except OSError:
            logs = []
        for path in logs[:5]:
            try:
                match = re.search(r"\[ICDRS\].*?did: (\d+)", path.read_text("utf-8", errors="ignore"))
                if match:
                    return match.group(1)
            except OSError:
                continue
    return ""


def unwrap(resp: dict) -> dict:
    if "checked_in" not in resp and isinstance(resp.get("data"), dict) \
            and "checked_in" in resp["data"]:
        return resp["data"]
    return resp


def _number(value) -> float | None:
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


class TraeProvider(Provider):
    key = "trae"
    label = "Trae CN"
    live_credit = True

    def scan(self, log: LogFn, extra_paths: tuple = ()) -> list[Account]:
        files = [paths.user_appdata_dir() / name / "User" / "globalStorage" / "storage.json"
                 for name in CLIENT_DIRS]
        extras = [Path(path) for path in extra_paths]
        accounts: list[Account] = []
        seen: set[str] = set()
        for storage_file in files + extras:
            is_extra = storage_file in extras
            if not storage_file.exists():
                if is_extra:
                    log(f"[Trae] 未找到 {storage_file}")
                continue
            try:
                storage = json.loads(storage_file.read_text(encoding="utf-8"))
                encrypted = storage.get(AUTH_KEY)
                if not encrypted:
                    log(f"[Trae] [{storage_file.parent.name}] 未登录，跳过")
                    continue
                auth = decrypt_auth(encrypted)
                token = auth.get("token")
                if not token:
                    log(f"[Trae] [{storage_file.parent.name}] 登录态中没有 token")
                    continue
                if token in seen:
                    continue
                seen.add(token)
                account = auth.get("account") or {}
                expires = parse_iso(auth.get("expiredAt")) or jwt_exp(token)
                base = storage_file.parents[2] if len(storage_file.parents) > 2 else storage_file.parent
                accounts.append(Account(
                    platform=self.key,
                    platform_label="Trae CN",
                    account_id=str(auth.get("userId") or token[-16:]),
                    name=account.get("username") or f"用户{auth.get('userId', '')}",
                    token=token,
                    expires_at=expires,
                    source=str(storage_file),
                    meta={
                        "region": (auth.get("userRegion") or {}).get("region", ""),
                        "device_id": device_id(base),
                    },
                ))
            except Exception as exc:
                log(f"[Trae] 解析 [{storage_file.name}] 失败：{exc}")
        return accounts

    def _post(self, url: str, account: Account, body: dict | None = None) -> dict:
        headers = {
            "Authorization": f"Cloud-IDE-JWT {account.token}",
            "Content-Type": "application/json",
            "x-device-id": account.meta.get("device_id", ""),
        }
        region = account.meta.get("region")
        if region:
            headers["X-User-Region"] = region
        resp = requests.post(url, headers=headers,
                             json=body if body is not None else {"req_source": 1},
                             timeout=20)
        if resp.status_code >= 400:
            raise RuntimeError(f"HTTP {resp.status_code}（登录态可能过期）")
        return resp.json()

    def _call(self, path: str, account: Account) -> dict:
        return self._post(f"{API_BASE}/{path}", account)

    def _usage(self, account: Account) -> tuple[str, float | None, list]:
        """读套餐/积分包用量，按“先到期先消耗”还原每个批次剩余。"""
        data = self._post(
            "https://api.trae.cn/trae/api/v2/pay/ide_user_ent_usage", account)
        summary_data = data.get("usage_summary") or {}
        total = _number(summary_data.get("total_amount"))
        consumed = _number(summary_data.get("consumed_amount")) or 0.0
        balance = max(0.0, total - consumed) if total is not None else None

        packs = []
        for pack in data.get("user_entitlement_pack_list") or []:
            if not isinstance(pack, dict):
                continue
            base = pack.get("entitlement_base_info") or {}
            quota = base.get("quota") or {}
            limit = _number(quota.get("credits_limit"))
            if limit is None or limit <= 0:
                continue
            expires = parse_iso(base.get("end_time"))
            packs.append([limit, expires, str(pack.get("display_desc") or "积分包")])
        packs.sort(key=lambda item: item[1] or datetime.max.replace(tzinfo=timezone.utc))

        # Trae 的消耗顺序是“剩余有效期最短的批次优先”，所以按到期日
        # 从早到晚扣掉已消耗量，剩下的就是每个批次当前可用积分。
        left = consumed
        lots = []
        now = datetime.now(timezone.utc)
        for amount, expires, label in packs:
            if left >= amount:
                left -= amount
                continue
            amount -= left
            left = 0
            if expires and expires <= now:
                continue
            lots.append(CreditLot(amount, expires, label))

        summary = f"剩余 {balance:g} 积分" if balance is not None else "无可用积分"
        if total is not None and consumed:
            summary += f"（已用 {consumed:g}/{total:g}）"
        return summary, balance, lots

    def status(self, account: Account, log: LogFn) -> StatusResult:
        data = unwrap(self._call("status", account))
        if data.get("enable") is False:
            return StatusResult(None, "签到未开放")
        checked = bool(data.get("checked_in"))
        try:
            summary, balance, lots = self._usage(account)
        except Exception:
            value = data.get("credits")
            balance = float(value) if isinstance(value, (int, float)) else None
            summary = f"剩余 {balance:g} 积分" if balance is not None else ""
            lots = None
        return StatusResult(checked, summary, balance, lots)

    def credits(self, account: Account, log: LogFn) -> CreditSnapshot:
        # 只读套餐用量，不请求 checkin_credits/status，因此不会查询签到状态。
        summary, balance, lots = self._usage(account)
        return CreditSnapshot(summary, balance, lots)

    def checkin(self, account: Account, log: LogFn) -> ActionResult:
        state = self.status(account, log)
        if state.checked_in:
            return ActionResult(True, f"今日已签到（{state.summary}）")
        resp = self._call("claim", account)
        if resp.get("code") == 0:
            summary, granted = "", None
            try:
                after = self.status(account, log)
                summary = after.summary
                if state.balance is not None and after.balance is not None:
                    granted = max(0.0, after.balance - state.balance)
            except Exception:
                pass
            message = f"签到成功，当前 {summary}" if summary else "签到成功"
            # 签到积分有效期为 31 个自然日
            return ActionResult(True, message, granted=granted, validity_days=31)
        message = resp.get("message") or json.dumps(resp, ensure_ascii=False)[:160]
        return ActionResult(False, str(message))

    def client_exe(self, account: Account | None = None) -> Path | None:
        base = paths.user_local_dir() / "Programs"
        for name in CLIENT_DIRS:
            exe = base / name / f"{name}.exe"
            if exe.exists():
                return exe
        return None
