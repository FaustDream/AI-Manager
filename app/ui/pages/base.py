# -*- coding: utf-8 -*-
"""页面基类：统一的页头、留白与常用交互出口。"""
from __future__ import annotations

from PySide6.QtWidgets import QVBoxLayout, QWidget

from ...core import bus
from ..widgets import PageHeader


class BasePage(QWidget):
    key = ""
    title = ""
    subtitle = ""

    def __init__(self, win):
        super().__init__()
        self.win = win
        self.cfg = win.cfg
        self.busy = False

        outer = QVBoxLayout(self)
        outer.setContentsMargins(24, 18, 24, 8)
        outer.setSpacing(14)

        self.header = PageHeader(self.title, self.subtitle)
        outer.addWidget(self.header)

        self.body = QVBoxLayout()
        self.body.setSpacing(12)
        outer.addLayout(self.body, 1)

    # ------------------------------------------------------------ 交互出口
    def log(self, message: str, level: str = "info") -> None:
        """一律走信号：界面线程里是直连（同步），后台线程里自动排队。

        不要在后台任务里直接调 win.log —— 那是在非 GUI 线程碰控件，
        轻则卡顿重则崩溃；PySide 的 QThread 包装对象也不能用 is 比较来判断线程。
        """
        bus.BUS.log.emit(str(message), level)

    def notify(self, message: str, level: str = "info") -> None:
        self.win.notify(message, level)

    def confirm(self, title: str, text: str) -> bool:
        return self.win.confirm(title, text)

    def info(self, title: str, text: str) -> None:
        self.win.info(title, text)

    def warn(self, title: str, text: str) -> None:
        self.win.warn(title, text)

    def error(self, title: str, text: str) -> None:
        self.win.error(title, text)

    # ------------------------------------------------------------ 生命周期
    def on_show(self) -> None:
        """页面被切到前台时调用。"""

    def on_hide(self) -> None:
        """页面被切走时调用：把还没落盘的状态写出去。"""

    def on_data_changed(self, scope: str) -> None:
        """其他页面改动了共享数据（如新增账号）时调用。"""

    def on_credits(self, snapshots: dict) -> None:
        """定时轮询拿到一批积分快照 {iid: CreditSnapshot} 时调用。"""

    def _set_busy(self, busy: bool, text: str = "") -> None:
        self.busy = busy
        self.win.set_busy(busy, text, self.key)
