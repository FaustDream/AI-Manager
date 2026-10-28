# -*- coding: utf-8 -*-
"""开发期热重载入口：改 app/ 下任意 .py，自动重建窗口。

用法：python dev.py

为什么不用内置的 importlib.reload：
Qt 的 C++ 对象不会因为模块被 reload 而换成新类，所以必须「销毁窗口 →
清掉 app.* 模块缓存 → 重新 import → 重建窗口」这一整套才会生效。
目标文件监听用 QFileSystemWatcher（Qt 自带），不新增任何第三方依赖。
"""
from __future__ import annotations

import importlib
import os
import sys
import traceback

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from PySide6.QtCore import QFileSystemWatcher, QTimer
from PySide6.QtWidgets import QApplication

ROOT = os.path.dirname(os.path.abspath(__file__))
APP_DIR = os.path.join(ROOT, "app")

DEBOUNCE_MS = 300        # 编辑器保存常触发多次事件，合并成一次重建
RETRY_MS = 1200          # 代码暂时写坏时，隔多久重试一次


def _all_dirs(path: str) -> list[str]:
    """列出 path 及其下所有子目录。QFileSystemWatcher 不递归，只能逐个加。

    必须排除 __pycache__：重建会重新 import，Python 写 .pyc 会让该目录变化，
    监听到就会再触发一次重建，形成死循环。
    """
    dirs = [path]
    for base, subs, _files in os.walk(path):
        for name in subs:
            if name == "__pycache__":
                continue
            dirs.append(os.path.join(base, name))
    return dirs


class HotReloader:
    def __init__(self, app: QApplication, cfg: dict) -> None:
        self.app = app
        self.cfg = cfg
        self.window = None
        self._building = False

        self._timer = QTimer()
        self._timer.setSingleShot(True)
        self._timer.setInterval(DEBOUNCE_MS)
        self._timer.timeout.connect(self._rebuild)

        # HotReloader 不是 QObject，不能当 parent 传进去
        self.watcher = QFileSystemWatcher()
        self._resync()

    # ------------------------------------------------------------------ 监听
    def _resync(self) -> None:
        """把新出现的子目录补进监听列表（例如新建了模块包）。"""
        known = set(self.watcher.directories()) | set(self.watcher.files())
        for path in _all_dirs(APP_DIR):
            if path not in known:
                self.watcher.addPath(path)

    def _on_change(self, _path: str) -> None:
        if not self._building:
            self._timer.start()

    # ------------------------------------------------------------------ 重建
    def build(self) -> None:
        from app.main import create_window

        self.window = create_window(self.cfg)
        self.window.show()

    def _rebuild(self) -> None:
        # 关掉「最后一个窗口关闭即退出」，否则重建过程中会直接退出进程
        self.app.setQuitOnLastWindowClosed(False)
        self._building = True
        try:
            if self.window is not None:
                self.window.close()
                self.window.deleteLater()
                self.window = None
                self.app.processEvents()

            # 丢掉 app 包及其子模块，下次 import 才会读到磁盘上的新代码
            for name in [n for n in sys.modules
                         if n == "app" or n.startswith("app.")]:
                del sys.modules[name]
            importlib.invalidate_caches()

            try:
                self.build()
                print("[hot] 已重建窗口")
            except Exception:
                traceback.print_exc()
                print("[hot] 重建失败，修好文件保存后会自动重试")
                QTimer.singleShot(RETRY_MS, self._rebuild)
        finally:
            # 只有窗口真正存在时才恢复，否则 Qt 会立刻退出
            if self.window is not None:
                self.app.setQuitOnLastWindowClosed(True)
            self._building = False
            self._resync()

    # ------------------------------------------------------------------ 启动
    def install(self) -> None:
        self.watcher.directoryChanged.connect(self._on_change)
        self.watcher.fileChanged.connect(self._on_change)
        self.build()


def main() -> int:
    from PySide6.QtCore import Qt
    from PySide6.QtGui import QGuiApplication, QIcon

    from app import APP_TITLE, APP_VERSION
    from app.core import config, paths
    from app.main import _set_taskbar_identity, _single_instance
    from app.ui import theme

    paths.ensure_dirs()
    _set_taskbar_identity()
    QGuiApplication.setHighDpiScaleFactorRoundingPolicy(
        Qt.HighDpiScaleFactorRoundingPolicy.PassThrough)

    app = QApplication(sys.argv)
    app.setApplicationName("AI Manager (dev)")
    app.setApplicationDisplayName(APP_TITLE)
    app.setApplicationVersion(APP_VERSION)
    app.setQuitOnLastWindowClosed(True)
    if paths.ICON_FILE.exists():
        app.setWindowIcon(QIcon(str(paths.ICON_FILE)))

    # 和正式版共用同一份 data/，双开会互相覆盖配置，所以仍然走单实例
    if not _single_instance():
        print("已有一个 AI Manager 实例在运行，请先关掉它再启动开发模式。")
        return 0

    theme.apply(app)
    # 必须保留引用：临时对象会被回收，监听器随之失效
    reloader = HotReloader(app, config.load())
    reloader.install()
    return app.exec()


if __name__ == "__main__":
    sys.exit(main())
