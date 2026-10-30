# -*- coding: utf-8 -*-
"""统一路径管理。

开发态与打包态都把「程序目录」和「数据目录」分开：
- 程序目录：源码 / exe 所在处，只读资源（图标、qss）
- 数据目录：<程序目录>/data，配置、账号库、积分台账、账号档案、日志都放这里，
  便于整个文件夹拷走即用（免安装）。
"""
from __future__ import annotations

import os
import sys
from pathlib import Path

IS_WINDOWS = os.name == "nt"
IS_MACOS = sys.platform == "darwin"


def app_dir() -> Path:
    if getattr(sys, "frozen", False):
        return Path(sys.executable).resolve().parent
    # app/core/paths.py -> app/core -> app -> 项目根
    return Path(__file__).resolve().parents[2]


def resource_path(name: str) -> Path:
    base = Path(getattr(sys, "_MEIPASS", app_dir()))
    return base / name


APP_HOME = app_dir()
DATA_DIR = Path(os.environ.get("AIMANAGER_DATA") or (APP_HOME / "data")).resolve()
LOG_DIR = DATA_DIR / "logs"
ASSETS_DIR = resource_path("assets")

CONFIG_FILE = DATA_DIR / "config.json"
ACCOUNTS_FILE = DATA_DIR / "accounts.dat"      # DPAPI / Fernet 加密
ACCOUNTS_KEY_FILE = DATA_DIR / ".accounts.key"  # 非 Windows 的本地加密密钥
CHECKED_FILE = DATA_DIR / "checked.json"       # 今日已签到记录
CHECKIN_STATUS_FILE = DATA_DIR / "checkin_status.json"  # 当天签到状态（一天只查一次）
DAILY_FILE = DATA_DIR / "daily.json"           # 每日轮询状态（日期 + 当日已签账号）
LEDGER_FILE = DATA_DIR / "credits.json"        # 签到积分台账
CREDIT_HISTORY_FILE = DATA_DIR / "credit_history.json"  # 积分余额按天快照（算增减用）
SESSION_INDEX_FILE = DATA_DIR / "session_index.json"  # 会话归属索引（原始账号 / 引用方）
PROFILE_ROOT = DATA_DIR / "profiles"           # 账号档案（含登录态）
HUB_FILE = DATA_DIR / "hub.json"               # 技能 / MCP 的统一登记与平台归属
SKILL_LIBRARY = DATA_DIR / "skills"            # 技能库：一个技能一个目录，内含 SKILL.md
LOG_FILE = LOG_DIR / "app.log"

ICON_FILE = ASSETS_DIR / "app.ico"

# 会话管家数据：备份、回收站、删除黑名单，全部收进 data 目录，随程序一起拷走
BACKUP_ROOT = DATA_DIR / "backups"
HUB_BACKUP_ROOT = BACKUP_ROOT / "hub"          # 写平台 MCP 配置前的原件备份
TRASH_ROOT = DATA_DIR / "trash"
TOMB_FILE = DATA_DIR / "tombstones.json"

# 旧版签到中心的数据文件，仅用于首次启动迁移
LEGACY_CHECKIN_DIR = Path("D:/CheckinHub")
LEGACY_SYNC_DIR = Path("D:/workbuddy-session-sync")
# 旧版会话管家的家目录数据（历史备份/回收站），首次启动时迁入 data
LEGACY_SYNC_HOME = Path.home() / ".workbuddy-session-sync"


def ensure_dirs() -> None:
    for path in (DATA_DIR, LOG_DIR, PROFILE_ROOT, BACKUP_ROOT, TRASH_ROOT,
                 SKILL_LIBRARY):
        path.mkdir(parents=True, exist_ok=True)


def user_appdata_dir() -> Path:
    """被管理的客户端存配置的目录（%APPDATA% 那一层）。

    三家客户端只有 Windows 版，非 Windows 上这里永远扫不到东西；但至少要返回一个
    合法绝对路径——环境变量为空时直接拼 `Path("") / "com.qoder.app.stable"` 会变成
    相对路径，等于拿当前工作目录去碰运气。
    """
    raw = os.environ.get("APPDATA")
    if raw:
        return Path(raw)
    if IS_MACOS:
        return Path.home() / "Library" / "Application Support"
    return Path(os.environ.get("XDG_CONFIG_HOME") or (Path.home() / ".config"))


def user_local_dir() -> Path:
    """客户端的安装目录（%LOCALAPPDATA% 那一层），语义同 user_appdata_dir。"""
    raw = os.environ.get("LOCALAPPDATA")
    if raw:
        return Path(raw)
    if IS_MACOS:
        return Path.home() / "Library" / "Application Support"
    return Path(os.environ.get("XDG_DATA_HOME") or (Path.home() / ".local" / "share"))
