# -*- coding: utf-8 -*-
"""通用界面组件：卡片、统计块、徽标、表格、日志、气泡提示。"""
from __future__ import annotations

import html

from PySide6.QtCore import QEvent, QPoint, QRectF, QSize, QTime, QTimer, Qt, Signal
from PySide6.QtGui import QColor, QCursor, QFont, QPainter, QPen
from PySide6.QtWidgets import (QAbstractItemView, QAbstractSpinBox, QFrame, QHBoxLayout,
                               QHeaderView, QLabel, QPlainTextEdit, QPushButton,
                               QSizePolicy, QTableView, QTimeEdit, QToolButton,
                               QVBoxLayout, QWidget)

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


class TimeField(QFrame):
    """HH:mm 时间输入框。

    原生 QTimeEdit 的上下按钮在圆角边框里会被裁掉半个箭头，尺寸也很难对齐。
    这里改成「无按钮的时间输入 + 自绘步进按钮」，外观完全交给 QSS，
    对外接口与 QTimeEdit 一致：time() / set_time() / timeChanged。
    """

    timeChanged = Signal(QTime)

    def __init__(self, parent=None, step_minutes: int = 1):
        super().__init__(parent)
        self.setObjectName("TimeField")
        self.setProperty("active", False)
        self.setFixedHeight(34)
        self.setSizePolicy(QSizePolicy.Policy.Fixed, QSizePolicy.Policy.Fixed)
        self._step = max(1, int(step_minutes))

        row = QHBoxLayout(self)
        row.setContentsMargins(9, 0, 4, 0)
        row.setSpacing(4)

        icon = QLabel()
        icon.setPixmap(icons.pixmap("clock", PALETTE["muted"], 14))
        icon.setFixedSize(14, 14)
        icon.setAttribute(Qt.WidgetAttribute.WA_TransparentForMouseEvents, True)
        row.addWidget(icon)

        self.edit = QTimeEdit()
        self.edit.setObjectName("TimeFieldInput")
        self.edit.setDisplayFormat("HH:mm")
        self.edit.setButtonSymbols(QAbstractSpinBox.ButtonSymbols.NoButtons)
        self.edit.setFrame(False)  # 边框由外层容器画，避免和 QSS 打架
        self.edit.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.edit.setFixedWidth(54)
        self.edit.setSizePolicy(QSizePolicy.Policy.Fixed, QSizePolicy.Policy.Expanding)
        self.edit.setTime(QTime(8, 30))
        self.edit.setToolTip("可直接键入时间，↑ ↓ 微调；按住右侧箭头连续调整")
        self.edit.installEventFilter(self)
        self.edit.timeChanged.connect(self.timeChanged)
        row.addWidget(self.edit)

        steps = QVBoxLayout()
        steps.setContentsMargins(0, 3, 0, 3)
        steps.setSpacing(0)
        self.btn_up = self._step_button("chevron-up", 1)
        self.btn_down = self._step_button("chevron-down", -1)
        steps.addWidget(self.btn_up)
        steps.addWidget(self.btn_down)
        row.addLayout(steps)

        self.setFixedWidth(105)

    # ------------------------------------------------------------------ 内部
    def _step_button(self, icon_name: str, direction: int) -> QToolButton:
        button = QToolButton(self)
        button.setObjectName("TimeStep")
        button.setIcon(icons.icon(icon_name, PALETTE["text_2"], 10))
        button.setIconSize(QSize(10, 10))
        button.setFixedSize(16, 13)
        button.setCursor(QCursor(Qt.CursorShape.PointingHandCursor))
        button.setFocusPolicy(Qt.FocusPolicy.NoFocus)
        button.setAutoRaise(True)
        button.setAutoRepeat(True)
        button.setAutoRepeatDelay(280)
        button.setAutoRepeatInterval(90)
        button.clicked.connect(lambda: self._bump(direction))
        return button

    def _bump(self, direction: int) -> None:
        # 以分钟为单位整体步进，比按「当前选中的时/分」加减更符合直觉
        self.edit.setTime(self.edit.time().addSecs(direction * self._step * 60))

    # ------------------------------------------------------------------ 接口
    def time(self) -> QTime:
        return self.edit.time()

    def set_time(self, value: QTime) -> None:
        blocked = self.edit.blockSignals(True)
        self.edit.setTime(value)
        self.edit.blockSignals(blocked)

    # ------------------------------------------------------------------ 事件
    def mousePressEvent(self, event) -> None:  # noqa: N802 - Qt 接口
        self.edit.setFocus(Qt.FocusReason.MouseFocusReason)
        super().mousePressEvent(event)

    def eventFilter(self, obj, event):  # noqa: N802 - Qt 接口
        if obj is self.edit:
            if event.type() == QEvent.Type.FocusIn:
                self.setProperty("active", True)
                repolish(self)
            elif event.type() == QEvent.Type.FocusOut:
                self.setProperty("active", False)
                repolish(self)
        return super().eventFilter(obj, event)


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
        outer.addWidget(self.hint)

    def set_value(self, value: str, hint: str | None = None) -> None:
        self.value.setText(value)
        if hint is not None:
            self.hint.setText(hint)


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
    # 行被拖到新位置：(源行, 目标行)。模型不支持 moveRows，由页面自己重排数据
    row_moved = Signal(int, int)

    def __init__(self, columns: list[Column], key: str = "id", multi: bool = True,
                 row_height: int = 36, parent=None):
        super().__init__(parent)
        self.setObjectName("DataTable")
        self.model = RowModel(columns, key=key)
        self.setModel(self.model)
        self.columns = columns

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

        # 行拖动排序：Qt 的模型不支持 moveRows，dropEvent 只负责算目标行并发信号
        self.setDragEnabled(True)
        self.setAcceptDrops(True)
        self.setDropIndicatorShown(True)
        self.setDragDropOverwriteMode(False)
        self.setDragDropMode(QAbstractItemView.DragDropMode.InternalMove)
        self.setDefaultDropAction(Qt.DropAction.MoveAction)

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
        """只计算落点所在行，真正的重排交给页面（模型不支持 moveRows）。"""
        point = event.position().toPoint()
        target = self.indexAt(point)
        source = self.currentIndex().row()
        target_row = target.row() if target.isValid() else self.model.rowCount() - 1
        if source < 0 or target_row < 0 or source == target_row:
            event.ignore()
            return
        self.row_moved.emit(source, target_row)
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
