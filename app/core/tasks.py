# -*- coding: utf-8 -*-
"""后台任务：统一排队、并行执行、状态与进度跟踪。

所有耗时动作都从 submit() 进队列：默认并行跑（4 个 worker），
标了 serial 的任务（会写账号库 / 登录态）走串行通道，排队一个一个来，
两者互不挡路——并行任务不会被写盘任务卡住，写盘任务之间也不会互相踩。

任务状态（排队中 / 执行中 / 已完成 / 失败 / 已取消）与进度记在任务表里，
任务监控页实时读这张表；worker 里调 report() 即可上报进度，不用改函数签名。
"""
from __future__ import annotations

import itertools
import sys
import threading
import time
import traceback
from concurrent.futures import Future, ThreadPoolExecutor
from dataclasses import dataclass, field
from typing import Any, Callable, Iterable

from PySide6.QtCore import QObject, Signal

from . import logbook

PARALLEL_WORKERS = 4

STATE_TEXT = {
    "queued": "排队中",
    "running": "执行中",
    "done": "已完成",
    "failed": "失败",
    "cancelled": "已取消",
}


@dataclass
class Task:
    id: int
    name: str
    state: str = "queued"
    progress: float | None = None      # 0~1，None 表示没有进度上报
    message: str = ""
    created: float = field(default_factory=time.time)
    started: float | None = None
    finished: float | None = None
    error: str = ""
    serial: bool = False

    @property
    def duration(self) -> float:
        end = self.finished or time.time()
        start = self.started or self.created
        return max(0.0, end - start)


class _Dispatcher(QObject):
    """信号集中在一个与进程同生命周期的对象上，任务本身不需要是 QObject。"""

    changed = Signal()
    done = Signal(int, object)
    error = Signal(int, str)
    partial = Signal(int, object)


class _Book:
    """任务表：worker 线程写、界面线程读，加锁即可。"""

    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.tasks: dict[int, Task] = {}
        self.order: list[int] = []

    def add(self, task: Task) -> None:
        with self.lock:
            self.tasks[task.id] = task
            self.order.append(task.id)

    def get(self, task_id: int) -> Task | None:
        with self.lock:
            return self.tasks.get(task_id)

    def update(self, task_id: int, **fields: Any) -> None:
        with self.lock:
            task = self.tasks.get(task_id)
            if task is None:
                return
            for key, value in fields.items():
                setattr(task, key, value)

    def snapshot(self, limit: int = 200) -> list[dict]:
        with self.lock:
            rows = [self.tasks[key] for key in self.order[-limit:]]
        out = []
        for task in rows:
            out.append({
                "id": task.id,
                "name": task.name,
                "state": task.state,
                "state_text": STATE_TEXT.get(task.state, task.state),
                "progress": task.progress,
                "message": task.message,
                "error": task.error,
                "serial": task.serial,
                "created": task.created,
                "started": task.started,
                "finished": task.finished,
                "duration": task.duration,
            })
        out.reverse()      # 新的在前
        return out

    def counts(self) -> dict[str, int]:
        with self.lock:
            counts: dict[str, int] = {}
            for task in self.tasks.values():
                counts[task.state] = counts.get(task.state, 0) + 1
            return counts

    def clear_finished(self) -> int:
        keep_states = {"queued", "running"}
        with self.lock:
            drop = [key for key in self.order if self.tasks[key].state not in keep_states]
            for key in drop:
                self.tasks.pop(key, None)
            self.order = [key for key in self.order if key not in set(drop)]
        return len(drop)

    def running_ids(self) -> list[int]:
        with self.lock:
            return [key for key in self.order if self.tasks[key].state == "running"]


_BOOK = _Book()
_DISPATCH = _Dispatcher()
# 对外暴露的信号：任务状态一变就发，任务监控页和侧栏徽标都订阅它
changed = _DISPATCH.changed
_SEQ = itertools.count(1)
_LOCK = threading.Lock()
_CALLBACKS: dict[int, tuple] = {}
_FUTURES: dict[int, Future] = {}
_LOCAL = threading.local()

# 并行通道默认 4 个 worker；串行通道固定 1 个，专门放会写磁盘的任务
_PARALLEL = ThreadPoolExecutor(max_workers=PARALLEL_WORKERS, thread_name_prefix="task-p")
_SERIAL = ThreadPoolExecutor(max_workers=1, thread_name_prefix="task-s")


# ------------------------------------------------------------------ 进度
def report(value: float | None = None, message: str = "") -> None:
    """worker 里上报进度；取不到当前任务时静默忽略，不影响业务。"""
    task_id = getattr(_LOCAL, "id", None)
    if task_id is None:
        return
    fields: dict[str, Any] = {}
    if value is not None:
        fields["progress"] = max(0.0, min(1.0, float(value)))
    if message:
        fields["message"] = str(message)[:120]
    if fields:
        _BOOK.update(task_id, **fields)
        _notify()


def _notify() -> None:
    try:
        _DISPATCH.changed.emit()
    except RuntimeError:
        pass          # 退出过程中信号对象已回收


# ------------------------------------------------------------------ 命名
def _auto_name() -> str:
    """从调用栈取个像样的名字：类名 · 方法名，省得 46 个调用点逐个改。"""
    owner = ""
    func = ""
    frame = sys._getframe(2)
    for _ in range(4):
        if frame is None:
            break
        func = frame.f_code.co_name
        self_obj = frame.f_locals.get("self")
        if self_obj is not None and not owner:
            owner = type(self_obj).__name__
        if func not in ("<lambda>", "worker", "run", "done", "failed", "query",
                        "work", "_emit", "<module>"):
            break
        frame = frame.f_back
    label = (func or "后台任务").replace("_", " ").strip()
    if owner and not owner.startswith("_"):
        return f"{owner} · {label}"
    return label or "后台任务"


def _friendly_owner(owner: str) -> str:
    return {
        "AccountCenterPage": "账号中心",
        "HubPage": "技能与 MCP",
        "SessionsPage": "会话管理",
        "OverviewPage": "总览",
        "MaintenancePage": "备份与清理",
        "SettingsPage": "设置",
        "MainWindow": "主窗口",
        "AddAccountDialog": "添加账号",
    }.get(owner, owner)


def _label(raw: str) -> str:
    if " · " not in raw:
        return raw
    owner, rest = raw.split(" · ", 1)
    return f"{_friendly_owner(owner)} · {rest}"


# ------------------------------------------------------------------ 执行
def _run_task(task_id: int, fn: Callable) -> None:
    _LOCAL.id = task_id
    started = time.time()
    _BOOK.update(task_id, state="running", started=started)
    _notify()
    task = _BOOK.get(task_id)
    name = task.name if task else f"任务#{task_id}"
    queue_ms = max(0.0, (started - task.created) * 1000) if task else 0.0
    logbook.debug(f"任务开始：{name}", module="任务", task=name)
    try:
        result = fn(lambda payload: _DISPATCH.partial.emit(task_id, payload))
    except Exception as exc:
        detail = "".join(traceback.format_exception_only(type(exc), exc)).strip()
        _BOOK.update(task_id, state="failed", finished=time.time(),
                     error=detail, message=detail)
        _notify()
        logbook.error(f"任务失败：{name} — {detail}", module="任务", task=name)
        try:
            _DISPATCH.error.emit(task_id, f"{detail}\n{traceback.format_exc()}")
        except RuntimeError:
            pass
    else:
        _BOOK.update(task_id, state="done", finished=time.time(), progress=1.0)
        _notify()
        logbook.debug(f"任务完成：{name}", module="任务", task=name)
        try:
            _DISPATCH.done.emit(task_id, result)
        except RuntimeError:
            pass
    finally:
        # 性能打点：不管成败都记，排队耗时与执行耗时分开，定位卡顿用
        current = _BOOK.get(task_id)
        state = current.state if current else ""
        logbook.perf(name, (time.time() - started) * 1000,
                     queue_ms=round(queue_ms, 1), state=state,
                     serial=bool(task.serial) if task else False, task=name)
        _LOCAL.id = None


def _on_done(token: int, result: Any) -> None:
    with _LOCK:
        callbacks = _CALLBACKS.pop(token, None)
    if not callbacks or callbacks[0] is None:
        return
    try:
        callbacks[0](result)
    except RuntimeError:
        pass
    except Exception:
        traceback.print_exc()


def _on_error(token: int, message: str) -> None:
    with _LOCK:
        callbacks = _CALLBACKS.pop(token, None)
    if not callbacks or callbacks[1] is None:
        return
    try:
        callbacks[1](message)
    except RuntimeError:
        pass
    except Exception:
        traceback.print_exc()


def _on_partial(token: int, payload: Any) -> None:
    with _LOCK:
        callbacks = _CALLBACKS.get(token)
    if callbacks is None:
        return
    if isinstance(payload, str):
        _BOOK.update(callbacks[3], message=payload[:120])
        _notify()
    if callbacks[2] is None:
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


def submit(
    fn: Callable[[Callable[[Any], None]], Any],
    on_done: Callable[[Any], None] | None = None,
    on_error: Callable[[str], None] | None = None,
    on_partial: Callable[[Any], None] | None = None,
    name: str | None = None,
    serial: bool = False,
) -> int:
    """把耗时动作丢进队列；返回任务 ID，可在任务监控页看到它。

    serial=True 的任务走串行通道：会写账号库 / 登录态的任务用它，
    避免两个写盘任务互相踩；读接口的任务用默认通道，最多 4 个并行。
    """
    token = next(_SEQ)
    task = Task(id=token, name=_label(name or _auto_name()), serial=serial)
    _BOOK.add(task)
    with _LOCK:
        _CALLBACKS[token] = (on_done, on_error, on_partial, token)
    pool = _SERIAL if serial else _PARALLEL
    future = pool.submit(_run_task, token, fn)
    with _LOCK:
        _FUTURES[token] = future
    _notify()
    return token


def parallel_map(items: Iterable, worker: Callable, max_workers: int = 4) -> None:
    """在当前任务里并发跑一组互不依赖的请求（如多账号状态查询）。"""
    items = list(items)
    if not items:
        return
    with ThreadPoolExecutor(max_workers=max_workers) as pool:
        list(pool.map(worker, items))


# ------------------------------------------------------------------ 查询 / 管理
def snapshot() -> list[dict]:
    return _BOOK.snapshot()


def counts() -> dict[str, int]:
    return _BOOK.counts()


def active_count() -> int:
    counts_now = _BOOK.counts()
    return counts_now.get("queued", 0) + counts_now.get("running", 0)


def clear_finished() -> int:
    removed = _BOOK.clear_finished()
    with _LOCK:
        for key in [k for k, f in _FUTURES.items() if f.done()]:
            _FUTURES.pop(key, None)
    if removed:
        _notify()
    return removed


def cancel(task_id: int) -> bool:
    """取消还在排队的任务；已经开始跑的取消不了（正在写盘，打断更危险）。"""
    with _LOCK:
        future = _FUTURES.get(task_id)
    if future is not None and future.cancel():
        _BOOK.update(task_id, state="cancelled", finished=time.time(),
                     message="已在排队中取消")
        _notify()
        return True
    task = _BOOK.get(task_id)
    if task is not None and task.state == "queued":
        _BOOK.update(task_id, state="cancelled", finished=time.time(),
                     message="已取消")
        _notify()
        return True
    return False
