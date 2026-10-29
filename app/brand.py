# -*- coding: utf-8 -*-
"""作者署名与授权信息，以及「署名被改掉」的自检。

作者名、来源地址、版权文案只在这里定义一次，界面各处都从这里取，
不在任何地方硬编码字符串，避免漏改。

_SIGNATURE 是作者名 + 来源 + 产品名拼接后的摘要。有人把作者换成自己的、
又没同步改签名，启动就会判定为非官方副本并在侧栏标红——这拦不住有耐心的改造，
但能挡住「下载下来改个名字就发出去」这种最常见的盗用。
"""
from __future__ import annotations

import hashlib

AUTHOR = "FaustDream"
HOMEPAGE = "https://github.com/FaustDream/AI-Manager"
COPYRIGHT = "© 2026 FaustDream"
LICENSE_NAME = "AI Manager 授权条款 1.0"

# 允许做的事和唯一不能做的事，一句话讲清，界面上直接展示
LICENSE_BRIEF = "可自由使用、复制与分发原版；不得去除或替换作者署名后再分发，不得商业转售。"

# 官方版本的签名，见 signature()
_SIGNATURE = "9c61e70b04d59070"
_PRODUCT = "AI Manager"


def signature() -> str:
    """按当前作者信息重算摘要，用于和 _SIGNATURE 比对。"""
    raw = f"{AUTHOR}|{HOMEPAGE}|{_PRODUCT}"
    return hashlib.sha256(raw.encode("utf-8")).hexdigest()[:16]


def intact() -> bool:
    """作者信息是否仍是官方版本。"""
    return signature() == _SIGNATURE


def byline() -> str:
    """侧栏标题下的一行署名。"""
    return f"by {AUTHOR}"


def tampered_hint() -> str:
    """非官方副本时替换署名显示，让改动一眼可见。"""
    return "署名已被修改 · 非官方副本"
