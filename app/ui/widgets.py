# -*- coding: utf-8 -*-
"""通用界面组件：卡片、统计块、徽标、表格、日志、气泡提示。"""
from __future__ import annotations

import html

from PySide6.QtCore import QEvent, QPoint, QRectF, QTimer, Qt, Signal
from PySide6.QtGui import QColor, QCursor, QFont, QPainter, QPen
from PySide6.QtWidgets import (QAbstractItemView, QFrame, QHBoxLayout, QHeaderView,
                               QLabel, QPlainTextEdit, QPushButton, QSizePolicy,
                               QTableView, QVBoxLayout, QWidget)

from . import icons
from .models import Column, RowModel
from .theme import PALETTE, repolish


# --------------------------------------------------------------------- 小工具
def hline() -> QFrame:
    line = QFrame()
    line.setObjectName("Divider")
    line.setFixedHeight(1)
    return line


def label(text: str, name: str = "", parent=None) -> QLabel:
    widget = QLabel(text, parent)
    if name:
        widget.setObjectName(name)
    return widget


def make_button(text: str, kind: str = "ghost", icon_name: str | None = None,
                parent=None) -> QPushButton:
    button = QPushButton(text, parent)
    button.setObjectName({"primary": "Primary", "ghost": "Ghost",
                          "danger": "Danger", "danger-ghost": "DangerGhost",
                          "link": "Link"}.get(kind, "Ghost"))
    button.setCursor(QCursor(Qt.CursorShape.PointingHandCursor))
    if icon_name:
        color = "#FFFFFF" if kind in ("primary", "danger") else PALETTE["text_2"]
        button.setIcon(icons.icon(icon_name, color, 16))
    return button


class Spinner(QWidget):
    """经典旋转圆环，全局共用一个即可。

    只在可见时跑定时器：showEvent 启动、hideEvent 停止，避免后台空转。
    """

    def __init__(self, parent=None, size: int = 18, color: str | None = None):
        super().__init__(parent)
        self._angle = 0
        self._color = QColor(color or PALETTE["accent"])
        self._timer = QTimer(self)
        self._timer.setInterval(16)
        self._timer.timeout.connect(self._tick)
        self.setFixedSize(size, size)
        policy = QSizePolicy(QSizePolicy.Policy.Fixed, QSizePolicy.Policy.Fixed)
        policy.setRetainSizeWhenHidden(True)
        self.setSizePolicy(policy)
        self.setAttribute(Qt.WidgetAttribute.WA_TransparentForMouseEvents, True)

    def _tick(self) -> None:
        self._angle = (self._angle + 6) % 360
        self.update()

    def start(self) -> None:
        if not self._timer.isActive():
            self._timer.start()

    def stop(self) -> None:
        self._timer.stop()

    def showEvent(self, event) -> None:  # noqa: N802 - Qt 接口
        self.start()
        super().showEvent(event)

    def hideEvent(self, event) -> None:  # noqa: N802 - Qt 接口
        self.stop()
        super().hideEvent(event)

    def paintEvent(self, event) -> None:  # noqa: N802 - Qt 接口
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing, True)
        rect = QRectF(2.0, 2.0, self.width() - 4.0, self.height() - 4.0)

        track = QPen(QColor(PALETTE["border_strong"]), 2.0)
        track.setCapStyle(Qt.PenCapStyle.RoundCap)
        painter.setPen(track)
        painter.drawEllipse(rect)

        arc = QPen(self._color, 2.2)
        arc.setCapStyle(Qt.PenCapStyle.RoundCap)
        painter.setPen(arc)
        # Qt 的角度单位是 1/16 度；负角度顺时针旋转
        painter.drawArc(rect, -self._angle * 16, 270 * 16)
        painter.end()


class BusyIndicator(QFrame):
    """转圈 + 一句文案，固定在侧栏底部，任何页面加载数据时都亮。

    放在侧栏而不是页面里：页面顶部被工具行占满，浮层会盖住按钮；侧栏常驻，
    每个页面看到的都是同一处反馈。延迟 120ms 才显示，几毫秒就结束的任务不会闪一下。
    """

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setObjectName("BusyPill")
        row = QHBoxLayout(self)
        row.setContentsMargins(8, 5, 8, 5)
        row.setSpacing(8)
        self.spinner = Spinner(self, 18)
        row.addWidget(self.spinner)
        self.text = label("正在加载…", "BusyText")
        # 文案不参与宽度计算：长账号名不能把侧栏撑开
        self.text.setSizePolicy(QSizePolicy.Policy.Ignored, QSizePolicy.Policy.Fixed)
        row.addWidget(self.text, 1)

        self._delay = QTimer(self)
        self._delay.setSingleShot(True)
        self._delay.setInterval(120)
        self._delay.timeout.connect(self.show)
        self._full = "正在加载…"
        self.hide()

    def start(self, text: str = "") -> None:
        self._full = text or "正在加载…"
        self.text.setToolTip(self._full)
        self._elide()
        if not self.isVisible() and not self._delay.isActive():
            self._delay.start()

    def stop(self) -> None:
        self._delay.stop()
        self.hide()

    def _elide(self) -> None:
        # 侧栏宽度固定，超出的部分截断成省略号，不要硬裁半个字
        avail = max(40, self.width() - self.spinner.width() - 28)
        self.text.setText(
            self.text.fontMetrics().elidedText(self._full, Qt.TextElideMode.ElideRight, avail))

    def resizeEvent(self, event) -> None:  # noqa: N802 - Qt 接口
        super().resizeEvent(event)
        self._elide()

    def showEvent(self, event) -> None:  # noqa: N802 - Qt 接口
        # 隐藏期间布局不给宽度，_elide 会按旧宽度截断；露面前按真实宽度重算一次
        super().showEvent(event)
        self._elide()


class Badge(QLabel):
    def __init__(self, text: str = "", tone: str = "muted", parent=None):
        super().__init__(text, parent)
        self.setObjectName("Badge")
        self.setProperty("tone", tone)
        self.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.setSizePolicy(QSizePolicy.Policy.Maximum, QSizePolicy.Policy.Fixed)

    def set(self, text: str, tone: str = "muted") -> None:
        self.setText(text)
        self.setProperty("tone", tone)
        repolish(self)

    def set_tone(self, tone: str) -> None:
        self.setProperty("tone", tone)
        repolish(self)


# --------------------------------------------------------------------- 卡片
class Card(QFrame):
    def __init__(self, parent=None, padding: int = 16, spacing: int = 12,
                 flat: bool = False):
        super().__init__(parent)
        self.setObjectName("CardFlat" if flat else "Card")
        self.body = QVBoxLayout(self)
        self.body.setContentsMargins(padding, padding, padding, padding)
        self.body.setSpacing(spacing)

    def add_title(self, title: str, hint: str = "", right: QWidget | None = None):
        row = QHBoxLayout()
        row.setSpacing(8)
        row.addWidget(label(title, "SectionTitle"))
        if hint:
            row.addWidget(label(hint, "SectionHint"))
        row.addStretch(1)
        if right is not None:
            row.addWidget(right)
        self.body.addLayout(row)
        return row


class StatCard(QFrame):
    """概览用的数字卡：图标 + 数字 + 说明。"""

    def __init__(self, icon_name: str, label_text: str, value: str = "—",
                 hint: str = "", tone: str = "accent", parent=None):
        super().__init__(parent)
        self.setObjectName("Card")
        outer = QVBoxLayout(self)
        outer.setContentsMargins(16, 14, 16, 14)
        outer.setSpacing(8)

        top = QHBoxLayout()
        top.setSpacing(8)
        chip = QLabel()
        chip.setFixedSize(28, 28)
        chip.setAlignment(Qt.AlignmentFlag.AlignCenter)
        chip.setStyleSheet(
            f"background: {PALETTE['accent_soft']}; border-radius: 8px;")
        chip.setPixmap(icons.pixmap(icon_name, PALETTE.get(tone, PALETTE["accent"]), 16))
        top.addWidget(chip)
        top.addWidget(label(label_text, "StatLabel"))
        top.addStretch(1)
        outer.addLayout(top)

        self.value = label(value, "StatValue")
        outer.addWidget(self.value)

        self.hint = label(hint, "StatHint")
        self.hint.setWordWrap(True)
        # 空说明整行收起：不传文案的卡片只留图标 + 标题 + 数字
        self.hint.setVisible(bool(hint))
        outer.addWidget(self.hint)

    def set_value(self, value: str, hint: str | None = None) -> None:
        self.value.setText(value)
        if hint is not None:
            self.hint.setText(hint)
            self.hint.setVisible(bool(hint))


class TrendBars(QWidget):
    """近 N 天的走势柱：只画柱子和首尾日期，看趋势够用，也不用引图表库。"""

    def __init__(self, parent=None, height: int = 104, tone: str = "accent"):
        super().__init__(parent)
        self._points: list[tuple[str, float]] = []
        self._tone = tone
        self.setFixedHeight(height)
        self.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)

    def set_points(self, points) -> None:
        self._points = [(str(day), float(value or 0)) for day, value in points]
        self.update()

    def paintEvent(self, event) -> None:  # noqa: N802 - Qt 接口
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing, True)
        area = self.rect().adjusted(0, 6, 0, -16)
        points = self._points
        if not points or not any(value for _day, value in points):
            painter.setPen(QColor(PALETTE["muted"]))
            painter.drawText(self.rect(), Qt.AlignmentFlag.AlignCenter,
                             "还没有历史数据")
            painter.end()
            return

        peak = max(value for _day, value in points) or 1.0
        count = len(points)
        gap = 3.0
        width = max(3.0, (area.width() - gap * (count - 1)) / count)
        color = QColor(PALETTE.get(self._tone, PALETTE["accent"]))
        baseline = area.bottom()
        for position, (_day, value) in enumerate(points):
            height = max(2.0, area.height() * value / peak)
            x = area.left() + position * (width + gap)
            rect = QRectF(x, baseline - height, width, height)
            last = position == count - 1
            color.setAlpha(255 if last else 110)
            painter.setBrush(color)
            painter.setPen(Qt.PenStyle.NoPen)
            painter.drawRoundedRect(rect, 3.0, 3.0)

        painter.setPen(QColor(PALETTE["muted"]))
        font = QFont("Microsoft YaHei UI", 8)
        painter.setFont(font)
        painter.drawText(QRectF(area.left(), baseline + 2, area.width(), 14),
                         Qt.AlignmentFlag.AlignLeft | Qt.AlignmentFlag.AlignVCenter,
                         points[0][0][5:])
        painter.drawText(QRectF(area.left(), baseline + 2, area.width(), 14),
                         Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter,
                         points[-1][0][5:])
        painter.end()


class PageHeader(QWidget):
    def __init__(self, title: str, subtitle: str = "", parent=None):
        super().__init__(parent)
        layout = QHBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(12)

        text_box = QVBoxLayout()
        text_box.setSpacing(3)
        self.title = label(title, "PageTitle")
        text_box.addWidget(self.title)
        self.subtitle = label(subtitle, "PageSub")
        self.subtitle.setVisible(bool(subtitle))
        text_box.addWidget(self.subtitle)
        layout.addLayout(text_box)
        layout.addStretch(1)

        self.actions = QHBoxLayout()
        self.actions.setSpacing(8)
        layout.addLayout(self.actions)

    def add_action(self, widget: QWidget) -> QWidget:
        self.actions.addWidget(widget)
        return widget

    def set_subtitle(self, text: str) -> None:
        self.subtitle.setText(text)
        self.subtitle.setVisible(bool(text))


class EmptyState(QWidget):
    def __init__(self, text: str, sub: str = "", parent=None):
        super().__init__(parent)
        layout = QVBoxLayout(self)
        layout.setAlignment(Qt.AlignmentFlag.AlignCenter)
        layout.setSpacing(6)
        icon = QLabel()
        icon.setPixmap(icons.pixmap("archive", PALETTE["muted"], 28))
        icon.setAlignment(Qt.AlignmentFlag.AlignCenter)
        layout.addWidget(icon)
        title = label(text, "SectionHint")
        title.setAlignment(Qt.AlignmentFlag.AlignCenter)
        layout.addWidget(title)
        if sub:
            hint = label(sub, "KeyLabel")
            hint.setAlignment(Qt.AlignmentFlag.AlignCenter)
            layout.addWidget(hint)


# --------------------------------------------------------------------- 表格
class DataTable(QTableView):
    # 行被拖到新位置：(源行, 移除源行后的插入位置)。模型 move_row 由页面调用
    row_moved = Signal(int, int)

    def __init__(self, columns: list[Column], key: str = "id", multi: bool = True,
                 row_height: int = 36, reorder: bool = False, parent=None):
        super().__init__(parent)
        self.setObjectName("DataTable")
        self.model = RowModel(columns, key=key)
        self.setModel(self.model)
        self.columns = columns
        # 只有明确支持排序的表才开拖拽：其余表拖了也没人接收，容易误以为卡住
        self._reorder = bool(reorder)

        self.verticalHeader().setVisible(False)
        self.verticalHeader().setDefaultSectionSize(row_height)
        self.setShowGrid(False)
        self.setAlternatingRowColors(False)
        self.setWordWrap(False)
        self.setEditTriggers(QAbstractItemView.EditTrigger.NoEditTriggers)
        self.setSelectionBehavior(QAbstractItemView.SelectionBehavior.SelectRows)
        self.setSelectionMode(
            QAbstractItemView.SelectionMode.ExtendedSelection if multi
            else QAbstractItemView.SelectionMode.SingleSelection)
        self.setHorizontalScrollMode(QAbstractItemView.ScrollMode.ScrollPerPixel)
        self.setVerticalScrollMode(QAbstractItemView.ScrollMode.ScrollPerPixel)
        self.setFocusPolicy(Qt.FocusPolicy.StrongFocus)

        header = self.horizontalHeader()
        header.setHighlightSections(False)
        header.setDefaultAlignment(Qt.AlignmentFlag.AlignLeft | Qt.AlignmentFlag.AlignVCenter)
        header.setFixedHeight(34)
        header.setStretchLastSection(False)
        for position, column in enumerate(columns):
            self.setColumnWidth(position, column.width)
            if column.fixed:
                mode = QHeaderView.ResizeMode.Fixed
            elif column.stretch:
                mode = QHeaderView.ResizeMode.Stretch
            else:
                mode = QHeaderView.ResizeMode.Interactive
            header.setSectionResizeMode(position, mode)

        # 行拖动排序：dropEvent 只算落点并发信号，真正的重排由页面调 model.move_row
        if self._reorder:
            self.setDragEnabled(True)
            self.setAcceptDrops(True)
            self.setDropIndicatorShown(True)
            self.setDragDropOverwriteMode(False)
            self.setDragDropMode(QAbstractItemView.DragDropMode.InternalMove)
            self.setDefaultDropAction(Qt.DropAction.MoveAction)
            self.setToolTip("按住任意一行上下拖动可调整顺序，顺序会自动保存")

    # ------------------------------------------------------------------ 布局
    def column_widths(self) -> list[int]:
        header = self.horizontalHeader()
        return [header.sectionSize(i) for i in range(len(self.columns))]

    def apply_column_widths(self, widths) -> None:
        """恢复上次的列宽；自适应列跳过，否则会把「铺满」的布局改坏。"""
        if not widths:
            return
        header = self.horizontalHeader()
        for index, width in enumerate(widths[:len(self.columns)]):
            if self.columns[index].stretch or self.columns[index].fixed:
                continue
            try:
                value = int(width)
            except (TypeError, ValueError):
                continue
            if value >= 24:
                header.resizeSection(index, value)

    def dropEvent(self, event) -> None:  # noqa: N802 - Qt 接口
        """只计算落点，真正的重排交给页面（页面调 model.move_row）。

        落在某行上半 → 插到该行之前；下半 → 插到该行之后；落在空白 → 放最后。
        发出去的是「移除源行后的插入位置」，页面直接 insert/pop 即可。
        """
        if not self._reorder:
            event.ignore()
            return
        source = self.currentIndex().row()
        if source < 0:
            event.ignore()
            return
        point = event.position().toPoint()
        index = self.indexAt(point)
        if index.isValid():
            target = index.row()
            rect = self.visualRect(index)
            if point.y() > rect.center().y():
                target += 1
        else:
            target = self.model.rowCount()
        if target > source:
            target -= 1                     # 源行先被摘掉，后面的落点要前移一格
        if target < 0 or target >= self.model.rowCount() or target == source:
            event.ignore()
            return
        self.row_moved.emit(source, target)
        event.accept()

    # ------------------------------------------------------------------ 便捷
    def set_rows(self, rows: list[dict]) -> None:
        self.model.set_rows(rows)

    def selected_rows(self) -> list[dict]:
        model = self.selectionModel()
        if model is None:
            return []
        return [self.model.row_dict(index.row())
                for index in sorted(model.selectedRows(), key=lambda i: i.row())
                if self.model.row_dict(index.row()) is not None]

    def current_row(self) -> dict | None:
        rows = self.selected_rows()
        return rows[0] if rows else None

    def select_key(self, key) -> None:
        position = self.model.index_of(key)
        if position >= 0:
            index = self.model.index(position, 0)
            self.selectionModel().select(
                index, self.selectionModel().SelectionFlag.ClearAndSelect
                | self.selectionModel().SelectionFlag.Rows)
            self.scrollTo(index)


# --------------------------------------------------------------------- 日志
class LogView(QPlainTextEdit):
    """运行日志：按级别着色，最多保留 2000 行。"""

    COLORS = {
        "info": PALETTE["text_2"],
        "ok": PALETTE["ok"],
        "error": PALETTE["danger"],
        "dim": PALETTE["muted"],
        "warn": PALETTE["warn"],
    }

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setObjectName("Log")
        self.setReadOnly(True)
        self.setMaximumBlockCount(2000)
        self.setPlaceholderText("运行日志会显示在这里…")

    def append(self, message: str, level: str = "info") -> None:
        color = self.COLORS.get(level, self.COLORS["info"])
        text = html.escape(str(message)).replace("  ", "&nbsp;&nbsp;")
        self.appendHtml(f'<span style="color:{color}">{text}</span>')
        bar = self.verticalScrollBar()
        bar.setValue(bar.maximum())


class Toast(QLabel):
    """窗口右下角的轻提示，2.6 秒自动消失。"""

    def __init__(self, parent: QWidget):
        super().__init__(parent)
        self.setObjectName("Toast")
        self.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.hide()
        self._timer = QTimer(self)
        self._timer.setSingleShot(True)
        self._timer.timeout.connect(self.hide)
        parent.installEventFilter(self)

    def show_message(self, text: str, level: str = "info", msec: int = 2600) -> None:
        self.setText(text)
        self.adjustSize()
        self._reposition()
        self.show()
        self.raise_()
        self._timer.start(msec)

    def _reposition(self) -> None:
        parent = self.parentWidget()
        if parent is None:
            return
        x = max(12, parent.width() - self.width() - 24)
        y = max(12, parent.height() - self.height() - 40)
        self.move(QPoint(x, y))

    def eventFilter(self, obj, event):  # noqa: N802 - Qt 接口
        if obj is self.parentWidget() and event.type() == QEvent.Type.Resize and self.isVisible():
            self._reposition()
        return super().eventFilter(obj, event)


class ClickableLabel(QLabel):
    def __init__(self, text: str = "", parent=None):
        super().__init__(text, parent)
        self.setCursor(QCursor(Qt.CursorShape.PointingHandCursor))


def mono_font(size: int = 9) -> QFont:
    font = QFont("Cascadia Mono", size)
    font.setStyleHint(QFont.StyleHint.Monospace)
    return font
