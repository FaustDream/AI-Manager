# -*- coding: utf-8 -*-
"""客户端生命周期闸门：写会话前关闭、写完自动重启。

官方客户端把会话缓存在内存里，带着活进程写库会被回写覆盖；但客户端是用户开的，
关掉就得还回去——所以重启放在 finally，写失败也照样执行。只重启我们关掉的，
本来没开的不替用户启动。
"""
from __future__ import annotations

import time
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path

from ..core import winproc

RESTART_DELAY = 2.0


def _noop(_message: str, _level: str = "info") -> None:
    pass


@dataclass(frozen=True)
class Client:
    """一个可被关闭 / 重启的官方客户端。"""
    name: str            # 进程名，如 "Qoder CN.exe"
    label: str           # 界面用名
    exe: Path | None     # 安装路径，重启用；找不到就只能提示手动打开


def workbuddy_clients() -> list[Client]:
    from .editions import Edition
    return [Client(ep.proc, ep.label, ep.exe) for ep in (Edition("cn"), Edition("intl"))]


def close_many(clients, log=None) -> list[Client]:
    """关掉正在跑的客户端；关不掉就抛错，绝不带活写库。"""
    log = log or _noop
    closed = []
    for client in clients:
        if not winproc.process_running(client.name):
            continue
        log(f"正在关闭 {client.label}…")
        if not winproc.kill_process(client.name):
            raise RuntimeError(f"{client.label} 没能关闭，请手动退出后重试")
        closed.append(client)
    return closed


def restart_many(closed, log=None) -> None:
    log = log or _noop
    if not closed:
        return
    time.sleep(RESTART_DELAY)          # 退出后文件句柄要等系统回收，急着拉起容易撞上
    for client in closed:
        if client.exe is not None and client.exe.exists() and winproc.start_exe(client.exe):
            log(f"已重启 {client.label}")
        else:
            log(f"{client.label} 重启失败，请手动打开客户端", "warn")


@contextmanager
def client_guard(clients, log=None):
    """写入闸门：关闭 → 执行写入 → 自动重启（失败也重启）。"""
    closed = close_many(clients, log)
    try:
        yield closed
    finally:
        restart_many(closed, log)
