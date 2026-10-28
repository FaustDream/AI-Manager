# -*- coding: utf-8 -*-
"""开机自启：写当前用户的 Run 注册表项，不依赖任何第三方库。

用 HKCU 而不是 HKLM：不需要管理员权限，卸载时也不会在其他账号下留垃圾。
启动命令带 --boot-checkin，程序起来后先刷新签到状态再决定签谁。
"""
from __future__ import annotations

import sys
import winreg
from pathlib import Path

RUN_KEY = r"Software\Microsoft\Windows\CurrentVersion\Run"
VALUE_NAME = "AI Manager"
BOOT_FLAG = "--boot-checkin"


def _command() -> str:
    """打包后是 exe 自己；源码运行时用 pythonw 拉起 run.pyw，避免开机闪黑框。"""
    exe = Path(sys.executable)
    if exe.name.lower().startswith("python"):
        windowless = exe.with_name("pythonw.exe")
        if windowless.exists():
            exe = windowless
        root = Path(__file__).resolve().parents[2]
        return f'"{exe}" "{root / "run.pyw"}" {BOOT_FLAG}'
    return f'"{exe}" {BOOT_FLAG}'


def command_line() -> str:
    return _command()


def is_enabled() -> bool:
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, RUN_KEY) as key:
            value, _kind = winreg.QueryValueEx(key, VALUE_NAME)
            return bool(str(value).strip())
    except OSError:
        return False


def enable() -> str:
    command = _command()
    with winreg.CreateKeyEx(winreg.HKEY_CURRENT_USER, RUN_KEY, 0,
                            winreg.KEY_SET_VALUE) as key:
        winreg.SetValueEx(key, VALUE_NAME, 0, winreg.REG_SZ, command)
    return command


def disable() -> None:
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, RUN_KEY, 0,
                            winreg.KEY_SET_VALUE) as key:
            winreg.DeleteValue(key, VALUE_NAME)
    except FileNotFoundError:
        pass
