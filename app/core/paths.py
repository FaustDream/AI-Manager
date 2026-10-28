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
ACCOUNTS_FILE = DATA_DIR / "accounts.dat"      # DPAPI 加密
CHECKED_FILE = DATA_DIR / "checked.json"       # 今日已签到记录
DAILY_FILE = DATA_DIR / "daily.json"           # 每日轮询状态（日期 + 当日已签账号）
LEDGER_FILE = DATA_DIR / "credits.json"        # 签到积分台账
SESSION_INDEX_FILE = DATA_DIR / "session_index.json"  # 会话归属索引（原始账号 / 引用方）
PROFILE_ROOT = DATA_DIR / "profiles"           # 账号档案（含登录态）
LOG_FILE = LOG_DIR / "app.log"

ICON_FILE = ASSETS_DIR / "app.ico"

# 会话管家数据：备份、回收站、删除黑名单，全部收进 data 目录，随程序一起拷走
BACKUP_ROOT = DATA_DIR / "backups"
TRASH_ROOT = DATA_DIR / "trash"
TOMB_FILE = DATA_DIR / "tombstones.json"

# 旧版签到中心的数据文件，仅用于首次启动迁移
LEGACY_CHECKIN_DIR = Path("D:/CheckinHub")
LEGACY_SYNC_DIR = Path("D:/workbuddy-session-sync")
# 旧版会话管家的家目录数据（历史备份/回收站），首次启动时迁入 data
LEGACY_SYNC_HOME = Path.home() / ".workbuddy-session-sync"


def ensure_dirs() -> None:
    for path in (DATA_DIR, LOG_DIR, PROFILE_ROOT, BACKUP_ROOT, TRASH_ROOT):
        path.mkdir(parents=True, exist_ok=True)
