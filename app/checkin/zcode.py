# -*- coding: utf-8 -*-
"""ZCode 客户端登录态的读取与写回。

ZCode 把登录态放在 <dataBaseDir>/.zcode/v2/ 下的 credentials.json + config.json，
字段值带 enc:v1: 前缀（AES-256-GCM，密钥由「机器 + 用户名」推导，不落盘）。
本模块只做「读、解、写回」三件事：额度查询与领取在 zquota / zclaim 里。
"""
from __future__ import annotations

import base64
import hashlib
import json
import os
import uuid
from datetime import datetime, timezone
from pathlib import Path

from cryptography.hazmat.primitives.ciphers.aead import AESGCM

PLATFORM = "zcode"
LABEL = "ZCode"
PREFIX = "enc:v1:"
DEVICE_KEY_PREFIX = "web-remote-control:"
CLIENT_EXE = "ZCode.exe"


# ------------------------------------------------------------------ 路径
def home() -> Path:
    raw = os.environ.get("USERPROFILE") or os.environ.get("HOME") or ""
    return Path(raw) if raw else Path.home()


def data_root() -> Path:
    """ZCode 的 dataBaseDir：新版写进 ~/.zcode/v2/setting.json，旧版就是家目录。"""
    raw = os.environ.get("ZCODE_DATA_BASE_DIR") or ""
    if raw.strip():
        candidate = Path(raw.strip())
        if candidate.is_absolute():
            return candidate
    try:
        data = json.loads(setting_path().read_text(encoding="utf-8"))
    except (OSError, ValueError):
        data = {}
    base = str(data.get("dataBaseDir") or "").strip()
    if base and Path(base).is_absolute():
        return Path(base)
    return home()


def zcode_dir() -> Path:
    return data_root() / ".zcode"


def live_credentials() -> Path:
    return zcode_dir() / "v2" / "credentials.json"


def live_config() -> Path:
    return zcode_dir() / "v2" / "config.json"


def live_telemetry() -> Path:
    return zcode_dir() / "v2" / "telemetry-state.json"


def live_plan_cache() -> Path:
    return zcode_dir() / "v2" / "coding-plan-cache.json"


def setting_path() -> Path:
    return home() / ".zcode" / "v2" / "setting.json"


def client_paths() -> tuple[Path, ...]:
    """ZCode 可执行文件的常见位置；找不到时由界面提示手动设置。"""
    local = os.environ.get("LOCALAPPDATA") or ""
    out = [Path(r"C:\Program Files\ZCode\ZCode.exe")]
    if local:
        out.append(Path(local) / "Programs" / "ZCode" / "ZCode.exe")
        out.append(Path(local) / "ZCode" / "ZCode.exe")
    return tuple(out)


def client_exe() -> Path | None:
    for path in client_paths():
        if path.exists():
            return path
    return None


# ------------------------------------------------------------------ 加解密
def secret() -> str:
    """密钥推导口径必须与客户端一致，否则解出来是乱码。

    客户端用「平台 + 家目录 + 用户名」拼固定前缀；这里不能换成 DPAPI，
    那是本工具自己的账号库在用，跟 ZCode 无关。
    """
    env = os.environ.get("ZCODE_CREDENTIAL_SECRET")
    if env:
        return env
    user = os.environ.get("USERNAME") or os.environ.get("USER") or "unknown"
    return f"zcode-credential-fallback:win32:{home()}:{user}"


def is_encrypted(value: str) -> bool:
    return value.startswith(PREFIX)


def decrypt(value: str) -> str | None:
    if not value:
        return None
    if not is_encrypted(value):
        return value
    body = value[len(PREFIX):]
    parts = body.split(".")
    if len(parts) != 3:
        return None
    try:
        nonce, tag, cipher = (base64.urlsafe_b64decode(part + "=" * (-len(part) % 4))
                              for part in parts)
    except (ValueError, TypeError):
        return None
    key = hashlib.sha256(secret().encode("utf-8")).digest()
    try:
        plain = AESGCM(key).decrypt(nonce, cipher + tag, None)
    except Exception:
        return None
    return plain.decode("utf-8", "replace")


def field(creds: dict, key: str) -> str | None:
    value = creds.get(key)
    return decrypt(value) if isinstance(value, str) else None


def json_field(creds: dict, key: str) -> dict | None:
    raw = field(creds, key)
    if not raw:
        return None
    try:
        data = json.loads(raw)
    except ValueError:
        return None
    return data if isinstance(data, dict) else None


# ------------------------------------------------------------------ 身份
def jwt_payload(token: str) -> dict:
    parts = token.split(".")
    if len(parts) < 2:
        return {}
    payload = parts[1] + "=" * (-len(parts[1]) % 4)
    try:
        data = json.loads(base64.urlsafe_b64decode(payload))
    except (ValueError, TypeError):
        return {}
    return data if isinstance(data, dict) else {}


def identity(creds: dict) -> dict:
    provider = field(creds, "oauth:active_provider") or "bigmodel"
    info = json_field(creds, f"oauth:{provider}:user_info") or {}
    user_id = info.get("id")
    if user_id is None:
        access = field(creds, f"oauth:{provider}:access_token") or ""
        payload = jwt_payload(access)
        user_id = payload.get("user_id") or payload.get("sub")
    email = info.get("email") or (info.get("rawProfile") or {}).get("email") or ""
    return {
        "provider": provider,
        "username": info.get("username") or "",
        "display_name": info.get("displayName") or "",
        "email": email,
        "user_id": str(user_id) if user_id not in (None, "") else "",
    }


def label_of(ident: dict) -> str:
    name = ident.get("display_name") or ident.get("username")
    if name:
        return name
    email = ident.get("email") or ""
    # 手机号登录的账号，客户端把手机号写成 xxx@phone.local，界面上只留号码
    if email.endswith("@phone.local"):
        return email.split("@", 1)[0]
    return email or "ZCode 账号"


def jwt_expiry(creds: dict) -> datetime | None:
    token = field(creds, "zcodejwttoken")
    exp = jwt_payload(token or "").get("exp")
    if not isinstance(exp, (int, float)) or exp <= 0:
        return None
    return datetime.fromtimestamp(exp, tz=timezone.utc)


def is_logged_in(creds: dict) -> bool:
    if not isinstance(creds, dict):
        return False
    if any(key.startswith("oauth:") and key.endswith(":access_token") for key in creds):
        return True
    return bool((creds.get("zcodejwttoken") or "").strip()) if isinstance(
        creds.get("zcodejwttoken"), str) else False


def canonical_hash(creds: dict) -> str:
    """去掉 web-remote-control:* 再算哈希：那几项跟设备绑定，换机器会变。"""
    body = {key: value for key, value in creds.items()
            if not key.startswith(DEVICE_KEY_PREFIX)} if isinstance(creds, dict) else {}
    raw = json.dumps(body, sort_keys=True, ensure_ascii=False, separators=(",", ":"))
    return hashlib.sha256(raw.encode("utf-8")).hexdigest()


def same_login(left: dict, right: dict) -> bool:
    if not left or not right:
        return False
    if canonical_hash(left) == canonical_hash(right):
        return True
    a, b = identity(left), identity(right)
    return bool(a["user_id"] and a["user_id"] == b["user_id"])


def account_id_of(creds: dict) -> str:
    ident = identity(creds)
    return (ident["user_id"] or ident["email"] or ident["username"]
            or canonical_hash(creds)[:16])


# ------------------------------------------------------------------ 读写
def read_json(path: Path) -> dict | None:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    return data if isinstance(data, dict) else None


def atomic_write(path: Path, data: dict) -> None:
    """先写临时文件再替换：客户端正好在读时不会读到半截 JSON。"""
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f"{path.name}.tmp-{uuid.uuid4().hex[:8]}")
    tmp.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    os.replace(tmp, path)


def read_live() -> dict | None:
    return read_json(live_credentials())


def live_logged_in() -> bool:
    return is_logged_in(read_live() or {})


def backup(path: Path) -> Path | None:
    if not path.exists():
        return None
    target = path.with_name(f"{path.name}.bak-{datetime.now():%Y%m%d-%H%M%S}")
    target.write_bytes(path.read_bytes())
    return target


# ------------------------------------------------------------------ 设备身份
def device_mid() -> str | None:
    data = read_json(live_telemetry()) or {}
    mid = str(data.get("deviceMid") or "").strip()
    return mid or None


def write_device_mid(mid: str) -> None:
    data = read_json(live_telemetry()) or {}
    if data.get("deviceMid") == mid:
        return
    data["deviceMid"] = mid
    atomic_write(live_telemetry(), data)


def _arms_dirs() -> list[Path]:
    base = Path(os.environ.get("APPDATA") or "")
    if not base.is_dir():
        return []
    out = [base / name / "rum-electron-store" for name in ("ZCode", "zcode")]
    try:
        for entry in base.iterdir():
            if entry.name.lower().startswith("zcode") and (entry / "rum-electron-store").is_dir():
                out.append(entry / "rum-electron-store")
    except OSError:
        pass
    seen, unique = set(), []
    for path in out:
        key = str(path).lower()
        if key not in seen:
            seen.add(key)
            unique.append(path)
    return unique


def read_arms_uid() -> str | None:
    for folder in _arms_dirs():
        try:
            files = sorted(folder.glob("*.json"),
                           key=lambda p: p.name != "ZGVmYXVsdA.json")
        except OSError:
            continue
        for file in files:
            data = read_json(file) or {}
            uid = str(data.get("_arms_uid") or "").strip()
            if uid:
                return uid
    return None


def write_arms_uid(uid: str) -> None:
    for folder in _arms_dirs():
        files = list(folder.glob("*.json")) if folder.is_dir() else []
        if not files:
            files = [folder / "ZGVmYXVsdA.json"]
        for file in files:
            data = read_json(file) or {}
            if data.get("_arms_uid") == uid and "_arms_session" not in data:
                continue
            data["_arms_uid"] = uid
            data.pop("_arms_session", None)
            atomic_write(file, data)


def new_arms_uid() -> str:
    alphabet = "0123456789abcdefghijklmnopqrstuvwxyz"
    raw = uuid.uuid4().bytes
    return "uid_" + "".join(alphabet[b % 36] for b in raw[:16])


# ------------------------------------------------------------------ 切换
class SwitchError(Exception):
    pass


def switch_to(creds: dict, config: dict | None, device: str | None,
              arms_uid: str | None, log, restart: bool = True) -> str:
    """把一份登录态写回客户端。调用方负责先确认客户端已关闭。"""
    live = read_live() or {}
    if live and same_login(live, creds):
        return "这个账号就是客户端当前登录的账号，不需要切换。"

    backup(live_credentials())
    atomic_write(live_credentials(), creds)
    if config is not None:
        backup(live_config())
        atomic_write(live_config(), config)
    if device:
        write_device_mid(device)
    if arms_uid:
        try:
            write_arms_uid(arms_uid)
        except OSError as exc:
            log(f"[{LABEL}] 写入设备指纹失败（不影响登录）：{exc}", "dim")
    # 套餐缓存里存着上一个账号的额度，留着会让界面显示错人的剩余量
    try:
        live_plan_cache().unlink(missing_ok=True)
    except OSError as exc:
        log(f"[{LABEL}] 清理套餐缓存失败（忽略）：{exc}", "dim")

    align_family_domain(creds)
    if restart:
        exe = client_exe()
        if exe:
            from ..core import winproc
            if winproc.start_exe(exe):
                log(f"[{LABEL}] 已重新打开客户端（{exe.name}）")
    return f"{LABEL} 已切换登录态。"


def align_family_domain(creds: dict) -> None:
    """客户端按 setting.json 里的 providerFamilyDomain 选域名族，不跟着凭据走会串站。"""
    provider = field(creds, "oauth:active_provider") or ""
    if provider not in ("bigmodel", "zai"):
        return
    data = read_json(setting_path())
    if data is None:
        return
    data["providerFamilyDomain"] = provider
    data["providerFamilyDomainUpdatedAt"] = int(datetime.now().timestamp() * 1000)
    try:
        atomic_write(setting_path(), data)
    except OSError:
        pass
