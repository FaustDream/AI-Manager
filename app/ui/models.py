# -*- coding: utf-8 -*-
"""表格模型：字典列表 ↔ QTableView。

行是 dict，列定义决定取值与格式化；需要局部染色时给行加 _fg_map，
需要整行强调时给行加 _bold / _fg。
"""
from __future__ import annotations

from typing import Callable

from PySide6.QtCore import QAbstractTableModel, QModelIndex, Qt
from PySide6.QtGui import QColor, QFont

from .theme import resolve

ALIGN = {
    "left": Qt.AlignmentFlag.AlignLeft | Qt.AlignmentFlag.AlignVCenter,
    "center": Qt.AlignmentFlag.AlignHCenter | Qt.AlignmentFlag.AlignVCenter,
    "right": Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter,
}

# 勾选列的列名：DataTable(checkable=True) 自动插一列，行上用 _checked 记状态
CHECK_KEY = "rowcheck"

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
                 fmt: Callable | None = None, mono: bool = False, stretch: bool = False,
                 fixed: bool = False):
        self.key = key
        self.title = title
        self.width = width
        self.align = ALIGN.get(align, ALIGN["left"])
        self.fmt = fmt
        self.mono = mono
        self.stretch = stretch
        # 固定列：宽度由定义决定，用户拖不动，也不被历史列宽配置覆盖
        self.fixed = fixed


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

    def flags(self, index):  # noqa: N802 - Qt 接口
        """行可拖可放；勾选列可点，分组标题行（_section）只亮不可选。"""
        base = Qt.ItemFlag.ItemIsEnabled | Qt.ItemFlag.ItemIsSelectable
        if not index.isValid():
            return base | Qt.ItemFlag.ItemIsDropEnabled
        row = self.rows[index.row()]
        if row.get("_section"):
            return Qt.ItemFlag.ItemIsEnabled
        if self.columns[index.column()].key == CHECK_KEY:
            return base | Qt.ItemFlag.ItemIsUserCheckable
        return base | Qt.ItemFlag.ItemIsDragEnabled | Qt.ItemFlag.ItemIsDropEnabled

    def supportedDropActions(self):  # noqa: N802 - Qt 接口
        # 只做行内重排，Move 才能让视图显示「移动」光标；Copy 语义在这里没意义
        return Qt.DropAction.MoveAction

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
                return QColor(resolve(cell_map[column.key]))
            color = row.get("_fg")
            return QColor(resolve(color)) if color else None
        if role == Qt.ItemDataRole.FontRole:
            return _cell_font(column.mono, bool(row.get("_bold")) and not column.mono)
        if role == Qt.ItemDataRole.CheckStateRole:
            if column.key != CHECK_KEY or row.get("_section"):
                return None
            return Qt.CheckState.Checked if row.get("_checked") else Qt.CheckState.Unchecked
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

    def setData(self, index, value, role=Qt.ItemDataRole.EditRole):  # noqa: N802
        """勾选列的开关：状态记在行的 _checked 上，界面线程内完成。"""
        if role != Qt.ItemDataRole.CheckStateRole or not index.isValid():
            return False
        column = self.columns[index.column()]
        row = self.rows[index.row()]
        if column.key != CHECK_KEY or row.get("_section"):
            return False
        row["_checked"] = value in (Qt.CheckState.Checked, Qt.CheckState.Checked.value)
        self.dataChanged.emit(index, index, [Qt.ItemDataRole.CheckStateRole])
        return True

    # --------------------------------------------------------------- 数据操作
    def set_rows(self, rows: list[dict]) -> None:
        self.beginResetModel()
        self.rows = list(rows)
        self._reindex()
        self.endResetModel()

    def _reindex(self) -> None:
        self._index = {}
        for position, row in enumerate(self.rows):
            value = row.get(self.key)
            if value is not None:
                self._index[str(value)] = position

    def move_row(self, source: int, target: int) -> bool:
        """把 source 行移到 target（移除源行后的插入位置）。

        用 beginMoveRows 而不是整表 reset：选中行、滚动位置和行内控件都跟着走，
        拖完不会出现「选中丢了、详情面板关了」的跳变。
        """
        if source == target:
            return False
        if not (0 <= source < len(self.rows)) or not (0 <= target < len(self.rows)):
            return False
        # beginMoveRows 的落点是「插到第几行之前」，向下移动要跳过自己占的那一格
        destination = target + 1 if target > source else target
        if not self.beginMoveRows(QModelIndex(), source, source,
                                  QModelIndex(), destination):
            return False
        self.rows.insert(target, self.rows.pop(source))
        self._reindex()
        self.endMoveRows()
        return True

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
