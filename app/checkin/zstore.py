# -*- coding: utf-8 -*-
"""ZCode 账号档案与切换。

档案存在本工具统一的账号库里（data/accounts.dat，DPAPI 加密），
每条记录带着整份 credentials.json + config.json 快照 —— ZCode 的登录态
就是一个 JSON，不像别的平台需要拼 Chromium 票据，所以整份存最省事。
"""
from __future__ import annotations

import uuid
from datetime import datetime

from ..core import winproc
from . import store as account_store
from . import zcode, zquota
from .base import Account, CreditSnapshot, Provider, StatusResult

PROCESS_NAMES = ("ZCode.exe", "zcode.exe")
CLIENT_NAME = "ZCode.exe"


def _meta(account: Account) -> dict:
    return account.meta if isinstance(account.meta, dict) else {}


def credentials_of(account: Account) -> dict:
    creds = _meta(account).get("credentials")
    return creds if isinstance(creds, dict) else {}


def config_of(account: Account) -> dict | None:
    config = _meta(account).get("config")
    return config if isinstance(config, dict) else None


def device_of(account: Account) -> str:
    return str(_meta(account).get("device_mid") or "").strip()


def arms_of(account: Account) -> str:
    return str(_meta(account).get("arms_uid") or "").strip()


def _used_devices() -> set[str]:
    return {device_of(item) for item in profiles() if device_of(item)}


def profiles() -> list[Account]:
    return [account for account in account_store.as_accounts()
            if account.platform == zcode.PLATFORM]


def find_same(creds: dict) -> Account | None:
    for account in profiles():
        if zcode.same_login(credentials_of(account), creds):
            return account
    return None


def save(creds: dict, config: dict | None, name: str | None = None,
         device_mid: str | None = None, arms_uid: str | None = None) -> Account:
    """写一条档案；同一登录态只更新，不重复入库。"""
    existing = find_same(creds)
    ident = zcode.identity(creds)
    account_id = zcode.account_id_of(creds)
    label = name or zcode.label_of(ident)
    meta = {
        "credentials": creds,
        "config": config,
        "provider": ident.get("provider") or "",
        "email": ident.get("email") or "",
        "uid": account_id,
        "device_mid": device_mid or (device_of(existing) if existing else ""),
        "arms_uid": arms_uid or (arms_of(existing) if existing else ""),
        "saved_at": datetime.now().strftime("%Y-%m-%d %H:%M"),
        "source": "客户端登录态",
    }
    if not meta["device_mid"]:
        live = zcode.device_mid()
        taken = _used_devices()
        meta["device_mid"] = live if (live and live not in taken) else str(uuid.uuid4())
    if not meta["arms_uid"]:
        live_uid = zcode.read_arms_uid()
        taken = {arms_of(item) for item in profiles() if arms_of(item)}
        meta["arms_uid"] = live_uid if (live_uid and live_uid not in taken) else zcode.new_arms_uid()

    account = Account(
        platform=zcode.PLATFORM,
        platform_label=zcode.LABEL,
        account_id=account_id,
        name=existing.name if existing and not name else label,
        token=zcode.field(creds, "zcodejwttoken") or "",
        expires_at=zcode.jwt_expiry(creds),
        source="客户端登录态",
        meta=meta,
        requires_checkin=False,
    )
    item = account_store.to_item(account)
    item["account_id"] = account_id
    account_store.upsert(item)
    return account


def capture_live(log) -> Account | None:
    """把客户端当前登录的账号存成档案；已经在库里就原样返回。

    log 只接受一个参数：service.collect 传下来的是单参回调，多传级别会 TypeError。
    """
    live = zcode.read_live()
    if not live or not zcode.is_logged_in(live):
        return None
    existing = find_same(live)
    if existing is not None:
        # 凭据可能被客户端刷新过（token 续期），顺手同步回档案
        if zcode.canonical_hash(credentials_of(existing)) != zcode.canonical_hash(live):
            save(live, zcode.read_json(zcode.live_config()), name=existing.name)
            log(f"[{zcode.LABEL}] 已更新 {existing.name} 的登录态")
        account = find_same(live) or existing
        account.source = "客户端登录态"
        return account
    account = save(live, zcode.read_json(zcode.live_config()))
    log(f"[{zcode.LABEL}] 自动保存登录态：{account.name}")
    return account


def import_zswitch(root=None) -> list[Account]:
    """把 zcode-switch 的账号库并进来（一次性迁移，已在库里的跳过）。

    源库每条就是一个完整的 credentials + config 快照，字段名只差设备身份
    （virtual_device_mid / virtual_arms_uid），映射过来即可，不需要重新登录。
    """
    folder = root or (zcode.home() / ".zcode-switch" / "accounts")
    if not folder.is_dir():
        return []
    imported: list[Account] = []
    for file in sorted(folder.glob("*.json")):
        data = zcode.read_json(file)
        if not data:
            continue
        creds = data.get("credentials")
        if not isinstance(creds, dict) or not zcode.is_logged_in(creds):
            continue
        if find_same(creds):
            continue
        name = str(data.get("name") or "").strip()
        if name.endswith("@phone.local"):
            name = ""          # 交给 label_of 去掉占位域名
        imported.append(save(
            creds,
            data.get("config") if isinstance(data.get("config"), dict) else None,
            name=name or None,
            device_mid=str(data.get("virtual_device_mid") or "").strip() or None,
            arms_uid=str(data.get("virtual_arms_uid") or "").strip() or None,
        ))
    return imported


def is_current(account: Account) -> bool:
    live = zcode.read_live()
    if not live or not zcode.is_logged_in(live):
        return False
    return zcode.same_login(live, credentials_of(account))


def client_running() -> bool:
    return winproc.any_running(PROCESS_NAMES)


def switch(account: Account, log) -> str:
    creds = credentials_of(account)
    if not creds:
        raise zcode.SwitchError("这条档案里没有登录凭据，无法切换。")
    if is_current(account):
        raise zcode.SwitchError("这个账号就是客户端当前登录的账号，不需要切换。")
    if client_running():
        log(f"[{zcode.LABEL}] 检测到客户端正在运行，先自动关闭…")
        for name in winproc.running_names(PROCESS_NAMES):
            winproc.kill_process(name)
        if client_running():
            raise zcode.SwitchError("ZCode 没能自动关闭，请手动退出后再切换。")
    message = zcode.switch_to(creds, config_of(account), device_of(account),
                              arms_of(account), log)
    log(f"[{zcode.LABEL}] 已切换客户端登录态：{account.name}", "ok")
    return message


def launch() -> bool:
    exe = zcode.client_exe()
    return bool(exe and winproc.start_exe(exe))


# ------------------------------------------------------------------ Provider
class ZCodeProvider(Provider):
    key = zcode.PLATFORM
    label = zcode.LABEL
    live_credit = False
    next_from_api = False

    def requires_checkin(self, account: Account) -> bool:
        return False

    def scan(self, log, extra_paths: tuple = ()) -> list[Account]:
        live = zcode.read_live()
        if not live:
            log(f"[{self.label}] 没有找到登录态文件（客户端未登录过）")
            return []
        if not zcode.is_logged_in(live):
            log(f"[{self.label}] 客户端当前未登录")
            return []
        account = capture_live(log)
        return [account] if account else []

    def status(self, account: Account, log) -> StatusResult:
        quota = self.quota(account, log)
        return StatusResult(checked_in=None, summary=zquota.summary_text(quota),
                            balance=None, lots=None)

    def credits(self, account: Account, log) -> CreditSnapshot:
        quota = self.quota(account, log)
        return CreditSnapshot(summary=zquota.summary_text(quota), balance=None,
                              lots=None, quota=quota)

    def quota(self, account: Account, log) -> dict:
        creds = credentials_of(account)
        if not creds:
            raise zcode.SwitchError("这条档案里没有登录凭据。")
        mid = device_of(account) or zcode.device_mid()
        try:
            quota = zquota.query(creds, config_of(account), mid)
        except zquota.QuotaError as exc:
            log(f"[{self.label}] {account.name} 额度查询失败：{exc}")
            raise
        quota["fetched_at"] = datetime.now()
        return quota

    def client_exe(self, account: Account | None = None):
        return zcode.client_exe()
