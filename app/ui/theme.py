# -*- coding: utf-8 -*-
"""统一视觉主题：浅色、留白、卡片化。

所有颜色集中在 PALETTE，QSS 里只引用变量展开后的值，
改主题只需要动这一处。
"""
from __future__ import annotations

from PySide6.QtGui import QColor, QFont, QGuiApplication
from PySide6.QtWidgets import QApplication, QWidget

PALETTE = {
    "bg": "#F4F6F9",
    "sidebar": "#FFFFFF",
    "card": "#FFFFFF",
    "border": "#E5E8EE",
    "border_strong": "#D6DBE4",
    "text": "#171A20",
    "text_2": "#4B5563",
    "muted": "#8A919E",
    "accent": "#2563EB",
    "accent_dark": "#1D4ED8",
    "accent_soft": "#EEF3FF",
    "ok": "#0E9F6E",
    "ok_soft": "#E7F7F0",
    "warn": "#B45309",
    "warn_soft": "#FDF3E3",
    "danger": "#DC2626",
    "danger_soft": "#FDECEC",
    "hover": "#F3F5F9",
    "sel": "#E8EFFF",
}

FONT_FAMILY = '"Microsoft YaHei UI", "Microsoft YaHei", "Segoe UI"'
MONO_FAMILY = '"Cascadia Mono", "Consolas", "Courier New"'

_CHECK_ICON_CACHE: str | None = None


def _check_icon_url() -> str:
    """勾选图标渲染成 png 交给 QSS（QSS 没法直接画 SVG path）。

    只在首次生成，放在 data/cache 下，失败时静默退回纯色块。
    """
    global _CHECK_ICON_CACHE
    if _CHECK_ICON_CACHE is not None:
        return _CHECK_ICON_CACHE
    if QGuiApplication.instance() is None:
        return ""                     # 还没有 GUI 应用，建不了 QPixmap，也不缓存
    _CHECK_ICON_CACHE = ""
    try:
        from ..core import paths
        from . import icons

        target = paths.DATA_DIR / "cache" / "check.png"
        target.parent.mkdir(parents=True, exist_ok=True)
        if icons.pixmap("check", "#FFFFFF", 12).save(str(target), "PNG"):
            _CHECK_ICON_CACHE = target.as_posix()
    except Exception:  # 图标只是装饰，任何异常都不该影响启动
        pass
    return _CHECK_ICON_CACHE


def qss() -> str:
    p = PALETTE
    check = _check_icon_url()
    check_rule = f'image: url("{check}");' if check else ""
    return f"""
/* ------------------------------------------------------------ 基础 */
QWidget {{
    font-family: {FONT_FAMILY};
    font-size: 13px;
    color: {p['text']};
}}
QMainWindow, QWidget#Root {{
    background: {p['bg']};
}}
QToolTip {{
    background: #1F2430;
    color: #FFFFFF;
    border: none;
    padding: 6px 8px;
    border-radius: 6px;
    font-size: 12px;
}}

/* ------------------------------------------------------------ 侧边栏 */
QFrame#Sidebar {{
    background: {p['sidebar']};
    border-right: 1px solid {p['border']};
}}
QLabel#BrandName {{
    font-size: 15px;
    font-weight: 600;
    color: {p['text']};
}}
QLabel#BrandSub {{
    font-size: 11px;
    color: {p['muted']};
}}
QLabel#BrandTag {{
    font-size: 10px;
    color: {p['accent']};
    background: {p['accent_soft']};
    border-radius: 4px;
    padding: 1px 5px;
}}
QLabel#NavGroup {{
    font-size: 10px;
    color: {p['muted']};
    letter-spacing: 1px;
    padding: 0 0 0 12px;
}}
QPushButton#NavItem {{
    text-align: left;
    padding: 0 10px 0 12px;
    border: none;
    border-radius: 8px;
    background: transparent;
    color: {p['text_2']};
    font-size: 13px;
    min-height: 38px;
    max-height: 38px;
}}
QPushButton#NavItem:hover {{
    background: {p['hover']};
}}
QPushButton#NavItem:checked {{
    background: {p['accent_soft']};
    color: {p['accent']};
    font-weight: 600;
}}
QFrame#SideFooter {{
    background: {p['sidebar']};
    border-top: 1px solid {p['border']};
}}
QLabel#SideHint {{
    font-size: 11px;
    color: {p['muted']};
}}

/* ------------------------------------------------------------ 页头 */
QFrame#TopBar {{
    background: transparent;
    border: none;
}}
QLabel#PageTitle {{
    font-size: 21px;
    font-weight: 600;
    color: {p['text']};
}}
QLabel#PageSub {{
    font-size: 12px;
    color: {p['muted']};
}}
QLabel#SectionTitle {{
    font-size: 14px;
    font-weight: 600;
    color: {p['text']};
}}
QLabel#SectionHint {{
    font-size: 12px;
    color: {p['muted']};
}}

/* ------------------------------------------------------------ 卡片 */
QFrame#Card {{
    background: {p['card']};
    border: 1px solid {p['border']};
    border-radius: 12px;
}}
QFrame#CardFlat {{
    background: {p['card']};
    border: 1px solid {p['border']};
    border-radius: 10px;
}}
QLabel#StatValue {{
    font-size: 26px;
    font-weight: 600;
    color: {p['text']};
}}
QLabel#StatLabel {{
    font-size: 12px;
    color: {p['text_2']};
}}
QLabel#StatHint {{
    font-size: 11px;
    color: {p['muted']};
}}
QLabel#KeyLabel {{
    font-size: 11px;
    color: {p['muted']};
}}
QLabel#ValueLabel {{
    font-size: 12px;
    color: {p['text']};
}}
QFrame#Divider {{
    background: {p['border']};
    max-height: 1px;
    min-height: 1px;
    border: none;
}}

/* ------------------------------------------------------------ 按钮 */
QPushButton {{
    font-size: 13px;
    border-radius: 8px;
    padding: 7px 14px;
    border: 1px solid {p['border']};
    background: {p['card']};
    color: {p['text']};
}}
QPushButton:hover {{
    background: {p['hover']};
    border-color: {p['border_strong']};
}}
QPushButton:pressed {{
    background: #E9ECF2;
}}
QPushButton:disabled {{
    color: {p['muted']};
    background: #F7F8FA;
    border-color: {p['border']};
}}
QPushButton#Primary {{
    background: {p['accent']};
    border: 1px solid {p['accent']};
    color: #FFFFFF;
    font-weight: 600;
}}
QPushButton#Primary:hover {{
    background: {p['accent_dark']};
    border-color: {p['accent_dark']};
}}
QPushButton#Primary:disabled {{
    background: #A8BEF5;
    border-color: #A8BEF5;
    color: #F5F8FF;
}}
QPushButton#Ghost {{
    background: transparent;
    border: 1px solid {p['border']};
    color: {p['text_2']};
}}
QPushButton#Ghost:hover {{
    background: {p['hover']};
    color: {p['text']};
}}
QPushButton#Danger {{
    background: {p['danger']};
    border: 1px solid {p['danger']};
    color: #FFFFFF;
    font-weight: 600;
}}
QPushButton#Danger:hover {{
    background: #B91C1C;
    border-color: #B91C1C;
}}
QPushButton#DangerGhost {{
    background: transparent;
    border: 1px solid {p['danger']};
    color: {p['danger']};
}}
QPushButton#DangerGhost:hover {{
    background: {p['danger_soft']};
}}
QPushButton#Link {{
    background: transparent;
    border: none;
    color: {p['accent']};
    padding: 2px 4px;
    font-size: 12px;
}}
QPushButton#Link:hover {{
    color: {p['accent_dark']};
    text-decoration: underline;
}}
QPushButton#RowAction {{
    background: transparent;
    border: none;
    color: {p['accent']};
    padding: 0 2px;
    font-size: 11px;
}}
QPushButton#RowAction:hover {{
    color: {p['accent_dark']};
    text-decoration: underline;
}}
QPushButton#RowAction:disabled {{
    color: {p['muted']};
    text-decoration: none;
}}

/* ------------------------------------------------------------ 输入
   注意：QTimeEdit / QSpinBox 这类 QAbstractSpinBox 千万不要在 QSS 里写
   border 或 padding —— Qt 的样式层会把内部编辑区算成 0，文字直接不显示。 */
QLineEdit, QComboBox, QSpinBox {{
    background: {p['card']};
    border: 1px solid {p['border']};
    border-radius: 8px;
    padding: 6px 10px;
    min-height: 20px;
    selection-background-color: {p['accent']};
    selection-color: #FFFFFF;
}}
QLineEdit:focus, QComboBox:focus {{
    border-color: {p['accent']};
}}
QLineEdit:disabled, QComboBox:disabled {{
    background: #F7F8FA;
    color: {p['muted']};
}}
QComboBox::drop-down {{
    border: none;
    width: 22px;
}}
QComboBox QAbstractItemView {{
    background: {p['card']};
    border: 1px solid {p['border']};
    border-radius: 8px;
    padding: 4px;
    outline: none;
    selection-background-color: {p['accent_soft']};
    selection-color: {p['accent']};
}}
QCheckBox {{
    font-size: 12px;
    color: {p['text_2']};
    spacing: 6px;
}}
QCheckBox::indicator {{
    width: 15px;
    height: 15px;
    border-radius: 4px;
    border: 1px solid {p['border_strong']};
    background: {p['card']};
}}
QCheckBox::indicator:hover {{
    border-color: {p['accent']};
}}
QCheckBox::indicator:checked {{
    background: {p['accent']};
    border-color: {p['accent']};
    {check_rule}
}}

/* ------------------------------------------------------------ 表格 */
QTableView#DataTable {{
    background: {p['card']};
    border: none;
    gridline-color: transparent;
    selection-background-color: {p['sel']};
    selection-color: {p['text']};
    outline: none;
    font-size: 12px;
}}
QTableView#DataTable::item {{
    padding: 0 8px;
    border-bottom: 1px solid #F1F3F7;
}}
QTableView#DataTable::item:selected {{
    background: {p['sel']};
    color: {p['text']};
}}
QHeaderView::section {{
    background: #FAFBFD;
    color: {p['muted']};
    border: none;
    border-bottom: 1px solid {p['border']};
    /* 竖线给出可拖动列宽的位置提示，没有它看不出哪一段能拉 */
    border-right: 1px solid {p['border']};
    padding: 0 8px;
    height: 34px;
    font-size: 11px;
    font-weight: 600;
}}
QTableCornerButton::section {{
    background: #FAFBFD;
    border: none;
}}

/* ------------------------------------------------------------ 滚动条 */
QScrollBar:vertical {{
    background: transparent;
    width: 10px;
    margin: 2px;
}}
QScrollBar::handle:vertical {{
    background: #CDD3DC;
    border-radius: 4px;
    min-height: 28px;
}}
QScrollBar::handle:vertical:hover {{
    background: #B6BEC9;
}}
QScrollBar::add-line:vertical, QScrollBar::sub-line:vertical,
QScrollBar::add-page:vertical, QScrollBar::sub-page:vertical {{
    background: transparent;
    height: 0;
}}
QScrollBar:horizontal {{
    background: transparent;
    height: 10px;
    margin: 2px;
}}
QScrollBar::handle:horizontal {{
    background: #CDD3DC;
    border-radius: 4px;
    min-width: 28px;
}}
QScrollBar::add-line:horizontal, QScrollBar::sub-line:horizontal,
QScrollBar::add-page:horizontal, QScrollBar::sub-page:horizontal {{
    background: transparent;
    width: 0;
}}

/* ------------------------------------------------------------ 日志 */
QPlainTextEdit#Log {{
    background: #FBFCFE;
    border: 1px solid {p['border']};
    border-radius: 10px;
    padding: 8px 10px;
    font-family: {MONO_FAMILY};
    font-size: 11px;
    color: {p['text_2']};
    selection-background-color: {p['accent_soft']};
}}
QFrame#LogBar {{
    background: {p['card']};
    border: 1px solid {p['border']};
    border-radius: 10px;
}}
QLabel#LogTitle {{
    font-size: 11px;
    font-weight: 600;
    color: {p['text_2']};
}}

/* ------------------------------------------------------------ 进度 */
QProgressBar {{
    border: none;
    background: #EDF0F5;
    border-radius: 3px;
    height: 6px;
    text-align: center;
}}
QProgressBar::chunk {{
    background: {p['accent']};
    border-radius: 3px;
}}
QProgressBar#ThinBar {{
    height: 4px;
}}

/* ------------------------------------------------------------ 标签块 */
QLabel#Badge {{
    font-size: 11px;
    border-radius: 5px;
    padding: 2px 7px;
    background: {p['hover']};
    color: {p['text_2']};
}}
QLabel#Badge[tone="ok"] {{
    background: {p['ok_soft']};
    color: {p['ok']};
}}
QLabel#Badge[tone="warn"] {{
    background: {p['warn_soft']};
    color: {p['warn']};
}}
QLabel#Badge[tone="danger"] {{
    background: {p['danger_soft']};
    color: {p['danger']};
}}
QLabel#Badge[tone="accent"] {{
    background: {p['accent_soft']};
    color: {p['accent']};
}}
QLabel#Badge[tone="muted"] {{
    background: #F1F3F7;
    color: {p['muted']};
}}

/* ------------------------------------------------------------ 日志条 */
QLabel#StatusText {{
    font-size: 11px;
    color: {p['muted']};
}}
QLabel#Toast {{
    background: #1F2430;
    color: #FFFFFF;
    border-radius: 8px;
    padding: 8px 14px;
    font-size: 12px;
}}

/* ------------------------------------------------------------ 对话框 */
QDialog {{
    background: {p['bg']};
}}
QDialog QLabel {{
    color: {p['text']};
}}
QMessageBox {{
    background: {p['card']};
}}
QMenu {{
    background: {p['card']};
    border: 1px solid {p['border']};
    border-radius: 8px;
    padding: 4px;
}}
QMenu::item {{
    padding: 6px 18px 6px 12px;
    border-radius: 6px;
}}
QMenu::item:selected {{
    background: {p['accent_soft']};
    color: {p['accent']};
}}
QMenu::separator {{
    height: 1px;
    background: {p['border']};
    margin: 4px 6px;
}}
QSplitter::handle {{
    background: transparent;
}}

/* ------------------------------------------------------------ 滚动区
   viewport 直接子级（内容容器）透明；不得使用无选择器的局部样式表，
   否则会级联污染按钮等后代控件。 */
QScrollArea {{
    background: transparent;
    border: none;
}}
QScrollArea > QWidget > QWidget {{
    background: transparent;
}}
"""


def apply(app: QApplication) -> None:
    app.setStyle("Fusion")
    font = QFont("Microsoft YaHei UI", 10)
    font.setHintingPreference(QFont.HintingPreference.PreferDefaultHinting)
    app.setFont(font)
    app.setStyleSheet(qss())


def repolish(widget: QWidget) -> None:
    """动态改 property 后必须重新 polish，否则 QSS 不生效。"""
    style = widget.style()
    style.unpolish(widget)
    style.polish(widget)
    widget.update()


def color(name: str) -> QColor:
    return QColor(PALETTE.get(name, "#000000"))
