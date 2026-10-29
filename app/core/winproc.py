# -*- coding: utf-8 -*-
"""Windows 进程与文件系统小工具（启动客户端、判断是否在运行、打开目录）。

为什么不用 tasklist / taskkill：
    这是个 pythonw 启动的 GUI 程序，每次 subprocess 调用控制台程序都会闪一个黑框，
    而且 tasklist 单次约 150ms、taskkill 后还要反复轮询，菜单里点一次就是几百毫秒的
    卡顿 + 若干次闪窗。这里改成 ctypes 调 kernel32 的进程快照，单次 1ms 级别、
    完全不产生窗口；只有 ctypes 不可用时才退回子进程，并统一加 CREATE_NO_WINDOW。
"""
from __future__ import annotations

import ctypes
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

# 创建子进程时不分配控制台窗口（windows 专用 flag，其他平台传 0）
CREATE_NO_WINDOW = 0x08000000
IS_WINDOWS = os.name == "nt"
IS_MACOS = sys.platform == "darwin"
NO_WINDOW_FLAGS = CREATE_NO_WINDOW if IS_WINDOWS else 0

if IS_WINDOWS:
    from ctypes import wintypes
else:
    # ctypes.wintypes 在非 Windows 上导入即抛 ValueError（_type_ 'v' not supported），
    # 所以整个 Win32 结构体定义要跟着平台走，不能只把调用点包起来。
    wintypes = None  # type: ignore[assignment]


def _hidden_startupinfo():
    """双保险：即使某个子进程忽略了 CREATE_NO_WINDOW，也指定 SW_HIDE。"""
    if not IS_WINDOWS:
        return None
    info = subprocess.STARTUPINFO()
    info.dwFlags |= subprocess.STARTF_USESHOWWINDOW
    info.wShowWindow = subprocess.SW_HIDE
    return info

TH32CS_SNAPPROCESS = 0x00000002
PROCESS_TERMINATE = 0x0001
INVALID_HANDLE_VALUE = ctypes.c_void_p(-1).value
MAX_PATH = 260

# 进程表缓存：短时间内多次查询（如一批账号/多个客户端）只扫一次
_CACHE_TTL = 0.8
_cache_at = 0.0
_cache_map: dict[str, list[int]] = {}


if IS_WINDOWS:
    class PROCESSENTRY32W(ctypes.Structure):
        _fields_ = [
            ("dwSize", wintypes.DWORD),
            ("cntUsage", wintypes.DWORD),
            ("th32ProcessID", wintypes.DWORD),
            ("th32DefaultHeapID", ctypes.c_void_p),
            ("th32ModuleID", wintypes.DWORD),
            ("cntThreads", wintypes.DWORD),
            ("th32ParentProcessID", wintypes.DWORD),
            ("pcPriClassBase", ctypes.c_long),
            ("dwFlags", wintypes.DWORD),
            ("szExeFile", wintypes.WCHAR * MAX_PATH),
        ]
else:
    PROCESSENTRY32W = None  # type: ignore[assignment,misc]


def _kernel32():
    if not IS_WINDOWS:
        raise OSError("kernel32 仅在 Windows 上可用")
    return ctypes.WinDLL("kernel32", use_last_error=True)


def _snapshot_ctypes() -> dict[str, list[int]] | None:
    """进程名（小写）→ pid 列表；ctypes 不可用时返回 None。"""
    if not IS_WINDOWS:
        return None
    try:
        kernel32 = _kernel32()
        kernel32.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
        kernel32.CreateToolhelp32Snapshot.argtypes = [wintypes.DWORD, wintypes.DWORD]
        kernel32.Process32FirstW.argtypes = [wintypes.HANDLE,
                                            ctypes.POINTER(PROCESSENTRY32W)]
        kernel32.Process32NextW.argtypes = [wintypes.HANDLE,
                                            ctypes.POINTER(PROCESSENTRY32W)]
    except (AttributeError, OSError):
        return None

    snap = kernel32.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
    if not snap or snap == INVALID_HANDLE_VALUE:
        return None
    table: dict[str, list[int]] = {}
    try:
        entry = PROCESSENTRY32W()
        entry.dwSize = ctypes.sizeof(PROCESSENTRY32W)
        ok = kernel32.Process32FirstW(snap, ctypes.byref(entry))
        while ok:
            table.setdefault(entry.szExeFile.lower(), []).append(int(entry.th32ProcessID))
            ok = kernel32.Process32NextW(snap, ctypes.byref(entry))
    finally:
        kernel32.CloseHandle(snap)
    return table


def _snapshot_subprocess() -> dict[str, list[int]]:
    """兜底：非 Windows 或 ctypes 失败时用系统命令，隐藏窗口。"""
    if not IS_WINDOWS:
        return {}
    try:
        out = subprocess.run(
            ["tasklist", "/NH", "/FO", "CSV"],
            capture_output=True, text=True, errors="ignore", timeout=20,
            creationflags=NO_WINDOW_FLAGS,
            startupinfo=_hidden_startupinfo(),
            stdin=subprocess.DEVNULL,
        ).stdout
    except (OSError, subprocess.SubprocessError):
        return {}
    table: dict[str, list[int]] = {}
    for line in out.splitlines():
        parts = [part.strip('" ') for part in line.split('","')]
        if len(parts) < 2:
            continue
        name, pid = parts[0].strip('"'), parts[1].strip('"').replace(",", "")
        if pid.isdigit():
            table.setdefault(name.lower(), []).append(int(pid))
    return table


def _snapshot_posix() -> dict[str, list[int]]:
    """Linux 读 /proc，macOS 走 ps；进程名统一取 basename 再转小写。"""
    table: dict[str, list[int]] = {}
    proc_root = Path("/proc")
    if proc_root.is_dir():
        for entry in proc_root.iterdir():
            if not entry.name.isdigit():
                continue
            try:
                name = (entry / "comm").read_text(encoding="utf-8", errors="ignore").strip()
            except OSError:
                continue
            if name:
                table.setdefault(name.lower(), []).append(int(entry.name))
        return table
    try:
        out = run_hidden(["ps", "-Ao", "pid=,comm="], timeout=20).stdout or ""
    except (OSError, subprocess.SubprocessError):
        return {}
    for line in out.splitlines():
        parts = line.strip().split(None, 1)
        if len(parts) == 2 and parts[0].isdigit():
            table.setdefault(Path(parts[1].strip()).name.lower(), []).append(int(parts[0]))
    return table


def _process_table(force: bool = False) -> dict[str, list[int]]:
    """带 0.8 秒缓存的进程表，避免同一批调用重复扫表。"""
    global _cache_at, _cache_map
    now = time.monotonic()
    if not force and _cache_map and now - _cache_at < _CACHE_TTL:
        return _cache_map
    if IS_WINDOWS:
        table = _snapshot_ctypes()
        if table is None:
            table = _snapshot_subprocess()
    else:
        table = _snapshot_posix()
    _cache_map = table
    _cache_at = now
    return table


def _invalidate_cache() -> None:
    global _cache_at
    _cache_at = 0.0


def process_running(name: str) -> bool:
    """按可执行文件名判断进程是否在跑（不区分大小写）。"""
    if not name:
        return False
    return bool(_process_table().get(name.lower()))


def any_running(names) -> bool:
    table = _process_table()
    return any(table.get(str(name).lower()) for name in names)


def running_names(names) -> list[str]:
    """一次扫表判断多个名字，返回正在运行的那些（顺序与入参一致）。"""
    table = _process_table()
    return [name for name in names if table.get(str(name).lower())]


def _terminate(pid: int) -> bool:
    if not IS_WINDOWS:
        try:
            os.kill(pid, signal.SIGTERM)
            return True
        except OSError:
            return False
    try:
        kernel32 = _kernel32()
        kernel32.OpenProcess.restype = wintypes.HANDLE
        handle = kernel32.OpenProcess(PROCESS_TERMINATE, False, pid)
        if not handle:
            return False
        try:
            return bool(kernel32.TerminateProcess(handle, 1))
        finally:
            kernel32.CloseHandle(handle)
    except (AttributeError, OSError):
        return False


def kill_process(name: str, wait_seconds: int = 15) -> bool:
    """结束指定进程并等待退出；用于写数据库/登录态前的保护。"""
    if not name:
        return True
    pids = _process_table().get(name.lower()) or []
    if not pids:
        return True
    for pid in pids:
        _terminate(pid)
    _invalidate_cache()
    # 250ms 一轮，最多等 wait_seconds；ctypes 查询不产生任何窗口
    deadline = time.monotonic() + max(1, wait_seconds)
    while time.monotonic() < deadline:
        if not process_running(name):
            return True
        time.sleep(0.25)
    return not process_running(name)


def _launch(path: Path) -> bool:
    """交给系统默认程序打开；Windows 用 startfile，其余平台用 open / xdg-open。"""
    try:
        if IS_WINDOWS:
            os.startfile(str(path))  # type: ignore[attr-defined]
        else:
            opener = "open" if IS_MACOS else "xdg-open"
            subprocess.Popen(
                [opener, str(path)],
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL, start_new_session=True,
            )
        return True
    except (OSError, subprocess.SubprocessError):
        return False


def start_exe(exe: Path | str) -> bool:
    """用系统默认方式启动，等同于双击（Electron 客户端需要这种启动方式）。

    比 subprocess.Popen 更安全：不继承控制台，不会闪黑框。
    """
    path = Path(exe)
    if not path.exists():
        return False
    return _launch(path)


def open_path(path: Path | str) -> bool:
    path = Path(path)
    if not path.exists():
        return False
    return _launch(path)


def run_hidden(args, timeout: int = 60, text: bool = True) -> subprocess.CompletedProcess:
    """调用外部命令但绝不出窗口，供确有必要的场景（如创建快捷方式）使用。"""
    return subprocess.run(
        args, capture_output=True, text=text, errors="replace", timeout=timeout,
        creationflags=NO_WINDOW_FLAGS,
        startupinfo=_hidden_startupinfo(),
        stdin=subprocess.DEVNULL,
    )
