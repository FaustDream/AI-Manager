# -*- coding: utf-8 -*-
"""任务监控：把所有后台任务的排队 / 执行 / 完成情况摊开给用户看。

以前只有右下角一个转圈，用户不知道哪个操作在跑、跑到哪了、还要多久。
这里把任务表整个亮出来：状态、进度、耗时、说明都在表格里，实时刷新。
"""
from __future__ import annotations

import time
from datetime import datetime

from PySide6.QtCore import Qt, QTimer
from PySide6.QtWidgets import QHBoxLayout, QSizePolicy, QVBoxLayout, QWidget

from ...core import tasks
from ..models import Column
from ..theme import PALETTE
from ..widgets import Card, DataTable, EmptyState, label, make_button
from .base import BasePage

TASK_COLUMNS = [
    Column("name", "任务", 210, stretch=True),
    Column("state", "状态", 80),
    Column("progress", "进度", 88),
    Column("duration", "耗时", 76),
    Column("started", "开始时间", 104),
    Column("message", "说明", 250, stretch=True),
]

STATE_TONE = {
    "queued": ("排队中", "muted"),
    "running": ("执行中", "accent"),
    "done": ("已完成", "ok"),
    "failed": ("失败", "danger"),
    "cancelled": ("已取消", "warn"),
}


def _clock(ts: float | None) -> str:
    if not ts:
        return "—"
    return datetime.fromtimestamp(ts).strftime("%H:%M:%S")


def _duration(seconds: float, state: str) -> str:
    if state == "queued":
        return "—"
    if seconds < 1:
        return f"{seconds * 1000:.0f} 毫秒"
    if seconds < 60:
        return f"{seconds:.1f} 秒"
    return f"{int(seconds // 60)} 分 {int(seconds % 60)} 秒"


def _progress_text(progress, state: str) -> str:
    if state == "queued":
        return "排队中"
    if state == "done":
        return "100%"
    if state == "failed":
        return "失败"
    if state == "cancelled":
        return "已取消"
    if progress is None:
        return "进行中"
    return f"{float(progress) * 100:.0f}%"


class TaskPage(BasePage):
    key = "tasks"
    title = "任务"
    subtitle = "后台任务的排队、执行与完成情况，实时更新"

    def __init__(self, win):
        super().__init__(win)
        self._debounce = QTimer(self)
        self._debounce.setSingleShot(True)
        self._debounce.setInterval(260)
        self._debounce.timeout.connect(self._refresh)
        tasks.changed.connect(self._on_change)
        self._build()
        self._refresh()

    def _build(self) -> None:
        self.header.hide()

        toolbar = Card(padding=12, spacing=8)
        row = QHBoxLayout()
        row.setSpacing(6)
        row.addWidget(self._button("刷新", self._refresh))
        row.addWidget(self._button("取消选中", self._cancel_selected))
        row.addWidget(self._button("清除已结束", self._clear_finished))
        self.summary = label("", "SectionHint")
        self.summary.setSizePolicy(QSizePolicy.Policy.Ignored,
                                   QSizePolicy.Policy.Preferred)
        self.summary.setAlignment(Qt.AlignmentFlag.AlignRight
                                 | Qt.AlignmentFlag.AlignVCenter)
        row.addWidget(self.summary, 1)
        toolbar.body.addLayout(row)
        self.body.addWidget(toolbar)

        card = Card(padding=0, spacing=0)
        self.table = DataTable(TASK_COLUMNS, key="id", multi=True, parent=self)
        card.body.addWidget(self.table, 1)
        self.empty = EmptyState(
            "还没有后台任务",
            "扫描登录态、签到、同步技能、查询额度等操作都会出现在这里，可看到排队与执行状态")
        card.body.addWidget(self.empty, 1)
        self.body.addWidget(card, 1)

    def _button(self, text: str, handler):
        button = make_button(text, "ghost", None, self)
        button.clicked.connect(handler)
        return button

    # ------------------------------------------------------------ 实时刷新
    def _on_change(self) -> None:
        # 任务更新可能很密（多账号逐个回传），合并成一次重绘
        self._debounce.start()

    def on_show(self) -> None:
        self._refresh()

    def _refresh(self) -> None:
        rows = []
        running = queued = 0
        for item in tasks.snapshot():
            state = item["state"]
            if state == "running":
                running += 1
            elif state == "queued":
                queued += 1
            text, tone = STATE_TONE.get(state, (state, "muted"))
            rows.append({
                "id": item["id"],
                "name": item["name"],
                "state": text,
                "progress": _progress_text(item["progress"], state),
                "duration": _duration(item["duration"], state),
                "started": _clock(item["started"] or item["created"]),
                "message": item.get("message") or item.get("error") or "—",
                "_fg_map": {
                    "state": PALETTE[tone],
                    "progress": PALETTE["accent"] if state == "running" else PALETTE["text_2"],
                },
                "_tip_map": {
                    "name": item.get("name") or "",
                    "state": f"排队于 {_clock(item['created'])}",
                    "progress": ("已排队，等待空闲的执行槽" if state == "queued"
                                 else f"{item['progress'] * 100:.0f}%"
                                 if item.get("progress") is not None else "没有进度上报"),
                    "message": item.get("error") or item.get("message") or "",
                },
            })
        self.table.set_rows(rows)
        self.empty.setVisible(not rows)
        self.table.setVisible(bool(rows))
        counts = tasks.counts()
        done = counts.get("done", 0)
        failed = counts.get("failed", 0)
        self.summary.setText(
            f"执行中 {running} · 排队 {queued} · 已完成 {done} · 失败 {failed}")

    # ------------------------------------------------------------ 操作
    def _selected_ids(self) -> list[int]:
        return [int(row["id"]) for row in self.table.selected_rows()]

    def _cancel_selected(self) -> None:
        ids = self._selected_ids()
        if not ids:
            self.info("取消任务", "请先在列表里选中任务。")
            return
        cancelled = sum(1 for key in ids if tasks.cancel(key))
        self._refresh()
        if cancelled:
            self.notify(f"已取消 {cancelled} 个排队中的任务", "ok")
        else:
            self.notify("选中的任务都已开始执行，无法取消", "warn")

    def _clear_finished(self) -> None:
        removed = tasks.clear_finished()
        self._refresh()
        self.log(f"任务记录已清理：{removed} 条", "dim")
