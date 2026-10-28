# -*- coding: utf-8 -*-
"""表格模型：字典列表 ↔ QTableView。

行是 dict，列定义决定取值与格式化；需要局部染色时给行加 _fg_map，
需要整行强调时给行加 _bold / _fg。
"""
from __future__ import annotations

from typing import Callable

from PySide6.QtCore import QAbstractTableModel, QModelIndex, Qt
from PySide6.QtGui import QColor, QFont

ALIGN = {
    "left": Qt.AlignmentFlag.AlignLeft | Qt.AlignmentFlag.AlignVCenter,
    "center": Qt.AlignmentFlag.AlignHCenter | Qt.AlignmentFlag.AlignVCenter,
    "right": Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter,
}

# 每个单元格都会问一次字体，现建 QFont 在几百行的表里就是白白的开销
_FONTS: dict[tuple, QFont] = {}


def _cell_font(mono: bool, bold: bool) -> QFont:
    key = (mono, bold)
    font = _FONTS.get(key)
    if font is None:
        if mono:
            font = QFont("Cascadia Mono", 9)
            font.setStyleHint(QFont.StyleHint.Monospace)
        else:
            font = QFont("Microsoft YaHei UI", 9)
            if bold:
                font.setBold(True)
        _FONTS[key] = font
    return font


class Column:
    def __init__(self, key: str, title: str, width: int = 120, align: str = "left",
                 fmt: Callable | None = None, mono: bool = False, stretch: bool = False):
        self.key = key
        self.title = title
        self.width = width
        self.align = ALIGN.get(align, ALIGN["left"])
        self.fmt = fmt
        self.mono = mono
        self.stretch = stretch


class RowModel(QAbstractTableModel):
    def __init__(self, columns: list[Column], rows: list[dict] | None = None,
                 key: str = "id"):
        super().__init__()
        self.columns = columns
        self.rows: list[dict] = []
        self.key = key
        self._index: dict[str, int] = {}
        self.set_rows(rows or [])

    # --------------------------------------------------------------- Qt 接口
    def rowCount(self, parent=QModelIndex()) -> int:  # noqa: N802
        return 0 if parent.isValid() else len(self.rows)

    def columnCount(self, parent=QModelIndex()) -> int:  # noqa: N802
        return 0 if parent.isValid() else len(self.columns)

    def headerData(self, section, orientation, role=Qt.ItemDataRole.DisplayRole):  # noqa: N802
        if role != Qt.ItemDataRole.DisplayRole:
            return None
        if orientation == Qt.Orientation.Horizontal:
            return self.columns[section].title
        return None

    def data(self, index: QModelIndex, role=Qt.ItemDataRole.DisplayRole):
        if not index.isValid():
            return None
        row = self.rows[index.row()]
        column = self.columns[index.column()]
        if role == Qt.ItemDataRole.DisplayRole:
            value = row.get(column.key, "")
            if column.fmt is not None:
                try:
                    return column.fmt(value, row)
                except TypeError:
                    return column.fmt(value)
            return "" if value is None else str(value)
        if role == Qt.ItemDataRole.TextAlignmentRole:
            return column.align
        if role == Qt.ItemDataRole.ForegroundRole:
            cell_map = row.get("_fg_map") or {}
            if column.key in cell_map:
                return QColor(cell_map[column.key])
            color = row.get("_fg")
            return QColor(color) if color else None
        if role == Qt.ItemDataRole.FontRole:
            return _cell_font(column.mono, bool(row.get("_bold")) and not column.mono)
        if role == Qt.ItemDataRole.ToolTipRole:
            tip = (row.get("_tip_map") or {}).get(column.key)
            if tip:
                return tip
            value = row.get(column.key, "")
            text = str(value) if value is not None else ""
            return text if len(text) > 18 else None
        if role == Qt.ItemDataRole.UserRole:
            return row
        return None

    # --------------------------------------------------------------- 数据操作
    def set_rows(self, rows: list[dict]) -> None:
        self.beginResetModel()
        self.rows = list(rows)
        self._index = {}
        for position, row in enumerate(self.rows):
            value = row.get(self.key)
            if value is not None:
                self._index[str(value)] = position
        self.endResetModel()

    def row_dict(self, row_index: int) -> dict | None:
        if 0 <= row_index < len(self.rows):
            return self.rows[row_index]
        return None

    def index_of(self, key) -> int:
        return self._index.get(str(key), -1)

    def update(self, key, **changes) -> None:
        position = self.index_of(key)
        if position < 0:
            return
        self.rows[position].update(changes)
        left = self.index(position, 0)
        right = self.index(position, len(self.columns) - 1)
        self.dataChanged.emit(left, right)

    def touch(self, key) -> None:
        position = self.index_of(key)
        if position < 0:
            return
        self.dataChanged.emit(self.index(position, 0),
                              self.index(position, len(self.columns) - 1))

    def append(self, row: dict) -> None:
        position = len(self.rows)
        self.beginInsertRows(QModelIndex(), position, position)
        self.rows.append(row)
        value = row.get(self.key)
        if value is not None:
            self._index[str(value)] = position
        self.endInsertRows()
