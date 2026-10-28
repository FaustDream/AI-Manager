# -*- coding: utf-8 -*-
"""主窗口：左侧导航 + 页面栈。"""
from __future__ import annotations

from PySide6.QtCore import QSize, Qt, QTimer
from PySide6.QtGui import QGuiApplication, QIcon
from PySide6.QtWidgets import (QFrame, QHBoxLayout, QLabel, QMainWindow, QMessageBox,
                               QPushButton, QStackedWidget, QVBoxLayout, QWidget)

from .. import APP_TITLE, APP_VERSION
from ..core import bus, logbook, paths, tasks
from . import icons
from .pages.account_center import AccountCenterPage
from .pages.maintenance import MaintenancePage
from .pages.overview import OverviewPage
from .pages.sessions import SessionsPage
from .pages.settings import SettingsPage
from .theme import PALETTE
from .widgets import Toast

NAV_ITEMS = [
    ("overview", "总览", "overview"),
    ("account_center", "账号中心", "accounts"),
    ("sessions", "会话管理", "sessions"),
    ("maintenance", "备份与清理", "maintenance"),
]

PAGES = {
    "overview": OverviewPage,
    "account_center": AccountCenterPage,
    "sessions": SessionsPage,
    "maintenance": MaintenancePage,
    "settings": SettingsPage,
}

# 旧入口兼容：原「签到中心 / 账号列表 / 账号档案」三个页面已合成账号中心的一张账号表
LEGACY_TABS = {"checkin": "checkin", "accounts": "accounts", "profiles": "profiles"}

# 每日签到轮询：启动后先跑一轮，之后每 6 小时一轮。
# 启动后等各页面把首屏扫描跑完再开始；忙就 5 秒后再看，最多等半小时。
DAILY_INTERVAL_MS = 6 * 60 * 60 * 1000
DAILY_START_DELAY_MS = 12_000
DAILY_RETRY_MS = 5_000
DAILY_MAX_RETRY = 360


class NavButton(QPushButton):
    def __init__(self, key: str, text: str, icon_name: str, parent=None):
        super().__init__(text, parent)
        self.key = key
        self.setObjectName("NavItem")
        self.setCheckable(True)
        self.setCursor(Qt.CursorShape.PointingHandCursor)
        self.setIconSize(QSize(18, 18))
        self._normal, self._active = icons.nav_icons(icon_name)
        self.setIcon(self._normal)
        self.toggled.connect(self._sync_icon)

    def _sync_icon(self, checked: bool) -> None:
        self.setIcon(self._active if checked else self._normal)


class MainWindow(QMainWindow):
    def __init__(self, cfg: dict):
        super().__init__()
        self.cfg = cfg
        self.pages: dict[str, QWidget] = {}
        self.nav_buttons: dict[str, NavButton] = {}
        self._summary: dict[str, str] = {}
        self._current = ""
        self._pages_built: list[str] = []

        self.setObjectName("MainWindow")
        self.setWindowTitle(APP_TITLE)
        if paths.ICON_FILE.exists():
            self.setWindowIcon(QIcon(str(paths.ICON_FILE)))
        self.setMinimumSize(1100, 690)
        self._restore_geometry()

        self._build()
        self.toast = Toast(self)

        # 日志落盘用缓冲：定时 flush，避免每行一次文件 IO
        self._log_flush = QTimer(self)
        self._log_flush.setInterval(600)
        self._log_flush.timeout.connect(logbook.flush)
        self._log_flush.start()

        bus.BUS.log.connect(self.log)
        bus.BUS.toast.connect(self.notify)
        bus.BUS.busy.connect(self.set_busy)
        bus.BUS.data_changed.connect(self._on_data_changed)
        bus.BUS.credits.connect(self._on_credits)

        # 积分主动轮询：到点在后台查一遍各账号积分，广播给各页面静默更新
        self._credit_timer = QTimer(self)
        self._credit_timer.timeout.connect(self._poll_credits)
        self._apply_credit_interval()
        self._credit_polling = False

        # 每日签到轮询：启动先跑一轮，之后每 6 小时一轮，与界面上的页面切换无关
        self._daily_retry = 0
        self._daily_startup = True
        self._daily_timer = QTimer(self)
        self._daily_timer.setInterval(DAILY_INTERVAL_MS)
        self._daily_timer.timeout.connect(self._daily_poll)
        self._daily_timer.start()
        QTimer.singleShot(DAILY_START_DELAY_MS, self._daily_poll)

        # 每次启动都从总览进入：上次停在设置页不该成为下次的默认位置
        self.navigate("overview")
        QTimer.singleShot(300, lambda: self.log("AI Manager 已启动", "dim"))

    # ------------------------------------------------------------------ 构建
    def _restore_geometry(self) -> None:
        window = self.cfg.get("window") or {}
        width = int(window.get("w") or 1240)
        height = int(window.get("h") or 800)
        self.resize(width, height)
        x, y = window.get("x"), window.get("y")
        if isinstance(x, int) and isinstance(y, int):
            screen = QGuiApplication.primaryScreen()
            if screen is not None:
                area = screen.availableGeometry()
                if area.contains(x + 80, y + 40):
                    self.move(x, y)
                    return
        screen = QGuiApplication.primaryScreen()
        if screen is not None:
            area = screen.availableGeometry()
            self.move(area.center().x() - width // 2,
                      max(area.top() + 20, area.center().y() - height // 2))
        if window.get("max"):
            self.showMaximized()

    def _build(self) -> None:
        root = QWidget()
        root.setObjectName("Root")
        self.setCentralWidget(root)
        layout = QHBoxLayout(root)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(0)
        layout.addWidget(self._build_sidebar())

        content = QVBoxLayout()
        content.setContentsMargins(0, 0, 0, 0)
        content.setSpacing(0)

        self.stack = QStackedWidget()
        content.addWidget(self.stack, 1)

        # 页面改为首次进入时再构建（见 _ensure_page）：
        # 七个页面全量构造会在启动时同步扫描磁盘，白白卡住三秒以上。

        holder = QWidget()
        holder.setLayout(content)
        layout.addWidget(holder, 1)

    def _build_sidebar(self) -> QWidget:
        sidebar = QFrame()
        sidebar.setObjectName("Sidebar")
        sidebar.setFixedWidth(216)
        layout = QVBoxLayout(sidebar)
        layout.setContentsMargins(12, 16, 12, 12)
        layout.setSpacing(4)

        brand = QHBoxLayout()
        brand.setSpacing(10)
        brand.setContentsMargins(6, 0, 4, 0)
        logo = QLabel()
        logo.setFixedSize(36, 36)
        if paths.ICON_FILE.exists():
            logo.setPixmap(QIcon(str(paths.ICON_FILE)).pixmap(36, 36))
        else:
            logo.setPixmap(icons.pixmap("bolt", PALETTE["accent"], 28))
        brand.addWidget(logo)
        # layout.addWidget(label("工作台", "NavGroup"))
        layout.addSpacing(4)
        for key, text, icon_name in NAV_ITEMS:
            button = NavButton(key, text, icon_name)
            button.clicked.connect(lambda _c=False, k=key: self.navigate(k))
            self.nav_buttons[key] = button
            layout.addWidget(button)

        layout.addStretch(1)
        settings = NavButton("settings", "设置", "settings")
        settings.clicked.connect(lambda _c=False: self.navigate("settings"))
        self.nav_buttons["settings"] = settings
        layout.addWidget(settings)


        return sidebar

    # ------------------------------------------------------------------ 导航
    def _ensure_page(self, key: str) -> QWidget | None:
        """按需构建页面：第一次切过去才创建，之后复用同一个实例。"""
        page = self.pages.get(key)
        if page is not None:
            return page
        factory = PAGES.get(key)
        if factory is None:
            return None
        page = factory(self)
        self.pages[key] = page
        self.stack.addWidget(page)
        self._pages_built.append(key)
        return page

    def navigate(self, key: str) -> None:
        tab = LEGACY_TABS.get(key)
        if tab:                      # 旧入口：跳到账号中心并切到对应 Tab
            key = "account_center"
        page = self._ensure_page(key)
        if page is None:                 # 页面名不存在时兜底到总览
            key = "overview"
            page = self._ensure_page(key)
            if page is None:
                return
        previous = self.stack.currentWidget()
        if previous is not None and previous is not page:
            leave = getattr(previous, "on_hide", None)
            if callable(leave):
                leave()
        self.stack.setCurrentWidget(page)
        self._current = key
        for nav_key, button in self.nav_buttons.items():
            button.blockSignals(True)
            button.setChecked(nav_key == key)
            button.blockSignals(False)
            button._sync_icon(nav_key == key)
        if tab and hasattr(page, "select_tab"):
            page.select_tab(tab)
        else:
            page.on_show()

    def page(self, key: str):
        if key in LEGACY_TABS:
            center = self.pages.get("account_center")
            return center.sub(key) if center is not None else None
        return self.pages.get(key)

    # ------------------------------------------------------------------ 状态
    # 底部状态栏已移除。三个方法保留是因为页面与 bus 里已有十几处调用，
    # 逐个删改的改动面远大于收益；空实现即可，调用方不用改。
    def set_status(self, text: str) -> None:
        """状态栏已移除，进度文案不再有展示位。"""

    def set_busy(self, busy: bool, text: str = "") -> None:
        """状态栏已移除，忙闲反馈由各页面自己的按钮与提示承担。"""

    def set_summary(self, scope: str, text: str) -> None:
        """统计文案仍缓存，页面切换时如需恢复展示可直接取用。"""
        self._summary[scope] = text

    # ---------------------------------------------------------- 每日签到轮询
    def _daily_poll(self) -> None:
        """日期轮询入口：启动一次，之后每 6 小时一次。

        先等界面空闲：启动瞬间页面正在扫描，两边同时打同一批接口既慢，
        也容易让状态显示错乱。忙就延后重试，别把这一天的签到让过去。
        """
        if self._credit_polling or any(getattr(page, "busy", False)
                                       for page in self.pages.values()):
            self._daily_defer()
            return
        page = self._ensure_page("account_center")
        runner = getattr(page, "run_daily_round", None)
        if not callable(runner) or not runner(startup=self._daily_startup):
            self._daily_defer()
            return
        self._daily_startup = False
        self._daily_retry = 0

    def _daily_defer(self) -> None:
        if self._daily_retry >= DAILY_MAX_RETRY:
            self.log("每日轮询等待界面空闲超时，跳过本轮", "warn")
            return
        self._daily_retry += 1
        QTimer.singleShot(DAILY_RETRY_MS, self._daily_poll)

    # ------------------------------------------------------------------ 日志
    def log(self, message: str, level: str = "info") -> None:
        """界面不再展示日志，只按级别落盘到 data/logs/app.log。"""
        logbook.append(message)

    def notify(self, message: str, level: str = "info") -> None:
        self.toast.show_message(message, level)

    # ------------------------------------------------------------------ 对话框
    def confirm(self, title: str, text: str) -> bool:
        box = QMessageBox(self)
        box.setWindowTitle(title)
        box.setIcon(QMessageBox.Icon.Question)
        box.setText(text)
        yes = box.addButton("继续", QMessageBox.ButtonRole.AcceptRole)
        box.addButton("取消", QMessageBox.ButtonRole.RejectRole)
        box.setDefaultButton(yes)
        box.exec()
        return box.clickedButton() is yes

    def info(self, title: str, text: str) -> None:
        QMessageBox.information(self, title, text)

    def warn(self, title: str, text: str) -> None:
        QMessageBox.warning(self, title, text)

    def error(self, title: str, text: str) -> None:
        QMessageBox.critical(self, title, text)

    def clipboard(self):
        return QGuiApplication.clipboard()

    # ------------------------------------------------------------------ 生命周期
    def _on_data_changed(self, scope: str) -> None:
        for page in self.pages.values():
            try:
                page.on_data_changed(scope)
            except Exception as exc:  # 单个页面出错不能影响其他页面
                self.log(f"页面刷新失败：{exc}", "error")

    # ---------------------------------------------------------- 积分轮询
    def apply_credit_interval(self) -> None:
        """设置页改了轮询间隔后立即生效。"""
        self._apply_credit_interval()

    def _apply_credit_interval(self) -> None:
        minutes = 5 if "credit_interval" not in self.cfg \
            else int(self.cfg.get("credit_interval") or 0)
        if minutes <= 0:
            self._credit_timer.stop()
            return
        self._credit_timer.start(minutes * 60 * 1000)

    def _poll_credits(self) -> None:
        """到点后台查一遍积分，查完广播给各页面静默更新。

        与手动操作共用后台线程池：有任务在跑就跳过这一轮，下一轮再补，
        避免和签到/扫描抢并发额度。
        """
        if self._credit_polling:
            return
        if any(getattr(page, "busy", False) or getattr(page, "daily_running", False)
               for page in self.pages.values()):
            return
        self._credit_polling = True

        def worker(_emit):
            from ..checkin import service as checkin_service
            accounts = checkin_service.collect(self.cfg, lambda m, lv="info": None)
            if not accounts:
                return {}
            return checkin_service.query_credits(accounts, lambda m: None)

        def done(results: dict):
            self._credit_polling = False
            if results:
                bus.BUS.credits.emit(results)
                self.log(f"积分自动轮询完成（{len(results)} 个账号）", "dim")

        def failed(message: str):
            self._credit_polling = False
            self.log(f"积分自动轮询失败：{message}", "dim")

        tasks.submit(worker, on_done=done, on_error=failed)

    def _on_credits(self, snapshots: dict) -> None:
        for page in self.pages.values():
            try:
                page.on_credits(snapshots)
            except Exception as exc:  # 单个页面出错不能影响其他页面
                self.log(f"积分更新失败：{exc}", "error")

    def save_cfg(self) -> None:
        from ..core import config as config_module
        config_module.save(self.cfg)

    def closeEvent(self, event) -> None:  # noqa: N802 - Qt 接口
        # 退出前让当前页同步落盘：异步任务还没跑完，进程就结束了
        current = self.stack.currentWidget()
        flush = getattr(current, "flush", None)
        if callable(flush):
            flush()
        geo = self.geometry()
        self.cfg["window"] = {
            "w": geo.width(), "h": geo.height(),
            "x": geo.x(), "y": geo.y(),
            "max": self.isMaximized(),
        }
        self.save_cfg()
        logbook.flush()          # 确保缓冲里的日志落盘
        super().closeEvent(event)
