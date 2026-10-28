# -*- coding: utf-8 -*-
"""展示用格式化：体积、时间、相对时间。"""
from __future__ import annotations

from datetime import datetime


def fmt_size(n) -> str:
    try:
        value = float(n or 0)
    except (TypeError, ValueError):
        return "—"
    if value <= 0:
        return "—"
    for unit in ("B", "KB", "MB", "GB"):
        if value < 1024 or unit == "GB":
            return f"{value:.0f} {unit}" if unit == "B" else f"{value:.1f} {unit}"
        value /= 1024
    return f"{value:.1f} GB"


def fmt_time(ms) -> str:
    if not ms:
        return "—"
    try:
        return datetime.fromtimestamp(int(ms) / 1000).strftime("%Y-%m-%d %H:%M")
    except (ValueError, OSError, OverflowError):
        return "—"


def fmt_clock(ts: datetime | None = None) -> str:
    return (ts or datetime.now()).strftime("%H:%M:%S")


def clip(text, width: int) -> str:
    text = str(text or "").replace("\n", " ")
    return text if len(text) <= width else text[: width - 1] + "…"
