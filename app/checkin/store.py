# -*- coding: utf-8 -*-
"""浏览器登录得到的账号：用 Windows DPAPI 加密保存在 data/accounts.dat。

只保存登录流程拿到的 token，不修改官方客户端的任何文件。
"""
from __future__ import annotations

import base64
import ctypes
import ctypes.wintypes as wt
import json

from ..core import paths
from .base import Account, parse_iso
from .registry import LABELS


class _Blob(ctypes.Structure):
    _fields_ = [("cbData", wt.DWORD), ("pbData", ctypes.POINTER(ctypes.c_char))]


def _make_blob(data: bytes) -> _Blob:
    blob = _Blob()
    blob.cbData = len(data)
    blob.pbData = ctypes.create_string_buffer(data)
    return blob


def _protect(data: bytes) -> bytes:
    kernel32 = ctypes.windll.kernel32
    raw_in = _make_blob(data)
    blob_out = _Blob()
    ok = ctypes.windll.crypt32.CryptProtectData(
        ctypes.byref(raw_in), None, None, None, None, 0x1, ctypes.byref(blob_out))
    if not ok:
        raise OSError(f"CryptProtectData 失败：{kernel32.GetLastError()}")
    try:
        return ctypes.string_at(blob_out.pbData, blob_out.cbData)
    finally:
        kernel32.LocalFree(blob_out.pbData)


def _unprotect(data: bytes) -> bytes:
    kernel32 = ctypes.windll.kernel32
    raw_in = _make_blob(data)
    blob_out = _Blob()
    ok = ctypes.windll.crypt32.CryptUnprotectData(
        ctypes.byref(raw_in), None, None, None, None, 0, ctypes.byref(blob_out))
    if not ok:
        raise OSError(f"CryptUnprotectData 失败：{kernel32.GetLastError()}")
    try:
        return ctypes.string_at(blob_out.pbData, blob_out.cbData)
    finally:
        kernel32.LocalFree(blob_out.pbData)


def load() -> list[dict]:
    try:
        blob = base64.b64decode(paths.ACCOUNTS_FILE.read_text(encoding="ascii"))
        items = json.loads(_unprotect(blob).decode("utf-8"))
    except (OSError, ValueError):
        return []
    if not isinstance(items, list):
        return []
    return [item for item in items
            if isinstance(item, dict) and item.get("platform") in LABELS]


def save(items: list[dict]) -> None:
    paths.DATA_DIR.mkdir(parents=True, exist_ok=True)
    raw = json.dumps(items, ensure_ascii=False).encode("utf-8")
    try:
        paths.ACCOUNTS_FILE.write_text(
            base64.b64encode(_protect(raw)).decode("ascii"), encoding="ascii")
    except OSError:
        pass


def upsert(item: dict) -> None:
    items = [existing for existing in load()
             if not (existing.get("platform") == item.get("platform")
                     and existing.get("account_id") == item.get("account_id"))]
    items.append(item)
    save(items)


def remove(platform: str, account_id: str) -> None:
    save([item for item in load()
          if not (item.get("platform") == platform
                  and item.get("account_id") == account_id)])


def to_item(account: Account) -> dict:
    meta = {key: value for key, value in account.meta.items() if key != "stored"}
    return {
        "platform": account.platform,
        "account_id": account.account_id,
        "name": account.name,
        "token": account.token,
        "refresh_token": meta.get("refresh_token", ""),
        "expires_at": account.expires_at.isoformat() if account.expires_at else "",
        "requires_checkin": bool(getattr(account, "requires_checkin", True)),
        "meta": meta,
    }


def as_accounts() -> list[Account]:
    accounts: list[Account] = []
    for item in load():
        platform = item["platform"]
        meta = dict(item.get("meta") or {})
        meta["stored"] = True
        meta.setdefault("refresh_token", item.get("refresh_token", ""))
        account = Account(
            platform=platform,
            platform_label=("WorkBuddy 国际版" if platform == "workbuddy"
                            and meta.get("edition") == "intl"
                            else LABELS.get(platform, platform)),
            account_id=str(item.get("account_id") or ""),
            name=str(item.get("name") or "未命名账号"),
            token=str(item.get("token") or ""),
            expires_at=parse_iso(item.get("expires_at")),
            source="浏览器登录",
            meta=meta,
            requires_checkin=bool(item.get("requires_checkin", True)),
        )
        if account.token and account.account_id:
            accounts.append(account)
    return accounts


def find(platform: str, account_id: str) -> dict | None:
    for item in load():
        if item.get("platform") == platform and str(item.get("account_id")) == account_id:
            return item
    return None
