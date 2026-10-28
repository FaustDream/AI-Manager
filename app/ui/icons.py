# -*- coding: utf-8 -*-
"""线性风格图标：内置 SVG path，运行期按颜色渲染成 QIcon。"""
from __future__ import annotations

from functools import lru_cache

from PySide6.QtCore import QByteArray, QRectF, Qt
from PySide6.QtGui import QIcon, QPainter, QPixmap
from PySide6.QtSvg import QSvgRenderer

from .theme import PALETTE

_PATHS = {
    "overview": (
        '<rect x="3" y="3" width="7.5" height="7.5" rx="2"/>'
        '<rect x="13.5" y="3" width="7.5" height="7.5" rx="2"/>'
        '<rect x="3" y="13.5" width="7.5" height="7.5" rx="2"/>'
        '<rect x="13.5" y="13.5" width="7.5" height="7.5" rx="2"/>'
    ),
    "checkin": (
        '<rect x="3" y="4.5" width="18" height="16.5" rx="3"/>'
        '<path d="M8 2.5v4M16 2.5v4M3 10h18"/>'
        '<path d="m8.2 15.4 2.6 2.6 5.2-5.2"/>'
    ),
    "accounts": (
        '<circle cx="9" cy="8" r="3.4"/>'
        '<path d="M3.2 20c0-3.3 2.6-5.4 5.8-5.4s5.8 2.1 5.8 5.4"/>'
        '<path d="M16.4 7.2a3 3 0 0 1 0 5.8M18.6 20c0-2.3-.7-4-2.2-5"/>'
    ),
    "sessions": (
        '<path d="M21 12.2c0 4.1-4 7.4-9 7.4-1 0-2-.1-2.9-.4L4 21l1.3-3.4C4 16.3 3 14.4 3 12.2'
        ' 3 8.1 7 4.8 12 4.8s9 3.3 9 7.4Z"/>'
        '<path d="M8.5 11.5h7M8.5 14.5h4.5"/>'
    ),
    "profiles": (
        '<path d="M4 8.5 12 4l8 4.5-8 4.5-8-4.5Z"/>'
        '<path d="M4 8.5V16l8 4.5 8-4.5V8.5"/>'
        '<path d="M12 13v7.5"/>'
    ),
    "maintenance": (
        '<path d="M12 3.2 20 6.6v5.2c0 4.6-3.3 8-8 9.2-4.7-1.2-8-4.6-8-9.2V6.6l8-3.4Z"/>'
        '<path d="m8.8 12.2 2.4 2.4 4.2-4.6"/>'
    ),
    "settings": (
        '<circle cx="12" cy="12" r="3.2"/>'
        '<path d="M19.4 15a1.6 1.6 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.6 1.6 0 0 0-1.8-.3'
        ' 1.6 1.6 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.6 1.6 0 0 0-1-1.5 1.6 1.6 0 0 0-1.8.3l-.1.1'
        'a2 2 0 1 1-2.8-2.8l.1-.1a1.6 1.6 0 0 0 .3-1.8 1.6 1.6 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1'
        'a1.6 1.6 0 0 0 1.5-1 1.6 1.6 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.6 1.6 0 0 0 1.8.3'
        'H9a1.6 1.6 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.6 1.6 0 0 0 1 1.5 1.6 1.6 0 0 0 1.8-.3l.1-.1'
        'a2 2 0 1 1 2.8 2.8l-.1.1a1.6 1.6 0 0 0-.3 1.8V9a1.6 1.6 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1'
        'a1.6 1.6 0 0 0-1.5 1Z"/>'
    ),
    "refresh": (
        '<path d="M20.5 12a8.5 8.5 0 1 1-2.6-6.1"/>'
        '<path d="M20.8 4.2v5.2h-5.2"/>'
    ),
    "plus": '<path d="M12 5v14M5 12h14"/>',
    "key": (
        '<circle cx="8" cy="15.5" r="4.2"/>'
        '<path d="m11.2 12.3 8-8M16.6 6.9l2.2 2.2M18.8 4.7l2.2 2.2"/>'
    ),
    "bolt": '<path d="M13.2 2.5 4.5 13.4h6.1l-1.1 8.1 8.9-11H12l1.2-8Z"/>',
    "clock": '<circle cx="12" cy="12" r="8.6"/><path d="M12 7.4V12l3.2 2"/>',
    "trash": (
        '<path d="M4 7h16M9.5 7V4.8h5V7M6.2 7l1 13.2h9.6L17.8 7"/>'
        '<path d="M10.2 10.6v6.2M13.8 10.6v6.2"/>'
    ),
    "archive": (
        '<rect x="3" y="4" width="18" height="4.6" rx="1.6"/>'
        '<path d="M4.6 8.6V19a1.6 1.6 0 0 0 1.6 1.6h11.6A1.6 1.6 0 0 0 19.4 19V8.6"/>'
        '<path d="M9.4 12.6h5.2"/>'
    ),
    "folder": (
        '<path d="M3.2 7.4A2 2 0 0 1 5.2 5.4h4l2 2.4h7.6a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5.2'
        'a2 2 0 0 1-2-2V7.4Z"/>'
    ),
    "external": (
        '<path d="M14 4h6v6"/><path d="M20 4 11 13"/>'
        '<path d="M18.5 14.5V19a1.5 1.5 0 0 1-1.5 1.5H5.5A1.5 1.5 0 0 1 4 19V7.5A1.5 1.5 0 0 1 5.5 6H10"/>'
    ),
    "check": '<path d="m5 12.5 4.6 4.6L19 6.8"/>',
    "chevron-up": '<path d="m6.5 14.5 5.5-5.5 5.5 5.5"/>',
    "chevron-down": '<path d="m6.5 9.5 5.5 5.5 5.5-5.5"/>',
    "alert": '<path d="M12 3.6 22 20H2L12 3.6Z"/><path d="M12 9.6v5M12 17.4v.2"/>',
    "play": '<path d="M7 4.6 19 12 7 19.4V4.6Z"/>',
    "sync": (
        '<path d="M4 9.5A8 8 0 0 1 18.3 6.4L21 9"/>'
        '<path d="M21 4v5h-5"/>'
        '<path d="M20 14.5A8 8 0 0 1 5.7 17.6L3 15"/>'
        '<path d="M3 20v-5h5"/>'
    ),
    "user": (
        '<circle cx="12" cy="8" r="3.8"/>'
        '<path d="M4.6 20.2c0-3.6 3.3-5.8 7.4-5.8s7.4 2.2 7.4 5.8"/>'
    ),
}


@lru_cache(maxsize=256)
def _pixmap(name: str, color: str, size: int) -> QPixmap:
    body = _PATHS.get(name, _PATHS["overview"])
    svg = (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" '
        f'viewBox="0 0 24 24" fill="none" stroke="{color}" stroke-width="1.7" '
        f'stroke-linecap="round" stroke-linejoin="round">{body}</svg>'
    )
    pixmap = QPixmap(size, size)
    pixmap.fill(Qt.GlobalColor.transparent)
    renderer = QSvgRenderer(QByteArray(svg.encode("utf-8")))
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing, True)
    renderer.render(painter, QRectF(0, 0, size, size))
    painter.end()
    return pixmap


def icon(name: str, color: str | None = None, size: int = 18) -> QIcon:
    return QIcon(_pixmap(name, color or PALETTE["text_2"], size))


def pixmap(name: str, color: str | None = None, size: int = 18) -> QPixmap:
    return _pixmap(name, color or PALETTE["text_2"], size)


def nav_icons(name: str) -> tuple[QIcon, QIcon]:
    """(常态, 选中态)"""
    return (icon(name, PALETTE["muted"]), icon(name, PALETTE["accent"]))
