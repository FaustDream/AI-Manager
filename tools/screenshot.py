# -*- coding: utf-8 -*-
"""开发用：离屏渲染各页面截图，检查布局。

用法：python tools/screenshot.py [输出目录]
"""
from __future__ import annotations

import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from PySide6.QtCore import QEventLoop, QTimer, Qt  # noqa: E402
from PySide6.QtWidgets import QApplication  # noqa: E402

from app.core import config, paths  # noqa: E402
from app.ui import theme  # noqa: E402
from app.ui.mainwindow import MainWindow  # noqa: E402

PAGES = ["overview", "checkin", "accounts", "sessions", "profiles", "maintenance", "settings"]


def wait(ms: int) -> None:
    loop = QEventLoop()
    QTimer.singleShot(ms, loop.quit)
    loop.exec()


def main() -> int:
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "shots"
    out.mkdir(parents=True, exist_ok=True)
    paths.ensure_dirs()

    app = QApplication(sys.argv)
    theme.apply(app)
    cfg = config.load()
    cfg["window"] = {}
    win = MainWindow(cfg)
    win.resize(1240, 800)
    # 用原生字体引擎渲染，但不真的把窗口显示到桌面
    win.setAttribute(Qt.WidgetAttribute.WA_DontShowOnScreen, True)
    win.show()
    wait(3500)

    for key in PAGES:
        win.navigate(key)
        wait(1600)
        image = win.grab()
        target = out / f"{key}.png"
        image.save(str(target))
        print(f"saved {target} {image.width()}x{image.height()}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
