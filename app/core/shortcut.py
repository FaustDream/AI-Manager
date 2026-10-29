# -*- coding: utf-8 -*-
"""桌面快捷方式：不打包也能双击启动。

快捷方式指向 pythonw.exe + run.pyw，工作目录是项目根，图标用 assets/app.ico。
用 PowerShell 的 -EncodedCommand（UTF-16LE + base64）传脚本，彻底避开中文路径
和引号转义问题；该调用带 CREATE_NO_WINDOW，不会闪黑框。

桌面路径优先读注册表：以前用 PowerShell 问系统要路径，单次要 800ms，
而「设置」页每次显示都要判断快捷方式是否存在，直接把界面拖卡。
"""
from __future__ import annotations

import base64
import os
import sys
from pathlib import Path

from .. import brand
from . import paths, winproc

SHORTCUT_NAME = "AI Manager"

# .lnk 是 Windows 独有的东西；其他平台（实验性构建）只把入口置灰，不做假实现
SUPPORTED = os.name == "nt"

_desktop_cache: Path | None = None


def _launcher() -> tuple[Path, str]:
    """返回 (可执行文件, 参数)。打包后直接指向 exe，开发态指向 pythonw + run.pyw。"""
    if getattr(sys, "frozen", False):
        return Path(sys.executable), ""
    entry = paths.APP_HOME / "run.pyw"
    pythonw = Path(sys.executable).with_name("pythonw.exe")
    exe = pythonw if pythonw.exists() else Path(sys.executable)
    return exe, f'"{entry}"'


def _ps_quote(text: str) -> str:
    return "'" + str(text).replace("'", "''") + "'"


def _desktop_from_registry() -> Path | None:
    """HKCU 的 User Shell Folders 里存的就是最终桌面路径（含 OneDrive 重定向）。"""
    if os.name != "nt":
        return None
    try:
        import winreg
    except ImportError:
        return None
    candidates = (
        (winreg.HKEY_CURRENT_USER,
         r"Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders"),
        (winreg.HKEY_CURRENT_USER,
         r"Software\Microsoft\Windows\CurrentVersion\Explorer\Shell Folders"),
    )
    for root, key_path in candidates:
        try:
            with winreg.OpenKey(root, key_path) as key:
                value, _kind = winreg.QueryValueEx(key, "Desktop")
        except OSError:
            continue
        if not value:
            continue
        expanded = Path(os.path.expandvars(str(value)))
        if expanded.is_dir():
            return expanded
    return None


def desktop_dir(refresh: bool = False) -> Path:
    """桌面目录。注册表 → 环境变量 → 用户目录，全程不起子进程。"""
    global _desktop_cache
    if _desktop_cache is not None and not refresh:
        return _desktop_cache
    found = _desktop_from_registry()
    if found is None:
        for candidate in (Path(os.environ.get("USERPROFILE", "")) / "Desktop",
                          Path.home() / "Desktop"):
            if candidate.is_dir():
                found = candidate
                break
    _desktop_cache = found or (Path.home() / "Desktop")
    return _desktop_cache


def create_desktop_shortcut(name: str = SHORTCUT_NAME) -> Path:
    if not SUPPORTED:
        raise RuntimeError("桌面快捷方式只支持 Windows")
    exe, args = _launcher()
    icon = paths.ICON_FILE if paths.ICON_FILE.exists() else exe
    script = f"""
$ErrorActionPreference = 'Stop'
$desktop = [Environment]::GetFolderPath('Desktop')
if (-not (Test-Path $desktop)) {{ $desktop = Join-Path $env:USERPROFILE 'Desktop' }}
$lnk = Join-Path $desktop {_ps_quote(name + '.lnk')}
$shell = New-Object -ComObject WScript.Shell
$sc = $shell.CreateShortcut($lnk)
$sc.TargetPath = {_ps_quote(str(exe))}
$sc.Arguments = {_ps_quote(args)}
$sc.WorkingDirectory = {_ps_quote(str(paths.APP_HOME))}
$sc.IconLocation = {_ps_quote(str(icon))}
$sc.Description = {_ps_quote(f'AI Manager - 签到 / 会话 / 账号一体化工作台 · by {brand.AUTHOR}')}
$sc.WindowStyle = 1
$sc.Save()
Write-Output $lnk
"""
    encoded = base64.b64encode(script.encode("utf-16-le")).decode("ascii")
    result = winproc.run_hidden(
        ["powershell", "-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden",
         "-EncodedCommand", encoded],
        timeout=60,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout or "").strip() or f"退出码 {result.returncode}"
        raise RuntimeError(f"创建快捷方式失败：{detail}")
    target = Path((result.stdout or "").strip().splitlines()[-1]) if result.stdout.strip() else None
    if target is None or not target.exists():
        fallback = desktop_dir(refresh=True) / f"{name}.lnk"
        if fallback.exists():
            return fallback
        raise RuntimeError("系统没有返回快捷方式文件，请检查桌面写入权限")
    return target


def shortcut_exists(name: str = SHORTCUT_NAME) -> Path | None:
    path = desktop_dir() / f"{name}.lnk"
    return path if path.exists() else None
