# -*- coding: utf-8 -*-
"""日志：查询、筛选与导出运行日志，定位问题时从这里开始。

app.log 是人读文本、app.jsonl 是机读结构化，本页读的是结构化那边
（老日志没有结构化记录时自动回退解析文本行），因此可以按级别 /
类型 / 关键词过滤，也可以把筛选结果导出成 txt / jsonl / csv。
"""
from __future__ import annotations

from datetime import datetime

from PySide6.QtCore import Qt, QTimer
from PySide6.QtGui import QGuiApplication
from PySide6.QtWidgets import (QComboBox, QFileDialog, QHBoxLayout, QLineEdit,
                               QSizePolicy, QVBoxLayout, QWidget)

from ...core import logbook, paths, winproc
from ..models import Column
from ..theme import PALETTE
from ..widgets import Card, DataTable, EmptyState, label, make_button
from .base import BasePage

LOG_COLUMNS = [
    Column("time", "时间", 150),
    Column("level", "级别", 62),
    Column("kind", "类型", 62),
    Column("module", "模块", 112),
    Column("message", "内容", 300, stretch=True),
    Column("duration", "耗时", 78),
]

LEVEL_TONE = {"debug": "muted", "info": "text_2", "warn": "warn", "error": "danger"}
LEVEL_TEXT = logbook.LEVEL_TEXT
KIND_TEXT = {"log": "日志", "event": "事件", "perf": "性能"}

SHOW_LIMIT = 500


def _ms_text(ms: float) -> str:
    if ms < 1000:
        return f"{ms:.0f} 毫秒"
    if ms < 60_000:
        return f"{ms / 1000:.1f} 秒"
    return f"{int(ms // 60_000)} 分 {int((ms % 60_000) / 1000)} 秒"


class LogPage(BasePage):
    key = "logs"
    title = "日志"
    subtitle = "任务过程、状态变化、错误与性能打点的记录，可筛选与导出"

    def __init__(self, win):
        super().__init__(win)
        self._debounce = QTimer(self)
        self._debounce.setSingleShot(True)
        self._debounce.setInterval(300)
        self._debounce.timeout.connect(self._refresh)
        self._records: list[dict] = []
        self._build()
        self._refresh()

    # ------------------------------------------------------------ 构建
    def _build(self) -> None:
        toolbar = Card(padding=12, spacing=8)
        row = QHBoxLayout()
        row.setSpacing(6)

        row.addWidget(label("级别", "StatLabel"))
        self.level_filter = QComboBox()
        self.level_filter.setFixedWidth(92)
        self.level_filter.addItem("全部", "")
        for key in ("debug", "info", "warn", "error"):
            self.level_filter.addItem(LEVEL_TEXT[key], key)
        self.level_filter.currentIndexChanged.connect(self._refresh)
        row.addWidget(self.level_filter)

        row.addWidget(label("类型", "StatLabel"))
        self.kind_filter = QComboBox()
        self.kind_filter.setFixedWidth(92)
        self.kind_filter.addItem("全部", "")
        self.kind_filter.addItem("日志", "log")
        self.kind_filter.addItem("事件", "event")
        self.kind_filter.addItem("性能", "perf")
        self.kind_filter.currentIndexChanged.connect(self._refresh)
        row.addWidget(self.kind_filter)

        self.keyword = QLineEdit()
        self.keyword.setFixedWidth(200)
        self.keyword.setPlaceholderText("关键词，输入即筛")
        self.keyword.setClearButtonEnabled(True)
        self.keyword.textChanged.connect(lambda _t: self._debounce.start())
        row.addWidget(self.keyword)

        row.addWidget(self._button("导出", self._export))
        row.addWidget(self._button("打开目录", self._open_dir))
        row.addWidget(self._button("清理旧日志", self._clear_rotated))

        self.summary = label("", "SectionHint")
        self.summary.setSizePolicy(QSizePolicy.Policy.Ignored,
                                   QSizePolicy.Policy.Preferred)
        self.summary.setAlignment(Qt.AlignmentFlag.AlignRight
                                 | Qt.AlignmentFlag.AlignVCenter)
        row.addWidget(self.summary, 1)
        toolbar.body.addLayout(row)
        self.body.addWidget(toolbar)

        card = Card(padding=0, spacing=0)
        self.table = DataTable(LOG_COLUMNS, key="id", parent=self)
        self.table.doubleClicked.connect(self._copy_row)
        card.body.addWidget(self.table, 1)
        self.empty = EmptyState(
            "没有匹配的日志",
            "放宽级别或类型筛选，或清空关键词；老日志若还没生成结构化记录，"
            "会自动回退解析 app.log 文本")
        card.body.addWidget(self.empty, 1)
        self.body.addWidget(card, 1)

    def _button(self, text: str, handler):
        button = make_button(text, "ghost", None, self)
        button.clicked.connect(handler)
        return button

    # ------------------------------------------------------------ 查询
    def on_show(self) -> None:
        self._refresh()

    def _refresh(self) -> None:
        records = logbook.query(
            level=str(self.level_filter.currentData() or ""),
            kind=str(self.kind_filter.currentData() or ""),
            keyword=self.keyword.text(),
            limit=SHOW_LIMIT,
        )
        self._records = records
        rows = []
        for index, record in enumerate(records):
            lv = str(record.get("lv") or "info")
            kind = str(record.get("kind") or "log")
            duration = record.get("dur")
            rows.append({
                "id": f"{record.get('ts', '')}|{index}",
                "time": str(record.get("ts") or "")[:19],
                "level": LEVEL_TEXT.get(lv, lv),
                "kind": KIND_TEXT.get(kind, kind),
                "module": record.get("mod") or "—",
                "message": str(record.get("msg") or ""),
                "duration": _ms_text(float(duration)) if duration is not None else "—",
                "_fg_map": {
                    "level": PALETTE.get(LEVEL_TONE.get(lv, "text_2"), PALETTE["text_2"]),
                    "duration": PALETTE["accent"] if duration is not None else PALETTE["muted"],
                },
                "_tip_map": {
                    "time": str(record.get("ts") or ""),
                    "level": {"debug": "过程细节，排查时才需要",
                              "info": "关键步骤与状态变化",
                              "warn": "能继续但不理想",
                              "error": "失败或异常"}.get(lv, ""),
                    "kind": {"log": "普通日志行", "event": "状态变化事件",
                             "perf": "性能打点：耗时、排队与内存"}.get(kind, ""),
                    "message": self._detail_tip(record),
                    "duration": ("执行耗时" if duration is not None else ""),
                },
            })
        self.table.set_rows(rows)
        self.empty.setVisible(not rows)
        self.table.setVisible(bool(rows))

        counts = logbook.stats(records)
        self.summary.setText(
            f"共 {counts['total']} 条 · 调试 {counts['debug']} · 信息 {counts['info']}"
            f" · 警告 {counts['warn']} · 错误 {counts['error']}"
            f" · 事件 {counts['event']} · 性能 {counts['perf']}"
            + (f"（只显示最新 {SHOW_LIMIT} 条）" if counts["total"] >= SHOW_LIMIT else ""))

    @staticmethod
    def _detail_tip(record: dict) -> str:
        parts = [str(record.get("msg") or "")]
        for key, value in record.items():
            if key in ("ts", "lv", "kind", "mod", "msg"):
                continue
            if key == "dur":
                parts.append(f"耗时 {_ms_text(float(value))}")
            elif key == "mem":
                parts.append(f"内存 {value}MB")
            elif key == "queue_ms":
                parts.append(f"排队 {float(value):.0f}ms")
            else:
                parts.append(f"{key} = {value}")
        return "\n".join(parts)

    # ------------------------------------------------------------ 操作
    def _copy_row(self, *_args) -> None:
        row = self.table.current_row()
        if not row:
            return
        QGuiApplication.clipboard().setText(str(row.get("message") or ""))
        self.notify("已复制该行内容")

    def _export(self) -> None:
        if not self._records:
            self.info("导出日志", "当前筛选结果为空，先查到内容再导出。")
            return
        stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
        default = str(paths.DATA_DIR / "exports" / f"AI-Manager-日志-{stamp}.txt")
        path, chosen = QFileDialog.getSaveFileName(
            self, "导出日志", default,
            "文本文件 (*.txt);;JSONL (*.jsonl);;CSV (*.csv)")
        if not path:
            return
        fmt = "jsonl" if chosen.startswith("JSONL") else "csv" if chosen.startswith("CSV") else "txt"
        # 让后缀和格式一致，省得用户选了 csv 还存成 txt
        expected = {"txt": ".txt", "jsonl": ".jsonl", "csv": ".csv"}[fmt]
        if not path.lower().endswith(expected):
            path += expected
        count = logbook.export(self._records, path, fmt)
        self.log(f"已导出 {count} 条日志到 {path}", "ok")
        self.notify(f"已导出 {count} 条日志")

    def _open_dir(self) -> None:
        logbook.flush()
        paths.LOG_DIR.mkdir(parents=True, exist_ok=True)
        winproc.open_path(paths.LOG_DIR)

    def _clear_rotated(self) -> None:
        if not self.confirm("清理旧日志",
                            "删除轮转出来的历史日志文件（app.log.1…3 / app.jsonl.1…3），"
                            "正在写的主日志不受影响。继续？"):
            return
        freed = logbook.clear_rotated()
        if freed:
            self.log(f"已清理历史日志，释放 {freed / 1024 / 1024:.1f}MB", "ok")
            self.notify(f"已清理历史日志，释放 {freed / 1024 / 1024:.1f}MB")
        else:
            self.notify("没有可清理的历史日志")
