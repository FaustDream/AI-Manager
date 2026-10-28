# -*- coding: utf-8 -*-
"""后台任务：把耗时动作丢到线程池，结果用信号回到界面线程。

用法：
    submit(lambda emit: do_work(emit), on_done=..., on_error=..., on_partial=...)

约定 fn 接收一个 emit 回调，需要分批刷新界面时调用 emit(obj)。

实现要点：信号集中在一个与进程同生命周期的 Dispatcher 上，每个任务只带一个
token。这样即使 QRunnable 被 Qt 回收，也不会出现「信号对象已删除」导致结果丢失。
"""
from __future__ import annotations

import itertools
import threading
import traceback
from concurrent.futures import ThreadPoolExecutor
from typing import Any, Callable, Iterable

from PySide6.QtCore import QObject, QRunnable, QThreadPool, Signal


class _Dispatcher(QObject):
    partial = Signal(int, object)
    done = Signal(int, object)
    error = Signal(int, str)


_DISPATCH = _Dispatcher()
_SEQ = itertools.count(1)
_LOCK = threading.Lock()
_CALLBACKS: dict[int, tuple] = {}


def _on_done(token: int, result: Any) -> None:
    with _LOCK:
        callbacks = _CALLBACKS.pop(token, None)
    if not callbacks:
        return
    handler = callbacks[0]
    if handler is None:
        return
    try:
        handler(result)
    except RuntimeError:
        pass
    except Exception:
        traceback.print_exc()


def _on_error(token: int, message: str) -> None:
    with _LOCK:
        callbacks = _CALLBACKS.pop(token, None)
    if not callbacks:
        return
    handler = callbacks[1]
    if handler is None:
        return
    try:
        handler(message)
    except RuntimeError:
        pass
    except Exception:
        traceback.print_exc()


def _on_partial(token: int, payload: Any) -> None:
    with _LOCK:
        callbacks = _CALLBACKS.get(token)
    if not callbacks or callbacks[2] is None:
        return
    try:
        callbacks[2](payload)
    except RuntimeError:
        pass
    except Exception:
        traceback.print_exc()


_DISPATCH.done.connect(_on_done)
_DISPATCH.error.connect(_on_error)
_DISPATCH.partial.connect(_on_partial)


class Job(QRunnable):
    def __init__(self, fn: Callable[[Callable[[Any], None]], Any], token: int) -> None:
        super().__init__()
        self.fn = fn
        self.token = token
        self.setAutoDelete(True)

    def run(self) -> None:  # noqa: D102 - QRunnable 接口
        token = self.token
        try:
            result = self.fn(lambda payload: _DISPATCH.partial.emit(token, payload))
        except Exception as exc:
            detail = "".join(traceback.format_exception_only(type(exc), exc)).strip()
            try:
                _DISPATCH.error.emit(token, f"{detail}\n{traceback.format_exc()}")
            except RuntimeError:
                pass          # 退出过程中信号对象已被回收，忽略即可
        else:
            try:
                _DISPATCH.done.emit(token, result)
            except RuntimeError:
                pass


def submit(
    fn: Callable[[Callable[[Any], None]], Any],
    on_done: Callable[[Any], None] | None = None,
    on_error: Callable[[str], None] | None = None,
    on_partial: Callable[[Any], None] | None = None,
) -> None:
    token = next(_SEQ)
    with _LOCK:
        _CALLBACKS[token] = (on_done, on_error, on_partial)
    QThreadPool.globalInstance().start(Job(fn, token))


def parallel_map(items: Iterable, worker: Callable, max_workers: int = 4) -> None:
    """在后台任务里并发跑一组互不依赖的请求（如多账号状态查询）。"""
    items = list(items)
    if not items:
        return
    with ThreadPoolExecutor(max_workers=max_workers) as pool:
        list(pool.map(worker, items))
