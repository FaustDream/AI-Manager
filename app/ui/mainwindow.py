# -*- coding: utf-8 -*-
"""主窗口：左侧导航 + 页面栈。"""
from __future__ import annotations

import time

from PySide6.QtCore import QSize, Qt, QTimer
from PySide6.QtGui import QGuiApplication, QIcon
from PySide6.QtWidgets import (QApplication, QFrame, QHBoxLayout, QLabel, QMainWindow,
                               QMessageBox, QPushButton, QStackedWidget, QVBoxLayout,
                               QWidget)

from .. import APP_TITLE, APP_VERSION, brand
from ..checkin import zclaim, zstore
from ..core import bus, logbook, paths, tasks
from . import icons, theme
from .pages.account_center import AccountCenterPage
from .pages.hub import HubPage
from .pages.logs import LogPage
from .pages.maintenance import MaintenancePage
from .pages.overview import OverviewPage
from .pages.sessions import SessionsPage
from .pages.settings import SettingsPage
from .pages.tasks import TaskPage
from .theme import PALETTE
from .widgets import BusyIndicator, Toast, label

NAV_ITEMS = [
    ("overview", "总览", "overview"),
    ("account_center", "账号中心", "accounts"),
    ("sessions", "会话管理", "sessions"),
    ("hub", "技能与 MCP", "hub"),
    ("maintenance", "备份与清理", "maintenance"),
    ("tasks", "任务", "clock"),
    ("logs", "日志", "log"),
]

PAGES = {
    "overview": OverviewPage,
    "account_center": AccountCenterPage,
    "sessions": SessionsPage,
    "hub": HubPage,
    "maintenance": MaintenancePage,
    "tasks": TaskPage,
    "logs": LogPage,
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

# 技能 / MCP 自动同步：启动后等首屏扫描跑完再动手，避免和签到轮询抢磁盘
HUB_START_DELAY_MS = 9_000

# ZCode 活动自动领取：跟页面无关地跑，15 分钟探一轮，单账号 30 分钟才探一次。
# 领取要过验证码（会拉起浏览器），所以默认关，且只在探到可领时才动作。
ZCODE_CLAIM_INTERVAL_MS = 15 * 60 * 1000
ZCODE_CLAIM_START_DELAY_MS = 45_000
ZCODE_PROBE_INTERVAL = 30 * 60


class NavButton(QPushButton):
    def __init__(self, key: str, text: str, icon_name: str, parent=None):
        super().__init__(text, parent)
        self.key = key
        self._icon_name = icon_name
        self.setObjectName("NavItem")
        self.setCheckable(True)
        self.setCursor(Qt.CursorShape.PointingHandCursor)
        self.setIconSize(QSize(18, 18))
        self._normal, self._active = icons.nav_icons(icon_name)
        self.setIcon(self._normal)
        self.toggled.connect(self._sync_icon)

    def _sync_icon(self, checked: bool) -> None:
        self.setIcon(self._active if checked else self._normal)

    def refresh_theme(self) -> None:
        """图标是按色号渲染成位图的，换肤后必须重新渲染一遍。"""
        self._normal, self._active = icons.nav_icons(self._icon_name)
        self._sync_icon(self.isChecked())


class MainWindow(QMainWindow):
    def __init__(self, cfg: dict):
        super().__init__()
        self.cfg = cfg
        self.pages: dict[str, QWidget] = {}
        self.nav_buttons: dict[str, NavButton] = {}
        self._summary: dict[str, str] = {}
        self._current = ""
        self._pages_built: list[str] = []
        # 正在加载的来源（页面 key / 后台任务名 → 文案）。按 key 记账而不是布尔量：
        # 几处同时加载时，先结束的那个不能把别人的转圈一起关掉。
        self._busy: dict[str, str] = {}

        self.setObjectName("MainWindow")
        self.setWindowTitle(APP_TITLE)
        if paths.ICON_FILE.exists():
            self.setWindowIcon(QIcon(str(paths.ICON_FILE)))
        self.setMinimumSize(1100, 690)
        self._restore_geometry()

        self._build()
        self._sync_theme_button()
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

        # 任务监控：侧栏「任务」入口带实时数量，不用点进去就知道有没有在跑
        tasks.changed.connect(self._refresh_task_badge)

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

        # 技能 / MCP：启动时把已登记但缺失或过期的条目补到各平台
        QTimer.singleShot(HUB_START_DELAY_MS, self._hub_auto_sync)

        # ZCode 活动自动领取：放在主窗口而不是账号中心页——页面是懒加载的，
        # 用户不打开那一页也得能自动领
        self._zcode_claiming = False
        self._zcode_retry = 0
        self._zcode_seen: dict[str, float] = {}
        self._zcode_timer = QTimer(self)
        self._zcode_timer.setInterval(ZCODE_CLAIM_INTERVAL_MS)
        self._zcode_timer.timeout.connect(self.zcode_auto_claim)
        self._zcode_timer.start()
        QTimer.singleShot(ZCODE_CLAIM_START_DELAY_MS, self.zcode_auto_claim)

        # 每次启动都从总览进入：上次停在设置页不该成为下次的默认位置
        self.navigate("overview")
        logbook.set_level(str(self.cfg.get("log_level") or "debug"))
        logbook.event("会话开始", module="主窗口", version=APP_VERSION,
                      mem=logbook.memory_mb())
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

        layout.addLayout(self._build_brand())
        layout.addSpacing(14)
        for key, text, icon_name in NAV_ITEMS:
            button = NavButton(key, text, icon_name)
            button.clicked.connect(lambda _c=False, k=key: self.navigate(k))
            self.nav_buttons[key] = button
            layout.addWidget(button)

        layout.addStretch(1)
        self.busy_box = BusyIndicator(sidebar)
        layout.addWidget(self.busy_box)
        settings = NavButton("settings", "设置", "settings")
        settings.clicked.connect(lambda _c=False: self.navigate("settings"))
        self.nav_buttons["settings"] = settings
        layout.addWidget(settings)

        # 版权行贴在侧栏最底部：跟着窗口一直在，比「关于」卡片更难被顺手删掉
        foot = label(f"{brand.COPYRIGHT} · v{APP_VERSION}", "BrandFoot")
        foot.setAlignment(Qt.AlignmentFlag.AlignHCenter)
        layout.addWidget(foot)
        return sidebar

    def _refresh_task_badge(self) -> None:
        """侧栏「任务」入口上的实时数量：执行中 + 排队中的总数。"""
        button = self.nav_buttons.get("tasks")
        if button is None:
            return
        active = tasks.active_count()
        button.setText(f"任务 · {active}" if active else "任务")

    def _build_brand(self) -> QHBoxLayout:
        """侧栏顶部的品牌区：图标 + 产品名 + 作者署名。

        署名放在侧栏而不是窗口标题：标题会被窗口管理器截断，
        侧栏顶部每次都在，是改名再发布时最容易漏删的位置。
        """
        row = QHBoxLayout()
        row.setSpacing(10)
        row.setContentsMargins(6, 0, 4, 0)

        logo = QLabel()
        logo.setFixedSize(36, 36)
        if paths.ICON_FILE.exists():
            logo.setPixmap(QIcon(str(paths.ICON_FILE)).pixmap(36, 36))
        else:
            logo.setPixmap(icons.pixmap("bolt", PALETTE["accent"], 28))
        row.addWidget(logo)

        text = QVBoxLayout()
        text.setSpacing(1)
        text.setContentsMargins(0, 0, 0, 0)
        text.addWidget(label("AI Manager", "BrandName"))
        self.brand_by = label(brand.byline(), "BrandSub")
        if not brand.intact():
            self.brand_by.setText(brand.tampered_hint())
            self.brand_by.setObjectName("BrandByWarn")
        text.addWidget(self.brand_by)
        row.addLayout(text)
        row.addStretch(1)

        # 主题开关贴在品牌区右上角：任何页面都看得见，不用翻到设置里找
        self.theme_button = QPushButton()
        self.theme_button.setObjectName("IconButton")
        self.theme_button.setFixedSize(28, 28)
        self.theme_button.setCursor(Qt.CursorShape.PointingHandCursor)
        self.theme_button.clicked.connect(self.toggle_theme)
        row.addWidget(self.theme_button)
        return row

    # ------------------------------------------------------------------ 外观
    def toggle_theme(self) -> None:
        self.set_theme("light" if theme.mode() == "dark" else "dark")

    def set_theme(self, mode: str) -> None:
        """切浅色 / 深色：重刷整个界面，并记住这次选择。"""
        app = QApplication.instance()
        theme.set_mode(mode)
        if app is not None:
            # 重设 QSS 与调色板，顺带重渲染缓存过色号的图标与富文本
            theme.apply(app)
        self.cfg["theme"] = theme.mode()
        self.save_cfg()
        self._sync_theme_button()
        self.log(f"外观已切换为{'深色' if theme.mode() == 'dark' else '浅色'}", "dim")

    def _sync_theme_button(self) -> None:
        dark = theme.mode() == "dark"
        name = "sun" if dark else "moon"
        # 记下 (图标名, 取色键)：下次换肤由 theme 重新渲染，名称也跟着反转
        self.theme_button.setProperty("themeIcon", (name, "text_2", 17))
        self.theme_button.setIcon(icons.icon(name, PALETTE["text_2"], 17))
        self.theme_button.setToolTip("切换到浅色界面" if dark else "切换到深色界面")


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
    # 底部状态栏已移除，进度文案改由侧栏底部的转圈承担（见 set_busy）。
    def set_status(self, text: str) -> None:
        """状态栏已移除，进度文案不再有展示位。"""

    def set_busy(self, busy: bool, text: str = "", key: str = "") -> None:
        """任一页面或后台任务在加载数据就转圈，全部结束才停。

        key 区分来源：页面用自身的 key，后台轮询用 daily / credits。
        同一 key 重复置忙只更新文案，不会叠出多个圈。
        """
        if busy:
            self._busy[key] = text or "正在加载…"
        else:
            self._busy.pop(key, None)
        if self._busy:
            self.busy_box.start(list(self._busy.values())[-1])
        else:
            self.busy_box.stop()

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

    # ------------------------------------------------------- ZCode 自动领取
    def zcode_auto_claim(self, force: bool = False) -> None:
        """探测 ZCode 可领活动，命中就领取（会拉起浏览器过验证码）。

        force=True 用于用户刚打开开关时立即探一次；平时按 15 分钟节奏跑。
        """
        if not self.cfg.get("zcode_auto_claim") or self._zcode_claiming:
            return
        if any(getattr(page, "busy", False) for page in self.pages.values()):
            # 界面正在扫描/签到，先让路：10 秒后再看，最多等 1 分钟
            if self._zcode_retry < 6:
                self._zcode_retry += 1
                QTimer.singleShot(10_000, self.zcode_auto_claim)
            return
        self._zcode_retry = 0
        now = time.time()
        targets = [account for account in zstore.profiles()
                   if force or now - self._zcode_seen.get(account.iid, 0) > ZCODE_PROBE_INTERVAL]
        if not targets:
            return
        self._zcode_claiming = True
        for account in targets:
            self._zcode_seen[account.iid] = now

        def worker(_emit):
            hits = []
            for account in targets:
                try:
                    plans = zclaim.claimable(zstore.credentials_of(account),
                                             zstore.config_of(account),
                                             zstore.device_of(account))
                except zclaim.ClaimError as exc:
                    self.log(f"[ZCode] {account.name} 活动探测失败：{exc}", "dim")
                    continue
                if plans:
                    hits.append((account, plans[0]))
            return hits

        def done(hits) -> None:
            if not hits:
                self._zcode_claiming = False
                self.log(f"[ZCode] 活动探测：{len(targets)} 个账号暂无可领活动", "dim")
                return
            account, plan = hits[0]
            name = plan.get("name") or plan["plan_id"]
            self.log(f"[ZCode] 发现可领活动：{account.name} · {name}", "ok")
            self.notify(f"ZCode 可领：{name}，正在领取…", "info")
            self._claim_zcode(account, plan)

        tasks.submit(worker, on_done=done, on_error=self._zcode_claim_failed)

    def _claim_zcode(self, account, plan: dict) -> None:
        def worker(_emit):
            return zclaim.claim(zstore.credentials_of(account), zstore.config_of(account),
                                plan["plan_id"], zstore.device_of(account), self.log)

        def done(result) -> None:
            self._zcode_claiming = False
            name = result.get("name") or plan.get("name") or plan["plan_id"]
            self.log(f"[ZCode] {account.name} 自动领取成功：{name}", "ok")
            self.notify(f"ZCode 已自动领取：{name}", "ok")
            bus.BUS.data_changed.emit("checkin")

        tasks.submit(worker, on_done=done, on_error=self._zcode_claim_failed)

    def _zcode_claim_failed(self, message: str) -> None:
        self._zcode_claiming = False
        self.log(f"[ZCode] 自动领取失败：{message.splitlines()[0]}", "error")
        self.notify(f"ZCode 自动领取失败：{message.splitlines()[0]}", "warn")

    # ---------------------------------------------------------- 技能 / MCP
    def _hub_auto_sync(self) -> None:
        """启动后把技能与 MCP 的变更补到各平台；没有登记内容时直接返回。"""
        from ..hub import service as hub_service

        def worker(_emit):
            return hub_service.auto_sync()

        def done(result: dict):
            if result.get("skipped"):
                return
            if result.get("changed"):
                self.log(f"技能 / MCP 自动同步：更新 {result['changed']} 项", "dim")
            for item in (result.get("conflicts") or [])[:3]:
                self.log(f"技能 / MCP 自动同步跳过：{item}", "warn")
            for item in (result.get("errors") or [])[:3]:
                self.log(f"技能 / MCP 自动同步失败：{item}", "warn")

        def failed(message: str):
            self.log(f"技能 / MCP 自动同步失败：{message}", "warn")

        tasks.submit(worker, on_done=done, on_error=failed, serial=True )

    # ------------------------------------------------------------------ 日志
    def log(self, message: str, level: str = "info") -> None:
        """界面不再展示日志，按级别落盘到 data/logs/app.log + app.jsonl。"""
        logbook.append(message, level)

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
        self.set_busy(True, "正在刷新积分…", "credits")

        def worker(_emit):
            from ..checkin import service as checkin_service
            accounts = checkin_service.collect(self.cfg, lambda m, lv="info": None)
            if not accounts:
                return {}
            return checkin_service.query_credits(accounts, lambda m: None)

        def done(results: dict):
            self._credit_polling = False
            self.set_busy(False, "", "credits")
            if results:
                bus.BUS.credits.emit(results)
                self.log(f"积分自动轮询完成（{len(results)} 个账号）", "dim")

        def failed(message: str):
            self._credit_polling = False
            self.set_busy(False, "", "credits")
            self.log(f"积分自动轮询失败：{message}", "dim")

        tasks.submit(worker, on_done=done, on_error=failed, serial=True )

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
        logbook.event("会话结束", module="主窗口")
        logbook.flush()          # 确保缓冲里的日志落盘
        super().closeEvent(event)
