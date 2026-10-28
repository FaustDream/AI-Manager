# -*- coding: utf-8 -*-
"""首次启动的历史数据迁移。

把旧「签到中心」的 config/accounts/checked/credits 与旧「会话管家」的账号档案
搬到新数据目录。只在目标不存在时复制，绝不覆盖新数据。
"""
from __future__ import annotations

import shutil

from . import paths

CHECKIN_FILES = ("config.json", "accounts.dat", "checked.json", "credits.json")


def run_once() -> list[str]:
    moved: list[str] = []
    legacy = paths.LEGACY_CHECKIN_DIR

    for name in CHECKIN_FILES:
        src = legacy / name
        dst = paths.DATA_DIR / name
        if src.exists() and not dst.exists():
            try:
                shutil.copy2(src, dst)
                moved.append(name)
            except OSError:
                pass

    # 账号档案（含登录态，体积可能几十 MB）
    legacy_profiles = paths.LEGACY_SYNC_DIR / "profiles"
    if legacy_profiles.is_dir():
        for edition_dir in legacy_profiles.iterdir():
            if not edition_dir.is_dir():
                continue
            target = paths.PROFILE_ROOT / edition_dir.name
            if target.exists():
                continue
            try:
                shutil.copytree(edition_dir, target)
                moved.append(f"profiles/{edition_dir.name}")
            except OSError:
                pass

    # 历史备份与删除黑名单（回收站本来就按需重建，不用搬）
    legacy_home = paths.LEGACY_SYNC_HOME
    legacy_backup = legacy_home / "backups"
    if legacy_backup.is_dir() and paths.BACKUP_ROOT.exists():
        for item in legacy_backup.iterdir():
            target = paths.BACKUP_ROOT / item.name
            if target.exists():
                continue
            try:
                if item.is_dir():
                    shutil.copytree(item, target)
                else:
                    shutil.copy2(item, target)
                moved.append(f"backups/{item.name}")
            except OSError:
                pass
    legacy_tomb = legacy_home / "tombstones.json"
    if legacy_tomb.exists() and not paths.TOMB_FILE.exists():
        try:
            shutil.copy2(legacy_tomb, paths.TOMB_FILE)
            moved.append("tombstones.json")
        except OSError:
            pass
    return moved
