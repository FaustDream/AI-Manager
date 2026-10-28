# -*- coding: utf-8 -*-
"""全局事件总线：后台线程只发信号，界面线程负责渲染。

Qt 的跨线程信号默认是 QueuedConnection，所以后台线程 emit 是安全的；
所有耗时段都通过 bus.log 汇报进度，界面不需要知道线程细节。
"""
from __future__ import annotations

from PySide6.QtCore import QObject, Signal


class Bus(QObject):
    log = Signal(str, str)            # 文本, 级别(info/ok/dim/error)
    toast = Signal(str, str)          # 文本, 级别
    busy = Signal(bool, str)          # 是否忙, 状态文案
    data_changed = Signal(str)        # 某个数据域发生变化，页面据此刷新
    credits = Signal(object)          # 定时轮询的积分快照 {iid: CreditSnapshot}
    tray_message = Signal(str, str)   # 托盘气泡


BUS = Bus()

LEVELS = ("info", "ok", "dim", "warn", "error")


def log(message: str, level: str = "info") -> None:
    BUS.log.emit(str(message), level if level in LEVELS else "info")


def toast(message: str, level: str = "info") -> None:
    BUS.toast.emit(str(message), level)
