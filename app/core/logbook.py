# -*- coding: utf-8 -*-
"""运行日志落盘：界面日志和文件日志同源，便于事后排查。

写盘走内存缓冲 + 定时 flush：会话同步时一次可能刷几百行，
逐行 open/close 会把 GUI 线程拖住（每次约 1ms，几百行就是几百毫秒）。
进程退出前由主窗口调用 flush() 兜底，正常关闭不会丢日志。
"""
from __future__ import annotations

import threading
from datetime import datetime

from . import paths

_BUFFER: list[str] = []
_LOCK = threading.Lock()
_MAX_PENDING = 200


def _flush_locked() -> None:
    if not _BUFFER:
        return
    chunk = "\n".join(_BUFFER) + "\n"
    _BUFFER.clear()
    try:
        paths.LOG_DIR.mkdir(parents=True, exist_ok=True)
        with open(paths.LOG_FILE, "a", encoding="utf-8") as fh:
            fh.write(chunk)
    except OSError:
        pass


def append(text: str) -> str:
    """记录一行日志（先进缓冲）并返回带时间戳的整行文本。"""
    line = f"[{datetime.now():%Y-%m-%d %H:%M:%S}] {text}"
    with _LOCK:
        _BUFFER.append(line)
        if len(_BUFFER) >= _MAX_PENDING:
            _flush_locked()
    return line


def flush() -> None:
    """把缓冲写入磁盘；界面定时调用，退出时也会调用。"""
    with _LOCK:
        _flush_locked()
