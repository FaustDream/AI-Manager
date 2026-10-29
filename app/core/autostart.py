# -*- coding: utf-8 -*-
"""开机自启：写当前用户的 Run 注册表项，不依赖任何第三方库。

用 HKCU 而不是 HKLM：不需要管理员权限，卸载时也不会在其他账号下留垃圾。
启动命令不再带额外参数：每日签到轮询是程序启动后的固定动作，开机自启
与手动双击走同一条路径。

只有 Windows 有这条注册表路径；其他平台（实验性构建）一律当作「不支持」，
is_enabled 恒为 False、enable 抛 OSError，界面沿用既有的失败提示，不需要额外分支。
"""
from __future__ import annotations

import sys
from pathlib import Path

try:
    import winreg
except ImportError:                     # 非 Windows 平台没有 winreg
    winreg = None                       # type: ignore[assignment]

RUN_KEY = r"Software\Microsoft\Windows\CurrentVersion\Run"
VALUE_NAME = "AI Manager"

SUPPORTED = winreg is not None and sys.platform == "win32"


def _command() -> str:
    """打包后是 exe 自己；源码运行时用 pythonw 拉起 run.pyw，避免开机闪黑框。"""
    exe = Path(sys.executable)
    if exe.name.lower().startswith("python"):
        windowless = exe.with_name("pythonw.exe")
        if windowless.exists():
            exe = windowless
        root = Path(__file__).resolve().parents[2]
        return f'"{exe}" "{root / "run.pyw"}"'
    return f'"{exe}"'


def command_line() -> str:
    return _command()


def is_enabled() -> bool:
    if not SUPPORTED:
        return False
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, RUN_KEY) as key:
            value, _kind = winreg.QueryValueEx(key, VALUE_NAME)
            return bool(str(value).strip())
    except OSError:
        return False


def enable() -> str:
    if not SUPPORTED:
        raise OSError("开机自启只支持 Windows")
    command = _command()
    with winreg.CreateKeyEx(winreg.HKEY_CURRENT_USER, RUN_KEY, 0,
                            winreg.KEY_SET_VALUE) as key:
        winreg.SetValueEx(key, VALUE_NAME, 0, winreg.REG_SZ, command)
    return command


def disable() -> None:
    if not SUPPORTED:
        return
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, RUN_KEY, 0,
                            winreg.KEY_SET_VALUE) as key:
            winreg.DeleteValue(key, VALUE_NAME)
    except FileNotFoundError:
        pass
