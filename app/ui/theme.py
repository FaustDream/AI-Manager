# -*- coding: utf-8 -*-
"""统一视觉主题：浅色、留白、卡片化；深色为同构换色版。

所有颜色集中在 LIGHT / DARK 两张表，QSS 里只引用变量展开后的值。
运行期只有 PALETTE 一个符号，切主题时**原地更新**——各处
`from .theme import PALETTE` 拿到的是同一个 dict，不必重新导入。
"""
from __future__ import annotations

from PySide6.QtGui import QColor, QFont, QGuiApplication, QPalette
from PySide6.QtWidgets import QApplication, QWidget

LIGHT = {
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
    # —— 以下为控件细节色，深色下需要单独取值，不能靠半透明叠加
    "raised": "#FAFBFD",          # 表头、表角
    "line_soft": "#F1F3F7",       # 表格行分隔线、浅色标签底
    "pressed": "#E9ECF2",         # 按钮按下
    "disabled": "#F7F8FA",        # 禁用态底色
    "on_accent": "#FFFFFF",       # 铺在主色上的文字
    "accent_disabled": "#A8BEF5",
    "on_accent_dim": "#F5F8FF",
    "danger_dark": "#B91C1C",     # 危险按钮 hover
    "scroll": "#CDD3DC",
    "scroll_hover": "#B6BEC9",
    "code": "#FBFCFE",            # 日志 / JSON 编辑区
    "track": "#EDF0F5",           # 进度条槽
    "tip_bg": "#1F2430",          # 气泡提示
    "tip_text": "#FFFFFF",
}

DARK = {
    "bg": "#13151B",
    "sidebar": "#171A21",
    "card": "#1B1F27",
    "border": "#262C36",
    "border_strong": "#343C4A",
    "text": "#E8ECF3",
    "text_2": "#B6BECB",
    "muted": "#7E8797",
    "accent": "#4C82EA",
    "accent_dark": "#7CA5F7",     # 深色下 hover 反而要更亮，语义沿用键名
    "accent_soft": "#1E2A45",
    "ok": "#35C08A",
    "ok_soft": "#16332A",
    "warn": "#E3A44F",
    "warn_soft": "#33281A",
    "danger": "#F0776F",
    "danger_soft": "#3A2020",
    "hover": "#212733",
    "sel": "#22304E",
    "raised": "#1E232C",
    "line_soft": "#242832",
    "pressed": "#262C38",
    "disabled": "#191C23",
    "on_accent": "#FFFFFF",
    "accent_disabled": "#33436B",
    "on_accent_dim": "#C9D6F2",
    "danger_dark": "#FF8A8A",
    "scroll": "#3A4150",
    "scroll_hover": "#4A5364",
    "code": "#14171E",
    "track": "#242A34",
    "tip_bg": "#2B3140",
    "tip_text": "#EDF1F7",
}

# 全局唯一取色入口：切主题时原地更新，已持有该 dict 引用的模块自动跟着变
PALETTE = dict(LIGHT)

_MODE = "light"

# 表格行会把 PALETTE[...] 的色号字符串缓存下来（_fg_map / _fg）。切主题时这些
# 字符串还是旧配色，靠这张「色号 → 调色板键」的别名表在取色时映射回当前主题。
# 只收录文字色，避免 #FFFFFF 这类多义色号被误判。
_TEXT_KEYS = ("text", "text_2", "muted", "accent", "ok", "warn", "danger")
_ALIAS: dict[str, str] = {}
for _key in _TEXT_KEYS:
    _ALIAS[LIGHT[_key].lower()] = _key
    _ALIAS[DARK[_key].lower()] = _key

# 后面几个是 Linux / macOS 上的常见中文字体：Windows 首选微软雅黑，
# 换平台时 Qt 会顺着列表往下找，找不到才退到系统默认
FONT_FAMILY = ('"Microsoft YaHei UI", "Microsoft YaHei", "Segoe UI", '
               '"PingFang SC", "Noto Sans CJK SC", "Source Han Sans SC", sans-serif')
MONO_FAMILY = ('"Cascadia Mono", "Consolas", "SF Mono", "Menlo", '
               '"DejaVu Sans Mono", "Courier New", monospace')

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
    background: {p['tip_bg']};
    color: {p['tip_text']};
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
/* 署名被改掉时替换 BrandSub，用告警色让它一眼可见 */
QLabel#BrandByWarn {{
    font-size: 11px;
    color: {p['danger']};
}}
QLabel#BrandFoot {{
    font-size: 10px;
    color: {p['muted']};
    padding: 6px 0 2px 0;
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
/* 侧栏底部的加载提示：转圈旁边的文案，跟着主色走，一眼能看出还在跑 */
QFrame#BusyPill {{
    background: {p['accent_soft']};
    border-radius: 8px;
}}
QLabel#BusyText {{
    font-size: 12px;
    color: {p['accent']};
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
/* 概览数字卡左上角的图标底块 */
QLabel#StatChip {{
    background: {p['accent_soft']};
    border-radius: 8px;
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
    background: {p['pressed']};
}}
/* 一行要塞很多按钮时用：去掉图标，左右内边距收紧 */
QPushButton[compact="true"] {{
    padding: 7px 10px;
}}
QPushButton:disabled {{
    color: {p['muted']};
    background: {p['disabled']};
    border-color: {p['border']};
}}
QPushButton#Primary {{
    background: {p['accent']};
    border: 1px solid {p['accent']};
    color: {p['on_accent']};
    font-weight: 600;
}}
QPushButton#Primary:hover {{
    background: {p['accent_dark']};
    border-color: {p['accent_dark']};
}}
QPushButton#Primary:disabled {{
    background: {p['accent_disabled']};
    border-color: {p['accent_disabled']};
    color: {p['on_accent_dim']};
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
    color: {p['on_accent']};
    font-weight: 600;
}}
QPushButton#Danger:hover {{
    background: {p['danger_dark']};
    border-color: {p['danger_dark']};
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
/* 技能 / MCP 页签：两个并排的分段按钮，选中项像一颗贴住底色的药丸 */
QPushButton#Segment {{
    background: transparent;
    border: 1px solid transparent;
    color: {p['muted']};
    padding: 6px 14px;
    font-weight: 500;
}}
QPushButton#Segment:hover {{
    background: {p['hover']};
    color: {p['text_2']};
}}
QPushButton#Segment:checked {{
    background: {p['card']};
    border-color: {p['border']};
    color: {p['accent']};
    font-weight: 600;
}}

/* 只有图标的方形按钮：侧栏主题切换用，悬停才露底色 */
QPushButton#IconButton {{
    background: transparent;
    border: none;
    border-radius: 7px;
    padding: 0;
}}
QPushButton#IconButton:hover {{
    background: {p['hover']};
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
    selection-color: {p['on_accent']};
}}
QLineEdit:focus, QComboBox:focus {{
    border-color: {p['accent']};
}}
QLineEdit:disabled, QComboBox:disabled {{
    background: {p['disabled']};
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
    border-bottom: 1px solid {p['line_soft']};
}}
QTableView#DataTable::item:selected {{
    background: {p['sel']};
    color: {p['text']};
}}
QHeaderView::section {{
    background: {p['raised']};
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
    background: {p['raised']};
    border: none;
}}

/* ------------------------------------------------------------ 滚动条 */
QScrollBar:vertical {{
    background: transparent;
    width: 10px;
    margin: 2px;
}}
QScrollBar::handle:vertical {{
    background: {p['scroll']};
    border-radius: 4px;
    min-height: 28px;
}}
QScrollBar::handle:vertical:hover {{
    background: {p['scroll_hover']};
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
    background: {p['scroll']};
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
    background: {p['code']};
    border: 1px solid {p['border']};
    border-radius: 10px;
    padding: 8px 10px;
    font-family: {MONO_FAMILY};
    font-size: 11px;
    color: {p['text_2']};
    selection-background-color: {p['accent_soft']};
}}
/* MCP 编辑器与配置预览：等宽字体，方便对齐 JSON */
QPlainTextEdit#JsonEditor {{
    background: {p['code']};
    border: 1px solid {p['border']};
    border-radius: 10px;
    padding: 8px 10px;
    font-family: {MONO_FAMILY};
    font-size: 12px;
    color: {p['text_2']};
    selection-background-color: {p['accent_soft']};
}}
QListWidget#PickList {{
    background: {p['card']};
    border: 1px solid {p['border']};
    border-radius: 10px;
    padding: 6px;
    outline: none;
}}
QListWidget#PickList::item {{
    padding: 8px 10px;
    border-radius: 6px;
    color: {p['text_2']};
}}
QListWidget#PickList::item:selected {{
    background: {p['accent_soft']};
    color: {p['accent']};
}}
QLabel#PathHint {{
    font-family: {MONO_FAMILY};
    font-size: 11px;
    color: {p['muted']};
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
    background: {p['track']};
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
    background: {p['line_soft']};
    color: {p['muted']};
}}

/* ------------------------------------------------------------ 日志条 */
QLabel#StatusText {{
    font-size: 11px;
    color: {p['muted']};
}}
QLabel#Toast {{
    background: {p['tip_bg']};
    color: {p['tip_text']};
    border-radius: 8px;
    padding: 8px 14px;
    font-size: 12px;
}}

/* ------------------------------------------------------------ 对话框 */
/* 说明性长文本：设置页的作者信息、功能范围与风险提示 */
QLabel#BodyText {{
    font-size: 12px;
    color: {p['text_2']};
}}
QLabel#BodyText[tone="danger"] {{
    color: {p['danger']};
}}
QLabel#ErrorText {{
    font-size: 12px;
    color: {p['danger']};
}}
/* 会话正文预览：段落颜色由富文本内联给出，底色跟着卡片走 */
QTextBrowser#SessionDetail {{
    background: transparent;
    border: none;
    color: {p['text']};
    selection-background-color: {p['accent_soft']};
    selection-color: {p['text']};
}}
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


def mode() -> str:
    return _MODE


def set_mode(name: str) -> str:
    """切浅色 / 深色。只改内存色板，重刷界面由 apply() 负责。"""
    global _MODE
    _MODE = "dark" if str(name).lower() == "dark" else "light"
    PALETTE.clear()
    PALETTE.update(DARK if _MODE == "dark" else LIGHT)
    return _MODE


def resolve(value: str) -> str:
    """把页面上缓存的旧主题色号换回当前主题的同色。

    表格行在构建时就把十六进制存进了 _fg_map，切主题不会重建这些行；
    取色时过一道映射，深色下才不会残留浅色字。
    """
    key = _ALIAS.get(str(value).lower())
    return PALETTE[key] if key else value


def _app_palette() -> QPalette:
    """QSS 管不到的部件（原生对话框、spinbox 箭头等）靠调色板兜底。"""
    p = PALETTE
    palette = QPalette()
    roles = {
        QPalette.ColorRole.Window: "bg",
        QPalette.ColorRole.WindowText: "text",
        QPalette.ColorRole.Base: "card",
        QPalette.ColorRole.AlternateBase: "hover",
        QPalette.ColorRole.Text: "text",
        QPalette.ColorRole.Button: "card",
        QPalette.ColorRole.ButtonText: "text",
        QPalette.ColorRole.ToolTipBase: "tip_bg",
        QPalette.ColorRole.ToolTipText: "tip_text",
        QPalette.ColorRole.Highlight: "accent",
        QPalette.ColorRole.HighlightedText: "on_accent",
        QPalette.ColorRole.PlaceholderText: "muted",
        QPalette.ColorRole.Link: "accent",
    }
    for group in (QPalette.ColorGroup.Active, QPalette.ColorGroup.Inactive):
        for role, key in roles.items():
            palette.setColor(group, role, QColor(p[key]))
    disabled = QPalette.ColorGroup.Disabled
    palette.setColor(disabled, QPalette.ColorRole.Window, QColor(p["bg"]))
    palette.setColor(disabled, QPalette.ColorRole.Base, QColor(p["disabled"]))
    palette.setColor(disabled, QPalette.ColorRole.Button, QColor(p["card"]))
    palette.setColor(disabled, QPalette.ColorRole.Highlight, QColor(p["border"]))
    palette.setColor(disabled, QPalette.ColorRole.HighlightedText, QColor(p["muted"]))
    for role in (QPalette.ColorRole.WindowText, QPalette.ColorRole.Text,
                 QPalette.ColorRole.ButtonText):
        palette.setColor(disabled, role, QColor(p["muted"]))
    return palette


def _refresh_widgets(app: QApplication) -> None:
    """重刷按色号缓存过的东西：图标位图、画刷颜色、内联富文本。

    约定两条，界面上不必各自订阅主题事件：
    - setProperty("themeIcon", (图标名, 调色板键, 尺寸)) 的控件会重渲染图标；
    - 定义了 refresh_theme() 的控件会被调用一次。
    """
    from . import icons

    for widget in app.allWidgets():
        spec = widget.property("themeIcon")
        setter = getattr(widget, "setIcon", None)
        if spec and setter is not None:
            name, tone, size = spec
            setter(icons.icon(name, PALETTE.get(tone, PALETTE["text_2"]), size))
        hook = getattr(widget, "refresh_theme", None)
        if callable(hook):
            hook()
        widget.update()


def apply(app: QApplication) -> None:
    app.setStyle("Fusion")
    font = QFont()
    # setFamilies 才有多级回退；QFont("Microsoft YaHei UI") 在缺字体的平台上只会静默退化
    font.setFamilies(["Microsoft YaHei UI", "Microsoft YaHei", "Segoe UI",
                      "PingFang SC", "Noto Sans CJK SC", "Source Han Sans SC"])
    font.setPointSize(10)
    font.setHintingPreference(QFont.HintingPreference.PreferDefaultHinting)
    app.setFont(font)
    # setStyle 会把调色板重置成 Fusion 的浅色默认值，顺序不能反
    app.setPalette(_app_palette())
    app.setStyleSheet(qss())
    _refresh_widgets(app)


def repolish(widget: QWidget) -> None:
    """动态改 property 后必须重新 polish，否则 QSS 不生效。"""
    style = widget.style()
    style.unpolish(widget)
    style.polish(widget)
    widget.update()


def color(name: str) -> QColor:
    return QColor(PALETTE.get(name, "#000000"))
