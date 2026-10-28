# -*- coding: utf-8 -*-
"""账号中心：签到、登录切换、账号档案统一到同一张账号表上完成。

原「签到中心 / 账号列表 / 账号档案」三个页签合并成一张以账号为主体的表：
每行一个账号，行内直接签到、切换客户端登录、存档案、切档案，
不必再在三个页签之间对照同一个账号。
"""
from __future__ import annotations

import time
from datetime import datetime
from pathlib import Path

from PySide6.QtCore import QTimer
from PySide6.QtWidgets import (QApplication, QFrame, QHBoxLayout, QVBoxLayout,
                               QWidget)

from ...checkin import daily, ledger, service, store, switcher
from ...checkin.base import (Account, ActionResult, CreditSnapshot, StatusResult,
                             summarize_lots)
from ...checkin.registry import LABELS
from ...core import bus, tasks, winproc
from ...sessions import profiles
from ...sessions.editions import Edition
from .. import button_config
from ..dialogs import AddAccountDialog
from ..models import Column
from ..theme import PALETTE
from ..widgets import (Badge, Card, DataTable, EmptyState, hline, label,
                       make_button)
from .base import BasePage

# 全列左对齐：行内信息密度高，居中对齐会让长短不一的文本看起来参差
COLUMNS = [
    Column("platform", "平台", 150, "left", fixed=True),
    Column("name", "账号", 160, "left", stretch=True),
    Column("login", "登录态", 84, "left"),
    Column("current", "当前", 46, "left"),
    Column("today", "今日签到", 72, "left"),
    Column("balance", "剩余积分", 78, "left"),
    Column("expire", "最近到期", 108, "left"),
    Column("pack", "档案", 52, "left"),
    Column("action", "操作", 158, "left"),
]

# 明细列表最多展示几条：再多就要滚动，反而挡住下面的表格
DETAIL_ROWS = 5

SWITCHABLE = ("qoder", "qoder_intl", "workbuddy")
ACTION_COL = len(COLUMNS) - 1

# 一次整包拷贝动辄上百 MB，切页面太频繁时没必要每次都重存
AUTO_SAVE_INTERVAL = 300
EXIT_SAVE_INTERVAL = 60


def _error_text(balance, summary: str) -> str:
    """积分拿不到时，单元格里显示的简短原因。"""
    if balance is not None:
        return ""
    text = str(summary or "")
    if "失败" in text:
        return "读取失败"
    if "过期" in text:
        return "已过期"
    return ""


def _round_text(result) -> str:
    """把一轮轮询的结果翻成一句人话，作为启动/定时轮询的可见反馈。"""
    if result.success:
        text = f"每日签到成功：{result.success} 个账号"
        if result.failed:
            text += f"，{result.failed} 个失败"
        return text
    if result.failed:
        return f"每日签到失败：{result.failed} 个账号，详情见运行日志"
    if result.waiting:
        return f"每日签到：{result.waiting} 个账号等待窗口开放"
    if result.already:
        return f"每日签到：{result.already} 个账号本轮已领过"
    if not result.scanned:
        return "每日签到检查完成：没有发现账号"
    return "每日签到检查完成：今天都已签到，无需重复"


def _round_short(result) -> str:
    """工具栏上的一行短结果，成功后一直留在这里，不随气泡消失。"""
    if result.success:
        return f"成功 {result.success}"
    if result.failed:
        return f"失败 {result.failed}"
    if result.waiting:
        return f"等待 {result.waiting}"
    if result.already:
        return f"已领 {result.already}"
    return "已签"


def _short_time(value, prefix: str = "") -> str:
    """ISO 时间串转本地「月-日 时:分」；解析不了返回空串。"""
    if not value:
        return ""
    try:
        at = datetime.fromisoformat(str(value))
    except ValueError:
        return ""
    if at.tzinfo is not None:
        at = at.astimezone()
    return f"{prefix}{at:%m-%d %H:%M}"


class AccountCenterPage(BasePage):
    key = "account_center"
    title = "账号中心"

    def __init__(self, win):
        super().__init__(win)
        self.accounts: dict[str, Account] = {}
        self.live_lots: dict[str, list] = {}
        self.skip: set[str] = set(self.cfg.get("skip") or [])
        # 已签到集合与积分台账各读一次缓存起来：逐行读文件在账号多时是 O(n²) 磁盘读
        self._checked: set[str] = set()
        self._ledger: dict[str, tuple[str, float, int]] = {}
        self._balances: dict[str, float | None] = {}
        self._balance_error: dict[str, str] = {}
        self._expiry: dict[str, str] = {}
        self._needs: dict[str, bool] = {}
        self._action_buttons: list[tuple[QWidget, bool]] = []
        # 接口最近一次返回的签到状态：True 已签 / False 可签 / None 接口没给
        self._status_checked: dict[str, bool | None] = {}
        # 只记「哪个版本下有哪些 uid」，用来判断该账号能否免登录恢复
        self._packs: dict[str, set[str]] = {"cn": set(), "intl": set()}
        self._profile_gen = 0
        self._last_save = 0.0
        self._saving = False
        self._client_was_running = False
        # 积分明细面板：只在有选中行时显示；倒计时只刷文案，不重读文件
        self._detail_iid = ""
        self._detail_target: datetime | None = None

        # 列宽拖动结束后再落盘，拖动过程中每一像素都写配置会把磁盘打满
        self._width_timer = QTimer(self)
        self._width_timer.setSingleShot(True)
        self._width_timer.setInterval(600)
        self._width_timer.timeout.connect(self._save_columns)

        self._build()

        self._timer = QTimer(self)
        self._timer.setInterval(30_000)
        self._timer.timeout.connect(self._tick)
        self._timer.start()

        # 倒计时每秒走一格：只做时间减法，不碰磁盘/网络
        self._countdown = QTimer(self)
        self._countdown.setInterval(1000)
        self._countdown.timeout.connect(self._tick_detail)
        self._countdown.start()

    # ------------------------------------------------------------------ 界面
    def _build(self) -> None:
        # 页头只留标题：说明性副标题和按钮配置入口都已去掉
        self.header.subtitle.hide()

        self.btn_bar = button_config.ButtonBar(self.cfg)
        self._action_buttons = []

        # ---- 第一行：签到动作 ----
        toolbar = Card(padding=14, spacing=10)
        row = QHBoxLayout()
        row.setSpacing(8)
        checkin_buttons = self.btn_bar.build(row, "checkin", "toolbar", {
            "checkin.checkin_all": lambda: self.checkin(all_accounts=True),
            "checkin.query_status": lambda: self.query(selected_only=False),
            "checkin.refresh_credits": self.refresh_credits,
            "checkin.refresh_login": self.refresh,
            "checkin.add_account": self.add_account,
            "checkin.toggle_skip": self.toggle_skip,
            "checkin.open_client": self.open_client,
        }, ctx=self._btn_ctx)
        self.btn_all = checkin_buttons.get("checkin.checkin_all")
        row.addStretch(1)
        # 原来的「每日自动签到 + 时刻」换成固定节奏的轮询，这里留一句状态
        self.round_hint = label(
            f"启动一次 + 每 {daily.INTERVAL_HOURS} 小时", "SectionHint")
        self.round_hint.setToolTip(
            "每日轮询签到：确认进入新的一天后，先校验账号状态，再对可签的账号调用签到接口")
        row.addWidget(self.round_hint)
        toolbar.body.addLayout(row)

        self.body.addWidget(toolbar)

        # ---- 账号表 ----
        table_card = QFrame()
        table_card.setObjectName("Card")
        table_layout = QVBoxLayout(table_card)
        table_layout.setContentsMargins(0, 0, 0, 0)
        table_layout.setSpacing(0)

        head = QHBoxLayout()
        head.setContentsMargins(16, 12, 16, 10)
        head.setSpacing(8)
        head.addWidget(label("账号", "SectionTitle"))
        self.summary = label("正在扫描登录态…", "SectionHint")
        head.addWidget(self.summary)
        head.addStretch(1)
        account_buttons = self.btn_bar.build(head, "accounts", "header", {
            "accounts.add": self.add_account,
        }, ctx=self._btn_ctx)
        account_buttons.update(self.btn_bar.build(head, "accounts", "toolbar", {
            "accounts.open_client": self.open_client,
            "accounts.switch_login": lambda: self.switch_login(None),
            "accounts.remove": self.remove_account,
        }, ctx=self._btn_ctx))
        self.btn_bar.build(head, "profiles", "header", {
            "profiles.logout": self.logout,
        }, ctx=self._btn_ctx)
        self.btn_switch = account_buttons.get("accounts.switch_login")
        self.btn_remove = account_buttons.get("accounts.remove")
        table_layout.addLayout(head)
        table_layout.addWidget(hline())

        self.table = DataTable(COLUMNS, key="id", parent=self)
        saved = self.cfg.get("account_columns") or []
        if len(saved) != len(COLUMNS):
            saved = []                      # 列结构变了，旧宽度会错位，全部重来
        self.table.apply_column_widths(saved)
        self.table.row_moved.connect(self._move_row)
        self.table.horizontalHeader().sectionResized.connect(
            lambda *_: self._width_timer.start())
        self.table.doubleClicked.connect(
            lambda index: self._double_clicked(index.row()))
        self.table.selectionModel().selectionChanged.connect(
            lambda *_: self._on_selection())
        table_layout.addWidget(self.table, 1)

        self.empty = EmptyState("还没有发现任何账号",
                                "先打开对应客户端登录，或点右上角「添加账号」用浏览器授权")
        table_layout.addWidget(self.empty, 1)
        self.body.addWidget(table_card, 1)

        self.detail = self._build_detail()
        self.detail.setVisible(False)
        self.body.addWidget(self.detail)

    # -------------------------------------------------------------- 积分明细
    def _build_detail(self) -> QWidget:
        """选中账号时的积分明细：签到状态、下次签到倒计时、最近几笔积分。"""
        card = Card(padding=14, spacing=9)
        card.setMaximumHeight(216)

        top = QHBoxLayout()
        top.setSpacing(8)
        top.addWidget(label("积分记录", "SectionTitle"))
        self.detail_name = label("", "SectionHint")
        top.addWidget(self.detail_name)
        top.addStretch(1)
        self.detail_badge = Badge("—", "muted")
        top.addWidget(self.detail_badge)
        card.body.addLayout(top)

        line = QHBoxLayout()
        line.setSpacing(18)
        self.detail_checkin = label("", "StatLabel")
        self.detail_next = label("", "StatLabel")
        self.detail_total = label("", "StatLabel")
        for widget in (self.detail_checkin, self.detail_next, self.detail_total):
            line.addWidget(widget)
        line.addStretch(1)
        card.body.addLayout(line)

        card.body.addWidget(hline())

        self.detail_rows = QVBoxLayout()
        self.detail_rows.setSpacing(3)
        card.body.addLayout(self.detail_rows)
        card.body.addStretch(1)
        return card

    def _on_selection(self) -> None:
        self._sync_buttons()
        row = self.table.current_row()
        self._show_detail(row["id"] if row else "")

    def _show_detail(self, iid: str) -> None:
        account = self.accounts.get(iid) if iid else None
        if account is None:
            self._detail_iid = ""
            self._detail_target = None
            self.detail.setVisible(False)
            return
        self._detail_iid = iid
        # 下次签到时刻只在这里取一次（Qoder 取接口缓存，其余读 checked.json），
        # 倒计时每秒只做减法，不重复读盘/发请求
        self._detail_target = service.next_checkin_at(account)
        self.detail.setVisible(True)
        self._paint_detail(account)

    def _paint_detail(self, account: Account) -> None:
        iid = account.iid
        self.detail_name.setText(f"{account.platform_label} · {account.name}")

        requires = self._needs.get(iid, account.requires_checkin)
        at = service.checked_at(iid) if requires else None
        checked = self._status_checked.get(iid)
        api_window = service.next_from_api(account.platform)
        # 活动制平台的本地记录分不清今天/昨天两轮，不能当“今天已签”的兜底
        today_done = (not api_window
                      and bool(at and at.date() == datetime.now().date()))
        if not requires:
            self.detail_badge.set("免签到", "muted")
        elif checked is True or (checked is None and today_done):
            self.detail_badge.set("已签到", "ok")
        elif checked is False:
            self.detail_badge.set("未签到", "warn")
        else:
            self.detail_badge.set("状态未知", "muted")

        # 只认接口的平台空跑一次也算“签到”，本地时间会把没发放的调用记成签到；
        # 这类账号改看台账里真正到手的发放时间。
        records = ledger.records(iid) if (requires and api_window) else None
        if not requires:
            self.detail_checkin.setText("本次签到 免签到")
        elif records is not None:
            last = _short_time(records[0].get("granted")) if records else ""
            self.detail_checkin.setText(f"本次签到 {last}" if last else "本次签到 暂无记录")
        elif at:
            self.detail_checkin.setText(f"本次签到 {at:%m-%d %H:%M:%S}")
        else:
            self.detail_checkin.setText("本次签到 暂无记录")

        self._tick_detail()
        self._paint_records(iid, records)

    def _tick_detail(self) -> None:
        """每秒只改一行文案；无选中时不做事。

        文案要区分三种状态：接口给了下一轮时刻、接口说现在可签、接口什么都没给。
        后者不能写成可签，否则活动还没下发时用户会以为能点。
        """
        if not self._detail_iid:
            return
        checked = self._status_checked.get(self._detail_iid)
        target = self._detail_target
        if target is not None and target > datetime.now():
            # 今天这轮还没开（例如 10:00 前），先给时刻和倒计时，不写“现在可签”
            self.detail_next.setText(
                f"下次签到 约 {target:%m-%d %H:%M} · {service.countdown_text(target)}")
        elif checked is False:
            self.detail_next.setText("下次签到 现在可签")
        elif checked is True:
            self.detail_next.setText("下次签到 本轮已签，等下一轮下发")
        else:
            self.detail_next.setText("下次签到 —")

    def _paint_records(self, iid: str, records: list | None = None) -> None:
        while self.detail_rows.count():
            item = self.detail_rows.takeAt(0)
            widget = item.widget()
            if widget is not None:
                widget.setParent(None)
                widget.deleteLater()

        entries = []
        rows = ledger.records(iid) if records is None else records
        if rows:
            # 本工具亲眼记录的发放：有确切获得时间，信息最全
            for row in rows:
                entries.append((
                    float(row.get("amount") or 0),
                    "签到领取",
                    _short_time(row.get("granted")),
                    _short_time(row.get("expires"), "到期 "),
                ))
            self.detail_total.setText(f"今日累计 +{ledger.today_total(iid):g}")
        else:
            # live_credit 平台不写台账，只能拿到批次金额与到期日
            for lot in self.live_lots.get(iid) or []:
                expires = lot.expires_at.astimezone() if lot.expires_at else None
                entries.append((
                    float(lot.amount or 0),
                    lot.label or "平台批次",
                    "",
                    f"到期 {expires:%m-%d}" if expires else "无到期日",
                ))
            self.detail_total.setText("今日累计 平台未提供")

        for amount, source, when, note in entries[:DETAIL_ROWS]:
            holder = QWidget()
            line = QHBoxLayout(holder)
            line.setContentsMargins(0, 0, 0, 0)
            line.setSpacing(12)
            line.addWidget(label(f"+{amount:g}", "StatLabel"))
            line.addWidget(label(source, "SectionHint"))
            line.addStretch(1)
            if when:
                line.addWidget(label(when, "SectionHint"))
            if note:
                line.addWidget(label(note, "SectionHint"))
            self.detail_rows.addWidget(holder)

        rest = len(entries) - DETAIL_ROWS
        if rest > 0:
            self.detail_rows.addWidget(label(f"…还有 {rest} 条", "SectionHint"))
        if not entries:
            self.detail_rows.addWidget(
                label("暂无领取记录：签到成功后，这里会列出每笔的来源与到期", "SectionHint"))

    # ------------------------------------------------------------------ 数据
    def on_show(self) -> None:
        if not self.accounts:
            self.refresh()

    def on_hide(self) -> None:
        self._auto_save("切换页面")

    def on_data_changed(self, scope: str) -> None:
        if scope == "checkin" and not self.busy:
            self.refresh()

    def on_credits(self, snapshots: dict) -> None:
        """定时轮询回来的积分快照：静默更新单元格，不打扰用户操作。"""
        if not self.accounts:
            return
        updated = 0
        for iid, snapshot in snapshots.items():
            if iid in self.accounts:
                self._render_credit(iid, snapshot)
                updated += 1
        if updated:
            self.log(f"积分自动轮询更新 {updated} 个账号", "dim")

    # ------------------------------------------------------- 旧入口 / 按钮配置
    def sub(self, _key: str = ""):
        """旧代码按「子页」取实例，合并后就是本页。"""
        return self

    def select_tab(self, _key: str = "") -> None:
        """旧入口（签到/账号/档案）现在都落到同一张表，无需切换。"""

    def rebuild(self) -> bool:
        """按钮配置保存后重建界面，让可见性/排序立即生效。"""
        if self.busy:
            self.warn("按钮配置", "当前有任务正在执行，请等它结束后再保存配置。")
            return False
        while self.body.count():
            item = self.body.takeAt(0)
            widget = item.widget()
            if widget is not None:
                widget.setParent(None)
                widget.deleteLater()
        self._build()
        self.refresh()
        return True

    # ------------------------------------------------------------------ 扫描
    def refresh(self) -> None:
        if self.busy:
            self.notify("当前任务还没结束，请稍候", "warn")
            return
        self._set_busy(True, "正在扫描登录态…")
        self.log("开始扫描各客户端登录态", "dim")
        tasks.submit(self._scan_worker, on_done=self._scan_done,
                     on_error=self._fail, on_partial=self._apply)

    def _scan_worker(self, emit):
        accounts = service.collect(self.cfg, self.log)
        emit(("accounts", accounts))
        self.log(f"共发现 {len(accounts)} 个账号", "ok" if accounts else "error")

        skip = set(self.skip)

        def query(account: Account) -> None:
            kind, iid, result = service.query_one(account, account.iid in skip, self.log)
            emit((kind, (iid, result)))

        tasks.parallel_map(accounts, query)
        return len(accounts)

    def _scan_done(self, count: int) -> None:
        self._set_busy(False)
        self._update_summary()
        self._sync_buttons()
        self._reload_profiles()

    # ------------------------------------------------------------ 每日轮询
    def run_daily_round(self, startup: bool = False) -> bool:
        """主窗口按「启动一次 + 每 6 小时一次」的节奏调用，返回是否真的开跑。

        启动那一轮即使当天已经签过也会完整跑一遍：先查接口状态，再决定签谁，
        最后由 _daily_done 给出明确的结果提示，不让用户以为「什么都没发生」。

        与手动扫描共用 busy：轮询期间按钮与行内操作一起禁用，
        避免用户点「全部签到」和后台轮询同时打接口、重复写签到记录。
        """
        if self.busy:
            return False
        self._set_busy(True, "正在执行每日签到轮询…")
        if startup:
            self.log("启动轮询：立即执行一次签到检查", "dim")
        else:
            self.log("每日轮询：开始检测日期与账号状态", "dim")
        tasks.submit(self._daily_worker, on_done=self._daily_done,
                     on_error=self._fail, on_partial=self._apply)
        return True

    def _daily_worker(self, emit):
        # 日期检测 → 状态校验 → 签到，三步都在 daily.run_round 里按顺序完成
        return daily.run_round(self.cfg, self.log, emit)

    def _daily_done(self, result) -> None:
        self._set_busy(False)
        self._update_summary()
        if result is None:
            return
        text = _round_text(result)
        self.notify(text, "warn" if result.failed and not result.success else "ok")
        # 气泡会消失，工具栏这行一直留着，回头也能看到上次的结果
        self.round_hint.setText(f"{result.when:%H:%M} · {_round_short(result)}")
        self.round_hint.setToolTip(f"{text}（每 {daily.INTERVAL_HOURS} 小时轮询一次）")
        if result.success and not self.win.isActiveWindow():
            # 窗口在后台时闪任务栏，避免错过签到成功
            QApplication.alert(self.win)

    # ------------------------------------------------------------------ 档案
    def _reload_profiles(self) -> None:
        """只取每个版本的 uid 列表，用来标记哪些账号能免登录恢复。"""
        self._profile_gen += 1
        generation = self._profile_gen

        def worker(_emit):
            return {
                "cn": [pack["uid"] for pack in profiles.list_packs(Edition("cn"))],
                "intl": [pack["uid"] for pack in profiles.list_packs(Edition("intl"))],
            }

        tasks.submit(worker,
                     on_done=lambda data: self._apply_packs(generation, data),
                     on_error=lambda m: self.log(
                         f"读取账号档案失败：{m.splitlines()[0]}", "error"))

    def _apply_packs(self, generation: int, data: dict) -> None:
        if generation != self._profile_gen:      # 期间又发起了一次，结果作废
            return
        self._packs = {key: set(value) for key, value in data.items()}
        for account in self.accounts.values():
            self.table.model.update(
                account.iid, pack="有" if self._has_pack(account) else "—")
        self._install_actions()

    def _has_pack(self, account: Account | None) -> bool:
        if account is None or account.platform != "workbuddy":
            return False
        uid = str(account.meta.get("uid") or "")
        edition = "intl" if account.meta.get("edition") == "intl" else "cn"
        return bool(uid) and uid in self._packs.get(edition, set())

    # ------------------------------------------------------------------ 渲染
    def _apply(self, event) -> None:
        kind, payload = event
        if kind == "accounts":
            self._render_accounts(payload)
        elif kind == "status":
            self._render_status(*payload)
        elif kind == "credit":
            self._render_credit(*payload)
        elif kind == "checkin":
            self._render_checkin(*payload)

    def _refresh_caches(self) -> None:
        # 传账号进去：只认接口的平台（Qoder）不能用本地记录当“今天已签”的基线
        self._checked = service.checked_ids(list(self.accounts.values()))
        self._ledger = ledger.summaries()

    def _ordered(self, accounts: list[Account]) -> list[Account]:
        """按用户拖出来的顺序排；没排过的按 iid 稳定排在后面。"""
        order = self.cfg.get("account_order") or []
        rank = {iid: index for index, iid in enumerate(order)}
        return sorted(accounts, key=lambda a: (rank.get(a.iid, len(order)), a.iid))

    def _render_accounts(self, accounts: list[Account]) -> None:
        accounts = self._ordered(accounts)
        keep = self._detail_iid          # 表格 set_rows 会清掉选中，先记住
        self.accounts = {account.iid: account for account in accounts}
        self.live_lots = {}
        self._balances = {}
        self._balance_error = {}
        self._expiry = {}
        self._needs = {account.iid: account.requires_checkin for account in accounts}
        self._status_checked = {}
        self._refresh_caches()

        rows = []
        for account in accounts:
            iid = account.iid
            current = switcher.is_current(account)
            if not account.requires_checkin:
                today = "免签到"
            elif iid in self._checked:
                today = "已签到"
            else:
                today = "—"
            rows.append({
                "id": iid,
                "platform": account.platform_label,
                "name": account.name,
                "login": account.expires_short(),
                "current": "当前" if current else "",
                "today": today,
                "balance": "—",
                "expire": "—",
                "pack": "有" if self._has_pack(account) else "—",
                "action": "",
                "_fg_map": self._colors(iid, account),
                "_tip_map": {"name": f"来源：{account.source or '浏览器登录'}"},
            })
        self.table.set_rows(rows)
        self._install_actions()
        for account in accounts:
            self._update_credit_cells(account.iid)
        self._update_summary()
        has_rows = bool(rows)
        self.table.setVisible(has_rows)
        self.empty.setVisible(not has_rows)
        if has_rows and keep in self.accounts:
            self.table.select_key(keep)
            self._show_detail(keep)
        else:
            self._show_detail("")

    def _colors(self, iid: str, account: Account) -> dict:
        colors = {}
        if account.expired:
            colors["login"] = PALETTE["danger"]
        if iid in self._checked:
            colors["today"] = PALETTE["ok"]
        if not account.requires_checkin:
            colors["today"] = PALETTE["muted"]
        if switcher.is_current(account):
            colors["current"] = PALETTE["ok"]
        return colors

    def _render_status(self, iid: str, result: StatusResult) -> None:
        account = self.accounts.get(iid)
        if account is None:
            return
        self._status_checked[iid] = result.checked_in
        # 下次可签时刻以接口下发为准，Qoder 这类活动制平台本地推算不成立
        service.note_next_at(iid, result.next_at)
        if result.lots is None:
            self.live_lots.pop(iid, None)
        else:
            self.live_lots[iid] = result.lots
        self._balances[iid] = result.balance
        self._balance_error[iid] = _error_text(result.balance, result.summary)

        requires = self._needs.get(iid, True)
        if result.checked_in is True:
            self._checked.add(iid)
        elif result.checked_in is False:
            self._checked.discard(iid)
        if result.checked_in is not None:
            checked = result.checked_in
        elif service.next_from_api(account.platform):
            checked = None
        else:
            checked = iid in self._checked
        if not requires:
            today = "免签到"
        elif checked is True:
            today = "已签到"
        elif checked is False:
            today = "待签到"
        else:
            today = "已签到" if iid in self._checked else "—"
        colors = self._colors(iid, account)
        if requires and checked:
            colors["today"] = PALETTE["ok"]
        elif requires and checked is False:
            colors["today"] = PALETTE["warn"]

        self.table.model.update(
            iid, today=today,
            balance=self._balance_text(iid),
            _fg_map=colors,
            _tip_map={"balance": result.summary or ""})
        self._update_credit_cells(iid)
        self._update_summary()
        if iid == self._detail_iid:
            self._detail_target = service.next_checkin_at(account)
            self._paint_detail(account)

    def _render_credit(self, iid: str, snapshot: CreditSnapshot) -> None:
        if iid not in self.accounts:
            return
        if snapshot.lots is None:
            self.live_lots.pop(iid, None)
        else:
            self.live_lots[iid] = snapshot.lots
        self._balances[iid] = snapshot.balance
        self._balance_error[iid] = _error_text(snapshot.balance, snapshot.summary)
        changes = {
            "balance": self._balance_text(iid),
            "_tip_map": {"balance": snapshot.summary or ""},
        }
        if not self._needs.get(iid, True):
            changes["today"] = "免签到"
        self.table.model.update(iid, **changes)
        self._update_credit_cells(iid)
        self._update_summary()
        account = self.accounts.get(iid)
        if iid == self._detail_iid and account is not None:
            self._paint_detail(account)

    def _render_checkin(self, account: Account, result: ActionResult) -> None:
        iid = account.iid
        if iid not in self.accounts:
            return
        if result.ok and not result.already and account.requires_checkin:
            self._checked.add(iid)
            self._status_checked[iid] = True
            # 台账刚写过这一条，只重算这一个账号
            self._ledger[iid] = ledger.summary(iid)
        changes = {
            "balance": self._balance_text(iid),
            "_tip_map": {"balance": result.message},
        }
        if not account.requires_checkin:
            changes["today"] = "免签到"
        elif not result.already:
            colors = self._colors(iid, account)
            colors["today"] = PALETTE["ok"] if result.ok else PALETTE["danger"]
            changes["today"] = "已签到" if result.ok else "失败"
            changes["_fg_map"] = colors
        # already 时不改单元格：本轮早已领过，今天到底算不算已签由紧随其后的状态刷新决定
        self.table.model.update(iid, **changes)
        self._update_credit_cells(iid)
        self._update_summary()
        if iid == self._detail_iid:
            self._paint_detail(account)
        self.log(f"[{account.platform_label}] {account.name}：{result.message}",
                 "ok" if result.ok else "error")

    def _balance_text(self, iid: str) -> str:
        value = self._balances.get(iid)
        if value is None:
            return self._balance_error.get(iid) or "—"
        return f"{value:g}"

    def _update_credit_cells(self, iid: str) -> None:
        if iid not in self.accounts:
            return
        if iid in self.live_lots:
            merged = list(self.live_lots[iid])
            account = self.accounts.get(iid)
            if account is not None and account.platform.startswith("qoder"):
                # Qoder 的资源包接口不给逐批到期日，用本工具领取台账补上
                merged.extend(ledger.credit_lots(iid))
            expiry, soon, _total, nearest_amount = summarize_lots(
                merged, ledger.SOON_DAYS)
        else:
            expiry, soon, _count = self._ledger.get(iid, ("", 0.0, 0))
            nearest_amount = 0.0
        self._expiry[iid] = expiry
        balance = self._balances.get(iid) or 0
        if expiry:
            expire_text = f"{expiry} · {nearest_amount:g}" if nearest_amount else expiry
        else:
            expire_text = "未提供" if balance > 0 else "—"
        position = self.table.model.index_of(iid)
        existing = {}
        tips = {}
        if position >= 0:
            existing = dict(self.table.model.rows[position].get("_fg_map") or {})
            tips = dict(self.table.model.rows[position].get("_tip_map") or {})
        if soon:
            existing["expire"] = PALETTE["warn"]
        tips["expire"] = (
            f"{expiry} 到期 {nearest_amount:g} 积分；"
            f"{ledger.SOON_DAYS} 天内合计 {soon:g}"
            if expiry else "平台未返回积分批次到期时间")
        self.table.model.update(iid,
                                balance=self._balance_text(iid),
                                expire=expire_text,
                                _fg_map=existing,
                                _tip_map=tips)

    def _update_summary(self) -> None:
        total = len(self.accounts)
        if not total:
            self.summary.setText("未发现账号")
            return
        active = sum(1 for iid, account in self.accounts.items()
                     if account.requires_checkin and iid not in self.skip)
        checked = sum(1 for iid in self.accounts if iid in self._checked)
        balance = sum(value for value in self._balances.values() if value is not None)
        dated = [value for value in self._expiry.values() if value]
        nearest = min(dated) if dated else ""
        text = f"{total} 个账号 · 参与 {active} · 今日已签到 {checked}"
        if self._balances:
            text += f" · 剩余 {balance:g} 积分"
        if nearest:
            text += f" · 最近到期 {nearest}"
        self.summary.setText(text)

    # ------------------------------------------------------------ 行内操作
    def _install_actions(self) -> None:
        """每次重设行数据后重新挂操作按钮；表格 reset 会连同旧 widget 一起清掉。"""
        self._action_buttons = []
        model = self.table.model
        for position in range(model.rowCount()):
            row = model.row_dict(position)
            account = self.accounts.get(row.get("id")) if row else None
            index = model.index(position, ACTION_COL)
            if account is None:
                self.table.setIndexWidget(index, None)
                continue
            self.table.setIndexWidget(index, self._action_widget(account))

    def _action_widget(self, account: Account) -> QWidget:
        host = QWidget()
        layout = QHBoxLayout(host)
        layout.setContentsMargins(2, 0, 2, 0)
        layout.setSpacing(2)

        current = switcher.is_current(account)
        specs = (
            ("签到", account.requires_checkin,
             lambda: self.checkin_one(account.iid)),
            ("切换", account.platform in SWITCHABLE and not current,
             lambda: self.switch_login(account.iid)),
            ("恢复会话", self._has_pack(account) and current,
             lambda: self.restore_sessions(account.iid)),
        )
        for text, enabled, slot in specs:
            button = make_button(text, "link")
            button.setObjectName("RowAction")
            button.setFixedHeight(24)
            button.clicked.connect(slot)
            button.setEnabled(enabled and not self.busy)
            self._action_buttons.append((button, enabled))
            layout.addWidget(button)
        return host

    def _refresh_row_actions(self) -> None:
        for button, enabled in self._action_buttons:
            button.setEnabled(enabled and not self.busy)

    def _account_of(self, iid: str | None) -> Account | None:
        if iid:
            return self.accounts.get(iid)
        row = self.table.current_row()
        return self.accounts.get(row["id"]) if row else None

    @staticmethod
    def _edition_for(account: Account | None) -> Edition:
        if account is not None and account.meta.get("edition") == "intl":
            return Edition("intl")
        return Edition("cn")

    def _current_workbuddy(self) -> Account | None:
        for account in self.accounts.values():
            if account.platform == "workbuddy" and switcher.is_current(account):
                return account
        return None

    def _double_clicked(self, position: int) -> None:
        row = self.table.model.row_dict(position)
        if row:
            self.query_one(row["id"])

    # ------------------------------------------------------------ 表格布局
    def _move_row(self, source: int, target: int) -> None:
        """拖拽后重排行并把新顺序写进配置，下次打开保持一致。"""
        rows = self.table.model.rows
        if not (0 <= source < len(rows)) or not (0 <= target < len(rows)):
            return
        rows.insert(target, rows.pop(source))
        self.table.model.set_rows(rows)
        self._install_actions()
        self.cfg["account_order"] = [row["id"] for row in rows]
        self.win.save_cfg()

    def _save_columns(self) -> None:
        widths = self.table.column_widths()
        if self.cfg.get("account_columns") != widths:
            self.cfg["account_columns"] = widths
            self.win.save_cfg()

    # ------------------------------------------------------------------ 签到
    def _btn_ctx(self) -> dict:
        row = self.table.current_row() if getattr(self, "table", None) else None
        account = self.accounts.get(row["id"]) if row else None
        return {
            "rows": bool(self.accounts),
            "selected": row is not None,
            "checkin_account": bool(account and account.requires_checkin),
            "stored_account": bool(account and account.meta.get("stored")),
        }

    def _sync_buttons(self) -> None:
        self.btn_bar.sync({"busy": self.busy, **self._btn_ctx()})
        self._refresh_row_actions()

    def _set_busy(self, busy: bool, text: str = "") -> None:
        self.busy = busy
        self.win.set_busy(busy, text)
        self._sync_buttons()

    def query(self, selected_only: bool = False) -> None:
        if self.busy:
            return
        targets = list(self.accounts.values())
        if selected_only:
            row = self.table.current_row()
            if not row:
                self.info("查询状态", "请先在列表中选择一个账号。")
                return
            targets = [self.accounts[row["id"]]] if row["id"] in self.accounts else []
        if not targets:
            self.info("暂无账号", "没有可查询的账号，请先登录对应客户端。")
            return

        self._set_busy(True, "正在查询状态…")
        skip = set(self.skip)

        def worker(emit):
            def query(account: Account) -> None:
                kind, iid, result = service.query_one(account, account.iid in skip, self.log)
                emit((kind, (iid, result)))

            tasks.parallel_map(targets, query)
            return len(targets)

        tasks.submit(worker, on_done=lambda _n: self._set_busy(False),
                     on_error=self._fail, on_partial=self._apply)

    def query_one(self, iid: str) -> None:
        account = self.accounts.get(iid)
        if account is None or self.busy:
            return
        self._set_busy(True, f"正在查询 {account.name}…")
        skip = set(self.skip)

        def worker(emit):
            kind, key, result = service.query_one(account, account.iid in skip, self.log)
            emit((kind, (key, result)))
            return 1

        tasks.submit(worker, on_done=lambda _n: self._set_busy(False),
                     on_error=self._fail, on_partial=self._apply)

    def refresh_credits(self) -> None:
        """只刷新剩余积分与到期，不查签到状态——适合高频刷新。"""
        if self.busy:
            self.notify("当前任务还没结束，请稍候", "warn")
            return
        targets = list(self.accounts.values())
        if not targets:
            self.info("暂无账号", "没有可查询的账号，请先登录对应客户端。")
            return
        self._set_busy(True, "正在刷新积分…")

        def worker(emit):
            results = service.query_credits(targets, lambda m: self.log(m, "dim"))
            for iid, snapshot in results.items():
                emit(("credit", (iid, snapshot)))
            return len(results)

        self.log(f"开始刷新 {len(targets)} 个账号的积分", "dim")
        tasks.submit(worker, on_done=lambda n: self._credits_done(n, manual=True),
                     on_error=self._fail, on_partial=self._apply)

    def _credits_done(self, count: int, manual: bool = False) -> None:
        if manual:
            self._set_busy(False)
            self._update_summary()
            self.notify(f"积分已刷新（{count} 个账号）")

    def checkin(self, all_accounts: bool) -> None:
        if self.busy:
            return
        if not self.accounts:
            self.info("暂无账号", "没有可签到的账号，请先登录对应客户端。")
            return
        if all_accounts:
            targets = [a for a in self.accounts.values()
                       if a.requires_checkin and a.iid not in self.skip]
            skipped = sum(1 for a in self.accounts.values()
                          if not a.requires_checkin or a.iid in self.skip)
            if skipped:
                self.log(f"跳过 {skipped} 个免签到账号", "dim")
            if not targets:
                self.info("免签到", "所有账号都无需签到，没有需要签到的账号。")
                return
        else:
            account = self._account_of(None)
            if account is None:
                self.info("未选择", "请先在列表中选择一个账号。")
                return
            if not account.requires_checkin:
                self.info("无需签到", f"{account.platform_label} 不需要签到，只查询积分即可。")
                return
            targets = [account]

        self._start_checkin(targets)

    def _start_checkin(self, targets: list[Account]) -> None:
        if self.busy or not targets:
            return
        self._set_busy(True, "正在签到…")
        tasks.submit(lambda emit: self._checkin_worker(emit, targets),
                     on_done=self._checkin_done, on_error=self._fail,
                     on_partial=self._apply)

    def checkin_one(self, iid: str) -> None:
        account = self.accounts.get(iid)
        if account is None or self.busy:
            return
        if not account.requires_checkin:
            self.info("无需签到", f"{account.platform_label} 不需要签到，只查询积分即可。")
            return
        self._set_busy(True, f"正在为 {account.name} 签到…")
        tasks.submit(lambda emit: self._checkin_worker(emit, [account]),
                     on_done=self._checkin_done, on_error=self._fail,
                     on_partial=self._apply)

    def _checkin_worker(self, emit, targets: list[Account]):
        success = 0
        already = 0
        for account in targets:
            self.log(f"[{account.platform_label}] 正在为 {account.name} 签到…", "dim")
            result = service.checkin_one(account, self.log)
            if result.ok:
                # 「本轮早已领过」没有新发放，不能把这次调用写成本次签到时刻
                if result.already:
                    already += 1
                else:
                    service.mark_checked(account.iid)
                    service.record_grant(account, result)
                    success += 1
            emit(("checkin", (account, result)))
            if result.ok:
                fresh = service.refresh_status_after_checkin(account, self.log)
                if fresh is not None:
                    emit(("status", (account.iid, fresh)))
        note = f"签到结束：成功 {success}/{len(targets)}"
        if already:
            note += f"，本轮已领取 {already}"
        self.log(note, "ok" if success + already == len(targets) else "error")
        return (success, len(targets), already)

    def _checkin_done(self, payload) -> None:
        self._set_busy(False)
        self._update_summary()
        success, total, already = payload
        if already and not success:
            # 今天这轮还没开（或别处已领），不是失败，别报红色警告
            self.notify(f"{already} 个账号本轮已领取，暂无可签的窗口", "warn")
            return
        text = f"签到完成：成功 {success}/{total}"
        if already:
            text += f" · 本轮已领取 {already}"
        self.notify(text, "ok" if success + already == total else "warn")

    def _fail(self, message: str) -> None:
        self._set_busy(False)
        self.log(message, "error")
        self.notify("操作失败，详情见运行日志", "error")

    def toggle_skip(self) -> None:
        account = self._account_of(None)
        if account is None:
            self.info("免签到", "请先在列表中选择一个账号。")
            return
        iid = account.iid
        if not account.requires_checkin:
            self.info("无需签到", f"{account.platform_label} 本身没有签到玩法，"
                                   "只查询积分，不参与后台签到。")
            return
        resumed = False
        if iid in self.skip:
            self.skip.discard(iid)
            note = "已恢复签到"
            resumed = True
        else:
            self.skip.add(iid)
            note = "已设为免签到"
        self.cfg["skip"] = sorted(self.skip)
        self.win.save_cfg()
        colors = self._colors(iid, account)
        self.table.model.update(iid, today="—" if iid in self.skip else "已签到",
                                _fg_map=colors)
        self._update_summary()
        self.log(f"[{account.platform_label}] {account.name} {note}", "dim")
        if resumed:
            self.query_one(iid)

    # ------------------------------------------------------------ 账号动作
    def add_account(self) -> None:
        account = self._account_of(None)
        platform = "workbuddy"
        if account is not None:
            platform = account.platform
            if platform == "workbuddy" and account.meta.get("edition") == "intl":
                platform = "workbuddy_intl"
        if platform == "qoder_intl":
            self.info("添加账号",
                      "Qoder 国际版目前通过扫描本机客户端登录态接入，"
                      "请先打开 Qoder 客户端登录，再点「刷新登录态」。")
            return
        AddAccountDialog(self.win, platform).exec()

    def open_client(self) -> None:
        account = self._account_of(None)
        platform = account.platform if account else "workbuddy"
        exe = service.client_exe(platform, account)
        if not exe or not Path(exe).exists():
            self.warn("打开客户端", f"未找到 {LABELS.get(platform, platform)} 的可执行文件。")
            return
        if not winproc.start_exe(exe):
            self.warn("打开客户端", f"启动 {Path(exe).name} 失败，请手动打开。")
            return
        self.log(f"已启动 {Path(exe).name}，登录后点「刷新登录态」", "dim")

    def switch_login(self, iid: str | None = None) -> None:
        account = self._account_of(iid)
        if account is None:
            self.info("切换账号", "请先在列表中选择一个账号。")
            return
        if switcher.is_current(account):
            self.info("切换账号", f"{account.name} 就是当前客户端登录的账号。")
            return
        if account.platform == "trae":
            self.warn("切换账号",
                      "Trae 的登录态是设备绑定的加密格式，无法安全写回客户端。\n"
                      "请点「打开客户端」在 Trae 里手动登录。")
            return
        if account.platform not in SWITCHABLE:
            self.warn("切换账号", f"暂不支持写回 {account.platform_label}。")
            return
        if not account.meta.get("refresh_token"):
            self.warn("切换账号",
                      "这个账号没有 refresh token，无法写回客户端。\n"
                      "请先用「添加账号」通过浏览器登录一次。")
            return
        running = switcher.client_running_for(account)
        hint = ("\n检测到客户端正在运行：切换前会自动关闭它，完成后自动重新打开。"
                if running else "")
        # 免登录靠的是本机档案里的 Chromium 登录态，目标没档案就只能重新登录一次
        if account.platform == "workbuddy":
            ep = self._edition_for(account)
            uid = str(account.meta.get("uid") or "")
            if not profiles.has_pack(ep, uid):
                hint += ("\n\n注意：本机没有保存过这个账号的登录态，切换后客户端会停在登录页，"
                         "需要重新登录一次。登录完成并关闭客户端后本工具会自动存档，"
                         "之后再切回它就不用再登录了。")
        if not self.confirm(
                "切换账号",
                f"将把 {account.platform_label} 官方客户端的登录态切换为：\n{account.name}\n\n"
                f"会修改官方客户端的登录文件，修改前自动备份。{hint}\n"
                "继续？"):
            return

        self._set_busy(True, "正在写回客户端登录态…")

        def worker(_emit):
            return switcher.switch(account, lambda message: self.log(message))

        def done(message: str):
            self._set_busy(False)
            self.log(f"已切换 {account.platform_label} 客户端登录：{account.name}", "ok")
            self.info("切换账号", message)
            bus.BUS.data_changed.emit("checkin")
            # 账号换了，会话归属跟着变：通知会话管理重拉（含云端会话列表）
            bus.BUS.data_changed.emit("sessions")
            self.refresh()

        tasks.submit(worker, on_done=done, on_error=self._fail)

    def remove_account(self) -> None:
        account = self._account_of(None)
        if account is None:
            self.info("移除账号", "请先在列表中选择一条记录。")
            return
        if not account.meta.get("stored"):
            self.info("移除账号", "本机客户端账号不能在这里移除，请到对应客户端退出登录。")
            return
        if not self.confirm("移除账号",
                            f"从本工具移除 {account.name}？\n不会影响官方客户端的登录状态。"):
            return
        store.remove(account.platform, account.account_id)
        self.log(f"已移除浏览器登录账号：{account.name}", "dim")
        self.refresh()
        self.notify("已移除账号")
        bus.BUS.data_changed.emit("checkin")

    # ------------------------------------------------------------ 账号档案
    # ------------------------------------------------------------ 自动保存
    def _save_target(self) -> tuple[Edition, str] | None:
        """按客户端实际登录的账号决定存哪份档案。

        .info 里的 uid 可能已经被「切换」改写成目标账号，而客户端真正登录的是
        account-snapshot.json 里记的那个；以 snapshot 为准，避免把旧会话存成
        新账号的档案，把档案内容污染掉。
        """
        account = self._current_workbuddy()
        if account is None:
            return None
        ep = self._edition_for(account)
        uid = profiles.current_uid(ep) or str(account.meta.get("uid") or "")
        if not uid:
            return None
        return ep, uid

    def _auto_save(self, reason: str, force: bool = False) -> None:
        """后台快照当前登录态。成功只在日志留一行，失败才轻提示。

        客户端运行时 Chromium 独占锁着 Cookies，拷贝一定拿不到登录态，
        所以这里直接跳过、不打扰用户，等它关掉后由定时器补一次。
        """
        if self._saving or self.busy:
            return
        if not force and time.monotonic() - self._last_save < AUTO_SAVE_INTERVAL:
            return
        target = self._save_target()
        if target is None:
            return
        ep, uid = target
        if winproc.process_running(ep.proc):
            return
        self._saving = True
        self._last_save = time.monotonic()

        def worker(_emit):
            return profiles.save_current(ep, uid)

        def done(_path):
            self._saving = False
            self.log(f"账号档案已自动保存（{reason}）", "dim")

        def failed(message: str):
            self._saving = False
            self.log(f"账号档案自动保存失败：{message.splitlines()[0]}", "error")
            self.notify("账号档案自动保存失败，详情见运行日志", "warn")

        tasks.submit(worker, on_done=done, on_error=failed)

    def flush(self) -> None:
        """退出前同步落盘：异步任务来不及跑完，进程就结束了。"""
        if self._saving:
            return
        if time.monotonic() - self._last_save < EXIT_SAVE_INTERVAL:
            return
        target = self._save_target()
        if target is None:
            return
        ep, uid = target
        if winproc.process_running(ep.proc):
            return
        try:
            profiles.save_current(ep, uid)
            self._last_save = time.monotonic()
            self.log(f"账号档案已保存（退出）：{uid[:8]}", "dim")
        except Exception as exc:      # 退出路径上不能再抛，只留日志
            self.log(f"退出前保存账号档案失败：{exc}", "error")

    def restore_sessions(self, iid: str | None = None) -> None:
        """用档案里的会话库覆盖客户端的 workbuddy.db，登录态不动。

        只允许对当前登录的账号操作：会话按 user_id 归属，把别的账号的库换进来
        客户端也读不到，反而会覆盖掉当前账号自己的会话。
        """
        account = self._account_of(iid)
        if account is None:
            self.info("恢复会话", "请先选择一个账号。")
            return
        if account.platform != "workbuddy":
            self.info("恢复会话", "只有 WorkBuddy 账号保存了会话档案。")
            return
        if not switcher.is_current(account):
            self.info("恢复会话", "只能恢复当前登录账号的会话；先切换到该账号再来恢复。")
            return
        ep = self._edition_for(account)
        uid = str(account.meta.get("uid") or "")
        if not uid or not profiles.has_pack(ep, uid):
            self.info("恢复会话", "这个账号还没有保存过会话档案。")
            return
        if not self.confirm(
                "恢复会话",
                f"将关闭客户端，用档案恢复 {account.name} 的会话记录，然后重新打开。\n"
                "客户端当前的会话库会被替换，登录态不变。继续吗？"):
            return
        if not profiles.ensure_closed(ep):
            self.error("关闭失败", f"{ep.label} 没能关闭，请手动退出后重试。")
            return

        self._set_busy(True, "正在恢复会话…")

        def worker(_emit):
            try:
                profiles.restore_sessions(ep, uid)
                return f"已恢复 {account.name} 的会话记录"
            finally:
                # 只要客户端被关过就重新打开，避免留下“软件打不开”的状态
                winproc.start_exe(ep.exe)

        def done(message: str) -> None:
            self._set_busy(False)
            self.log(message, "ok")
            self.notify(message)
            bus.BUS.data_changed.emit("sessions")

        tasks.submit(worker, on_done=done, on_error=self._fail)

    def logout(self) -> None:
        ep = self._edition_for(self._current_workbuddy())

        def action():
            ok, message = profiles.logout_and_restart(ep)
            if not ok:
                raise RuntimeError(message)
            return message

        self._run_profile(
            ep, action, "登录其它账号",
            "将先保存当前账号，然后清空登录态并重启客户端，停在登录页。\n"
            "登录后新账号的会话会重新累积，旧账号的会话仍在档案里。\n继续吗？",
            restart=False)

    def _run_profile(self, ep: Edition, action, title: str, message: str,
                     restart: bool = True) -> None:
        if self.busy:
            return
        if not self.confirm(title, message):
            return
        if not profiles.ensure_closed(ep):
            self.error("关闭失败", f"{ep.label} 没能关闭，请手动退出后重试。")
            return

        self._set_busy(True, "正在处理账号档案…")

        def worker(_emit):
            try:
                return action()
            finally:
                # 只要客户端被关过就重新打开，避免留下“软件打不开”的状态
                if restart:
                    winproc.start_exe(ep.exe)

        def done(result):
            self._set_busy(False)
            self.log(f"{ep.label}：{result}", "ok")
            self.notify(str(result).splitlines()[0])
            # 账号档案换了整包登录态+会话，会话管理需要按新账号重拉
            bus.BUS.data_changed.emit("sessions")
            self.refresh()

        def failed(message: str):
            self._set_busy(False)
            self.log(message, "error")
            self.error("操作失败", message.splitlines()[0])
            if restart:
                winproc.start_exe(ep.exe)

        tasks.submit(worker, on_done=done, on_error=failed)

    # ------------------------------------------------------------------ 说明
    def show_help(self) -> None:
        self.info(
            "签到说明",
            "· 账号来自各客户端已登录的本地凭证，签到走官方 Web 接口，不模拟界面点击。\n"
            f"· 每日轮询：软件启动时检查一次，之后每 {daily.INTERVAL_HOURS} 小时一次；"
            "确认进入新的一天后，先校验账号状态，再对可签的账号调用签到接口。\n"
            "· 同一账号当天签到成功后不再重复调用；接口给了下一轮开放时刻时，"
            "会等窗口开了再签。\n"
            "· 免签到：勾选后「全部签到」和每日轮询都会跳过它，但仍会刷新积分与到期。\n"
            "· WorkBuddy 国际版没有签到玩法，自动标记为免签到，不参与后台签到。\n"
            "· 手动「签到选中」对免签到账号依然有效（强制签到）。\n"
            "· 剩余积分来自平台实时接口；最近到期和临期积分按积分批次计算，"
            "Qoder 部分账号不返回批次时依据本工具的领取台账。\n"
            "· 自动化签到可能违反平台服务条款，请自行评估风险。",
        )

    # ------------------------------------------------------------------ 定时
    def _tick(self) -> None:
        self._watch_client_closed()

    def _watch_client_closed(self) -> None:
        """客户端从「运行中」变成「已关闭」的那一刻补存一次档案。

        登录态只在客户端退出后才是可读的完整文件，这是唯一不打扰用户的时机。
        """
        target = self._save_target()
        running = False
        if target is not None:
            running = winproc.process_running(target[0].proc)
            if self._client_was_running and not running:
                self._auto_save("客户端已关闭", force=True)
        self._client_was_running = running

