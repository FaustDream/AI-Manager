# -*- coding: utf-8 -*-
"""快速切换：把账号 token 写回官方客户端的登录态文件。

写入前自动备份；客户端必须处于关闭状态。支持 WorkBuddy 国内/国际、
Qoder 国内/国际；Trae 的 iCube 登录态是设备绑定加密格式，无法安全重写。
"""
from __future__ import annotations

import base64
import json
import os
import shutil
import time
import uuid
from datetime import datetime, timedelta, timezone
from pathlib import Path

from cryptography.hazmat.primitives.ciphers.aead import AESGCM

from ..core import winproc
from ..sessions import profiles as wb_profiles
from ..sessions.editions import Edition
from . import qoder as qoder_provider
from . import trae as trae_provider
from .base import Account, parse_iso
from .workbuddy import seal_field

QODER_DIR_CN = Path(os.environ.get("APPDATA", "")) / "com.qodercn.app.stable"
QODER_DIR_INTL = Path(os.environ.get("APPDATA", "")) / "com.qoder.app.stable"
WB_FILE_CN = (Path(os.environ.get("LOCALAPPDATA", "")) / "CodeBuddyExtension"
              / "Data" / "Public" / "auth" / "workbuddy-desktop.info")
WB_FILE_INTL = (Path(os.environ.get("LOCALAPPDATA", "")) / "CodeBuddyExtension"
                / "Data" / "Public" / "auth" / "workbuddy-desktop-ai.info")

SUPPORTED = {"qoder": True, "qoder_intl": True, "workbuddy": True, "trae": False}
PROCESS_NAMES = {
    "qoder": ("Qoder CN.exe", "Qoder.exe"),
    "qoder_intl": ("Qoder.exe", "Qoder CN.exe"),
    "workbuddy": ("CodeBuddy CN.exe", "WorkBuddy.exe", "WorkBuddyAI.exe", "CodeBuddy.exe"),
    "trae": ("Trae CN.exe", "Trae.exe"),
}


class SwitchError(Exception):
    pass


def client_names_for(account: Account) -> tuple[str, ...]:
    """这个账号对应的官方客户端进程名（国内/国际分开）。"""
    if account.platform == "workbuddy":
        intl = (account.meta.get("edition") == "intl"
                or str(account.meta.get("domain") or "").endswith(".ai"))
        return ("WorkBuddyAI.exe",) if intl \
            else ("CodeBuddy CN.exe", "WorkBuddy.exe", "CodeBuddy.exe")
    if account.platform == "qoder_intl":
        return ("Qoder.exe",)
    if account.platform == "qoder":
        return ("Qoder CN.exe",)
    return tuple(PROCESS_NAMES.get(account.platform, ()))


def client_running(platform: str) -> bool:
    return winproc.any_running(PROCESS_NAMES.get(platform, ()))


def client_running_for(account: Account) -> bool:
    return winproc.any_running(client_names_for(account))


def close_client_for(account: Account, log) -> bool:
    """切换前自动关闭客户端；全部退出返回 True，关不掉返回 False。"""
    names = client_names_for(account)
    running = winproc.running_names(names)
    if not running:
        return True
    log(f"[{account.platform_label}] 检测到客户端正在运行（{'、'.join(running)}），先自动关闭…")
    for name in running:
        winproc.kill_process(name)
    still = winproc.running_names(names)
    if still:
        log(f"[{account.platform_label}] {'、'.join(still)} 没能结束")
        return False
    log(f"[{account.platform_label}] 客户端已关闭，继续切换")
    return True


def _restart_client(account: Account, log) -> None:
    """切换完成后重新打开客户端，直接以新账号进入。"""
    try:
        from . import service  # 延迟导入，避免与 service 的加载顺序问题
        exe = service.client_exe(account.platform, account)
    except Exception:
        exe = None
    if exe and winproc.start_exe(exe):
        log(f"[{account.platform_label}] 已重新打开客户端（{Path(exe).name}）")
    else:
        log(f"[{account.platform_label}] 客户端已关闭，可手动打开使用新账号")


def _backup(path: Path) -> Path:
    stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    target = path.with_name(path.name + f".bak-{stamp}")
    shutil.copy2(path, target)
    return target


def _qoder_dir(account: Account) -> Path:
    return QODER_DIR_INTL if account.platform == "qoder_intl" else QODER_DIR_CN


def _wb_file(account: Account) -> Path:
    intl = (account.meta.get("edition") == "intl"
            or str(account.meta.get("domain") or "").endswith(".ai"))
    return WB_FILE_INTL if intl else WB_FILE_CN


def _iso_z(value: datetime | None) -> str:
    if not value:
        return ""
    return value.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def is_current(account: Account) -> bool:
    """判断这个账号是否就是官方客户端当前登录的账号。"""
    try:
        if account.platform == "workbuddy":
            data = json.loads(_wb_file(account).read_text(encoding="utf-8"))
            uid = str((data.get("account") or {}).get("uid") or "")
            # 不比较 domain：旧版 .info 可能没有写 domain
            return bool(uid) and uid == str(account.meta.get("uid") or "")
        if account.platform in ("qoder", "qoder_intl"):
            auth = qoder_provider.load_auth(_qoder_dir(account) / "auth.v1.dat")
            return str((auth.get("user") or {}).get("id") or "") == account.account_id
        if account.platform == "trae":
            storage = json.loads(Path(account.source).read_text(encoding="utf-8"))
            auth = trae_provider.decrypt_auth(storage.get(trae_provider.AUTH_KEY) or "")
            return str(auth.get("userId") or "") == account.account_id
    except Exception:
        return False
    return False


# ------------------------------------------------------------------ Qoder CN
def build_qoder_auth(account: Account, current: dict) -> dict:
    saved = account.meta.get("refresh_expires_at")
    refresh_expires = _iso_z(parse_iso(saved)) if saved else \
        _iso_z(datetime.now(timezone.utc) + timedelta(days=30))
    old_user = current.get("user") or {}
    same_user = str(old_user.get("id") or "") == account.account_id
    return {
        "schemaVersion": current.get("schemaVersion", 1),
        "token": account.token,
        "refreshToken": account.meta.get("refresh_token", ""),
        "expiresAt": _iso_z(account.expires_at),
        "refreshTokenExpiresAt": refresh_expires,
        "user": {
            "id": account.account_id,
            "name": account.name,
            "email": account.meta.get("email", "") or (old_user.get("email", "") if same_user else ""),
            "phone": old_user.get("phone", "") if same_user else "",
            "avatarUrl": old_user.get("avatarUrl", "") if same_user else "",
        },
        "authenticationContext": current.get("authenticationContext"),
    }


def switch_qoder(account: Account, log) -> str:
    folder = _qoder_dir(account)
    if not close_client_for(account, log):
        raise SwitchError("Qoder 客户端没能自动关闭，请手动退出后再切换。")
    path = folder / "auth.v1.dat"
    if not path.exists():
        raise SwitchError("没有找到 Qoder 的登录态文件。")
    if not account.meta.get("refresh_token"):
        raise SwitchError("这个账号没有 refresh token，无法写回客户端。")

    current = qoder_provider.load_auth(path)
    auth = build_qoder_auth(account, current)

    state = json.loads((folder / "Local State").read_text(encoding="utf-8"))
    raw_key = base64.b64decode(state["os_crypt"]["encrypted_key"])
    key = qoder_provider.dpapi_unprotect(raw_key[5:])
    nonce = os.urandom(12)
    sealed = AESGCM(key).encrypt(nonce, json.dumps(auth, ensure_ascii=False).encode("utf-8"), None)

    backup = _backup(path)
    path.write_bytes(b"v10" + nonce + sealed)
    log(f"[{account.platform_label}] 已切换客户端登录态，备份：{backup.name}")
    _restart_client(account, log)
    return f"{account.platform_label} 已切换到 {account.name}。\n备份：{backup.name}"


# ----------------------------------------------------------------- WorkBuddy
def build_workbuddy_info(account: Account, template: dict) -> dict:
    data = json.loads(json.dumps(template, ensure_ascii=False))
    now_ms = int(time.time() * 1000)
    expires_ms = int(account.expires_at.timestamp() * 1000) if account.expires_at \
        else now_ms + 3600 * 1000
    refresh_expires_ms = expires_ms + 30 * 24 * 3600 * 1000
    refresh = account.meta.get("refresh_token") or ""
    if not refresh:
        raise SwitchError("这个账号没有 refresh token，无法写回客户端。")

    info = account.meta.get("account_info") or {}
    intl = (account.meta.get("edition") == "intl"
            or str(account.meta.get("domain") or "").endswith(".ai"))
    auth = data.setdefault("auth", {})
    auth["accessToken"] = seal_field(account.token)
    auth["refreshToken"] = seal_field(refresh)
    auth["expiresAt"] = expires_ms
    auth["expiresIn"] = max(0, (expires_ms - now_ms) // 1000)
    auth["refreshExpiresAt"] = refresh_expires_ms
    auth["refreshExpiresIn"] = max(0, (refresh_expires_ms - now_ms) // 1000)
    auth["domain"] = account.meta.get("domain") or (
        "www.workbuddy.ai" if intl else "www.workbuddy.cn")
    auth["lastRefreshTime"] = now_ms
    auth["tokenType"] = "Bearer"
    auth["sessionState"] = str(uuid.uuid4())

    acc = data.setdefault("account", {})
    acc["uid"] = account.meta.get("uid") or account.account_id
    acc["nickname"] = seal_field(account.name)
    acc["uin"] = str(info.get("uin") or account.meta.get("uin") or "")
    acc["lastLogin"] = True

    for key in ("accounts", "allAccounts"):
        items = data.get(key)
        if not isinstance(items, list):
            items = []
        items = [item for item in items
                 if isinstance(item, dict)
                 and str(item.get("uid") or "") != acc["uid"]]
        entry = dict(acc)
        entry.setdefault("deployStatus", {})
        entry.setdefault("sso", {})
        # 客户端的 phoneNumber 可能是 $wbEncrypted 字段，原样保留
        entry["phoneNumber"] = info.get("phoneNumber") or account.meta.get("phone") or ""
        items.append(entry)
        data[key] = items
    return data


def _wb_edition(account: Account) -> Edition:
    """账号对应的客户端数据目录（国内 .workbuddy / 国际 .workbuddy-ai）。"""
    intl = (account.meta.get("edition") == "intl"
            or str(account.meta.get("domain") or "").endswith(".ai"))
    return Edition("intl" if intl else "cn")


def _wb_target_uid(account: Account) -> str:
    return str(account.meta.get("uid") or account.account_id or "")


def switch_workbuddy(account: Account, log) -> str:
    if not close_client_for(account, log):
        raise SwitchError(f"{account.platform_label} 客户端没能自动关闭，请手动退出后再切换。")
    path = _wb_file(account)
    if not path.exists():
        raise SwitchError(f"没有找到 {account.platform_label} 的登录态文件。")

    ep = _wb_edition(account)
    target_uid = _wb_target_uid(account)
    notes: list[str] = []
    cur_uid = ""

    # 1) 切换前：把当前客户端的登录态存档，之前的账号随时可一键换回
    try:
        info_uid = str(wb_profiles.auth_uid(ep) or "")
        snap_uid = str(wb_profiles.current_uid(ep) or "")
        cur_uid = snap_uid or info_uid
        if info_uid and snap_uid and info_uid != snap_uid:
            # .info 已被改写但客户端还没按新账号起来：这时存档只会张冠李戴
            log(f"[{account.platform_label}] 登录态与账号快照不一致"
                f"（{info_uid[:8]} / {snap_uid[:8]}），跳过本次存档")
            cur_uid = ""
        if cur_uid and cur_uid != target_uid:
            wb_profiles.save_current(ep, cur_uid)
            notes.append("已存档当前账号的登录态，之后可从账号档案一键切回")
            log(f"[{account.platform_label}] 已存档当前账号 {cur_uid[:8]} 的登录态")
    except Exception as exc:  # 存档失败不应阻断切换
        log(f"[{account.platform_label}] 存档当前账号失败（切换继续）：{exc}")

    # 2) 目标账号已有档案：先恢复登录态，再把切换前账号的会话按引用关联过来
    auth_ready = False
    if target_uid and wb_profiles.has_pack(ep, target_uid):
        try:
            wb_profiles.restore_pack(ep, target_uid)
            auth_ready = wb_profiles.has_pack_auth(ep, target_uid)
            notes.append(f"已恢复 {target_uid[:8]} 之前保存的登录态（免重新登录）")
            log(f"[{account.platform_label}] 已恢复目标账号 {target_uid[:8]} 的登录态")
            try:
                from ..sessions import restore as session_restore
                report = session_restore.restore_for_account(
                    ep, target_uid,
                    source_uids={cur_uid} if cur_uid else None,
                    log=log,
                )
                notes.append(report.summary())
                log(f"[{account.platform_label}] {report.summary()}")
            except Exception as exc:
                # 会话恢复失败不能把登录态回滚：客户端已经准备好用新账号启动
                notes.append(f"会话恢复未完成：{exc}")
                log(f"[{account.platform_label}] 会话恢复失败（登录态已切换）：{exc}", "error")
        except Exception as exc:
            log(f"[{account.platform_label}] 恢复目标账号登录态失败（按全新登录处理）：{exc}")

    # 3) 档案里没有登录票据（旧档案）：优先用客户端自己留的同账号备份，
    #    最后才退回「用 token 重建」——重建出来的 sessionState 服务端不认，
    #    客户端会回到登录页，所以只当兜底
    if not auth_ready and target_uid:
        backup_auth = wb_profiles.find_auth_backup(ep, target_uid)
        if backup_auth is not None:
            try:
                wb_profiles.write_auth(ep, backup_auth)
                auth_ready = True
                notes.append("已从客户端备份恢复该账号的登录票据")
                log(f"[{account.platform_label}] 用客户端备份 {backup_auth.name} 恢复登录票据")
            except OSError as exc:
                log(f"[{account.platform_label}] 写回登录票据失败：{exc}", "error")

    if not auth_ready:
        if not target_uid or not wb_profiles.has_pack(ep, target_uid):
            # 本机没有这个账号的 Chromium 登录态，无论 token 怎么写都要重新登录
            notes.append("本机没有该账号的登录态存档，客户端会停在登录页，需要重新登录一次；"
                         "登录后点「恢复会话」可把已有项目会话关联过来")
            log(f"[{account.platform_label}] 目标账号 {target_uid[:8] or '未知'} 无档案，"
                "切换后需要重新登录一次")
        template = json.loads(path.read_text(encoding="utf-8"))
        data = build_workbuddy_info(account, template)
        _backup(path)
        path.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")
        if target_uid and wb_profiles.has_pack(ep, target_uid):
            notes.append("旧档案没有登录票据，本次用 token 重建，可能仍要重新登录一次；"
                         "关闭客户端后会自动补存票据")
            log(f"[{account.platform_label}] 旧档案缺少 auth.info，已按 token 重建登录态", "warn")
    else:
        log(f"[{account.platform_label}] 登录票据已原样恢复，客户端会直接进入该账号")

    log(f"[{account.platform_label}] 已切换客户端登录态")
    _restart_client(account, log)

    msg = f"{account.platform_label} 已切换到 {account.name}。"
    if notes:
        msg += "\n" + "；".join(notes)
    return msg


def switch(account: Account, log) -> str:
    if is_current(account):
        raise SwitchError("这个账号就是客户端当前登录的账号，不需要切换。")
    if account.platform in ("qoder", "qoder_intl"):
        return switch_qoder(account, log)
    if account.platform == "workbuddy":
        return switch_workbuddy(account, log)
    if account.platform == "trae":
        raise SwitchError(
            "Trae 的登录态是设备绑定的加密格式，无法安全写回客户端。\n"
            "请用「打开客户端」在 Trae 里手动登录。")
    raise SwitchError(f"暂不支持切换 {account.platform_label} 的客户端登录。")


def describe(platform: str) -> str:
    """给界面显示的一句说明。"""
    if platform == "trae":
        return "设备绑定加密，无法写回，需在客户端手动登录"
    if platform == "qoder_intl":
        return "可写回 Qoder 国际版登录态（自动备份）"
    return "可写回客户端登录态（自动备份）"
