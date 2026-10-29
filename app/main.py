# -*- coding: utf-8 -*-
"""应用入口：单实例、DPI、主题、数据迁移、主窗口。"""
from __future__ import annotations

import ctypes
import sys
import traceback

from . import APP_TITLE, APP_VERSION, brand
from .core import config, logbook, migrate, paths

MUTEX_NAME = "Local\\AIManager.Desktop.Singleton"


_MUTEX_HANDLE = None        # 进程生命周期内一直持有，不手工关闭


def _raise_existing_window() -> None:
    """把已运行实例的主窗口提到前台；找不到就静默返回。"""
    user32 = ctypes.windll.user32
    hwnd = user32.FindWindowW(None, APP_TITLE)
    if not hwnd:
        return
    user32.ShowWindow(hwnd, 9)              # SW_RESTORE
    user32.SetForegroundWindow(hwnd)


def _single_instance() -> bool:
    """已经开着一个窗口时不再开第二个，并把原窗口提到前面。

    Local\\ 命名空间的互斥体只在当前登录会话内唯一，符合桌面工具的场景。
    句柄持有到进程退出、由系统回收；不要手工 CloseHandle，否则第二个实例
    会误判成「没有别的实例在跑」。
    """
    global _MUTEX_HANDLE

    if sys.platform != "win32":
        return True

    import ctypes.wintypes as wt

    # 必须用 use_last_error=True：ctypes.windll 配 GetLastError() 属于另一次
    # API 调用，中间的 Python 代码可能覆盖线程错误码，拿到的未必是
    # CreateMutexW 的结果。
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.CreateMutexW.restype = wt.HANDLE
    kernel32.CreateMutexW.argtypes = (wt.LPVOID, wt.BOOL, wt.LPCWSTR)

    handle = kernel32.CreateMutexW(None, False, MUTEX_NAME)
    if not handle:
        # 创建失败（权限、句柄耗尽等）时不要拦住用户：宁可多开一个实例
        return True

    if ctypes.get_last_error() == 183:      # ERROR_ALREADY_EXISTS
        _raise_existing_window()
        return False

    _MUTEX_HANDLE = handle
    return True


def _set_taskbar_identity() -> None:
    try:
        ctypes.windll.shell32.SetCurrentProcessExplicitAppUserModelID(
            "AIManager.Desktop.1")
    except Exception:
        pass


def create_window(cfg: dict):
    """创建主窗口。

    热重载（dev.py）会反复调用本函数，因此这里不要做一次性副作用
    （迁移、单实例、QApplication 创建都在 main/main_dev 里完成）。
    """
    from PySide6.QtWidgets import QApplication

    from .ui import theme
    from .ui.mainwindow import MainWindow

    app = QApplication.instance()
    if app is not None:
        theme.apply(app)
    return MainWindow(cfg)


def main(argv: list[str] | None = None) -> int:
    argv = list(argv or sys.argv)
    paths.ensure_dirs()

    if "--selftest" in argv:
        from .checkin import service
        return service.selftest()

    from PySide6.QtCore import Qt
    from PySide6.QtGui import QGuiApplication, QIcon
    from PySide6.QtWidgets import QApplication

    _set_taskbar_identity()
    QGuiApplication.setHighDpiScaleFactorRoundingPolicy(
        Qt.HighDpiScaleFactorRoundingPolicy.PassThrough)
    app = QApplication(argv)
    app.setApplicationName("AI Manager")
    app.setApplicationDisplayName(APP_TITLE)
    app.setApplicationVersion(APP_VERSION)
    app.setQuitOnLastWindowClosed(True)
    if paths.ICON_FILE.exists():
        app.setWindowIcon(QIcon(str(paths.ICON_FILE)))

    if not _single_instance():
        return 0

    migrated = migrate.run_once()
    cfg = config.load()

    window = create_window(cfg)
    window.show()

    if migrated:
        window.log(f"已从旧版本迁移数据：{'、'.join(migrated)}", "ok")
    window.log(f"AI Manager {APP_VERSION} 启动完成 · by {brand.AUTHOR} · 数据目录 {paths.DATA_DIR}",
               "dim")

    # 署名被改过就明确报一次：改动者可以继续用，但没法悄悄当成自己的作品发布
    if not brand.intact():
        window.log(f"署名校验失败（{brand.signature()}），本副本非 {brand.AUTHOR} 官方发布。", "warn")
        window.warn(
            "非官方副本",
            f"本副本的作者署名已被修改，不是 {brand.AUTHOR} 发布的原版。\n\n"
            f"官方来源：{brand.HOMEPAGE}\n"
            f"授权：{brand.LICENSE_BRIEF}",
        )

    return app.exec()


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception:
        detail = traceback.format_exc()
        logbook.append(detail)
        try:
            from PySide6.QtWidgets import QApplication, QMessageBox
            app = QApplication.instance() or QApplication(sys.argv)
            QMessageBox.critical(None, "AI Manager 启动失败", detail[-1500:])
        except Exception:
            pass
        sys.exit(1)
