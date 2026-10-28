# -*- coding: utf-8 -*-
"""已废弃：签到功能已并入账号中心的统一账号表（account_center.py）。

本文件不再注册到 PAGES，保留仅为追溯旧实现，不要在此继续加功能。
"""
from __future__ import annotations

from datetime import datetime
from pathlib import Path

from PySide6.QtCore import QTime, QTimer
from PySide6.QtWidgets import QCheckBox, QFrame, QHBoxLayout, QVBoxLayout, QWidget

from ...checkin import ledger, service
from ...checkin.base import (Account, ActionResult, CreditSnapshot, StatusResult,
                             summarize_lots)
from ...core import bus, tasks, winproc
from .. import button_config
from ..dialogs import AddAccountDialog
from ..models import Column
from ..theme import PALETTE
from ..widgets import Card, DataTable, TimeField, hline, label, make_button
from .base import BasePage

COLUMNS = [
    Column("platform", "平台", 118, "center"),
    Column("name", "账号", 190),
    Column("login", "登录态", 104, "center"),
    Column("today", "今日签到", 78, "center"),
    Column("balance", "剩余积分", 92, "right"),
    Column("expire", "最近到期", 132, "center"),
    Column("soon", "7 天内到期", 92, "right"),
    Column("skip", "签到", 62, "center"),
    Column("source", "来源文件", 130, stretch=True),
]


def _error_text(balance, summary: str) -> str:
    if balance is not None:
        return ""
    text = str(summary or "")
    if "失败" in text:
        return "读取失败"
    if "过期" in text:
        return "已过期"
    return ""


class CheckinPage(BasePage):
    key = "checkin"
    title = "签到中心"
    subtitle = ""

    def __init__(self, win):
        super().__init__(win)
        self.accounts: dict[str, Account] = {}
        self.live_lots: dict[str, list] = {}
        self.skip: set[str] = set(self.cfg.get("skip") or [])
        self._job_running = False
        # 已签到集合与积分台账各读一次缓存起来：逐行读文件在账号多时是 O(n²) 磁盘读
        self._checked: set[str] = set()
        self._ledger: dict[str, tuple[str, float, int]] = {}
        self._balances: dict[str, float | None] = {}
        self._balance_error: dict[str, str] = {}
        self._expiry: dict[str, str] = {}
        self._soon: dict[str, float] = {}
        self._needs: dict[str, bool] = {}

        self._build()
        self._load_auto()

        self._timer = QTimer(self)
        self._timer.setInterval(30_000)
        self._timer.timeout.connect(self._tick_auto)
        self._timer.start()

    # ------------------------------------------------------------------ 界面
    def _build(self) -> None:
        toolbar = Card(padding=14, spacing=10)
        row = QHBoxLayout()
        row.setSpacing(8)
        self.btn_bar = button_config.ButtonBar(self.cfg)
        buttons = self.btn_bar.build(row, "checkin", "toolbar", {
            "checkin.checkin_all": lambda: self.checkin(all_accounts=True),
            "checkin.checkin_selected": lambda: self.checkin(all_accounts=False),
            "checkin.query_status": lambda: self.query(selected_only=False),
            "checkin.refresh_credits": self.refresh_credits,
            "checkin.refresh_login": self.refresh,
            "checkin.add_account": self.add_account,
            "checkin.toggle_skip": self.toggle_skip,
            "checkin.open_client": self.open_client,
        }, ctx=self._btn_ctx)
        self.btn_all = buttons.get("checkin.checkin_all")
        self.btn_one = buttons.get("checkin.checkin_selected")
        self.btn_status = buttons.get("checkin.query_status")
        self.btn_credit = buttons.get("checkin.refresh_credits")
        self.btn_refresh = buttons.get("checkin.refresh_login")
        self.btn_add = buttons.get("checkin.add_account")
        self.btn_skip = buttons.get("checkin.toggle_skip")
        self.btn_client = buttons.get("checkin.open_client")
        row.addStretch(1)

        self.auto_check = QCheckBox("每日自动签到")
        self.auto_check.stateChanged.connect(self._save_auto)
        row.addWidget(self.auto_check)
        self.time_edit = TimeField()
        self.time_edit.timeChanged.connect(lambda _t: self._save_auto())
        row.addWidget(self.time_edit)
        toolbar.body.addLayout(row)

        self.action_buttons = list(buttons.values())
        self.body.addWidget(toolbar)

        table_card = QFrame()
        table_card.setObjectName("Card")
        table_layout = QVBoxLayout(table_card)
        table_layout.setContentsMargins(0, 0, 0, 0)
        table_layout.setSpacing(0)

        head = QWidget()
        head_layout = QHBoxLayout(head)
        head_layout.setContentsMargins(16, 12, 16, 10)
        head_layout.setSpacing(8)
        self.summary = label("正在扫描登录态…", "SectionHint")
        head_layout.addWidget(label("账号与积分", "SectionTitle"))
        head_layout.addWidget(self.summary)
        head_layout.addStretch(1)
        self.hint_link = make_button("免签到的账号不参与批量签到", "link")
        self.hint_link.clicked.connect(self.show_help)
        head_layout.addWidget(self.hint_link)
        table_layout.addWidget(head)
        table_layout.addWidget(hline())

        self.table = DataTable(COLUMNS, key="id", parent=self)
        self.table.doubleClicked.connect(lambda _i: self.query(selected_only=True))
        self.table.selectionModel().selectionChanged.connect(lambda *_: self._sync_buttons())
        table_layout.addWidget(self.table, 1)
        self.body.addWidget(table_card, 1)

    # ------------------------------------------------------------------ 数据
    def on_show(self) -> None:
        if not self.accounts:
            self.refresh()

    def on_data_changed(self, scope: str) -> None:
        if scope == "checkin" and not self.busy:
            self.refresh()

    def _btn_ctx(self) -> dict:
        row = self.table.current_row()
        account = self.accounts.get(row["id"]) if row else None
        return {
            "rows": bool(self.accounts),
            "selected": row is not None,
            "checkin_account": bool(account and account.requires_checkin),
        }

    def _set_busy(self, busy: bool, text: str = "") -> None:
        self.busy = busy
        self.win.set_busy(busy, text)
        self._sync_buttons()

    def _sync_buttons(self) -> None:
        """按启用规则统一刷新按钮状态（busy / 有数据 / 选中行）。"""
        self.btn_bar.sync({
            "busy": self.busy,
            "rows": bool(self.accounts),
            "selected": self.table.current_row() is not None,
        })

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

    def on_credits(self, snapshots: dict) -> None:
        """定时轮询回来的积分快照：静默更新表格，不打扰用户操作。"""
        if not self.accounts:
            return
        updated = 0
        for iid, snapshot in snapshots.items():
            if iid in self.accounts:
                self._render_credit(iid, snapshot)
                updated += 1
        if updated:
            self.log(f"积分自动轮询更新 {updated} 个账号", "dim")

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
            row = self.table.current_row()
            if not row:
                self.info("未选择", "请先在列表中选择一个账号。")
                return
            account = self.accounts.get(row["id"])
            if not account:
                return
            if not account.requires_checkin:
                self.info("无需签到", f"{account.platform_label} 不需要签到，只查询积分即可。")
                return
            targets = [account]

        self._set_busy(True, "正在签到…")
        tasks.submit(lambda emit: self._checkin_worker(emit, targets),
                     on_done=self._checkin_done, on_error=self._fail,
                     on_partial=self._apply)

    def _checkin_worker(self, emit, targets: list[Account]):
        success = 0
        for account in targets:
            self.log(f"[{account.platform_label}] 正在为 {account.name} 签到…", "dim")
            result = service.checkin_one(account, self.log)
            if result.ok:
                service.mark_checked(account.iid)
                service.record_grant(account, result)
                success += 1
            emit(("checkin", (account, result)))
            if result.ok:
                fresh = service.refresh_status_after_checkin(account, self.log)
                if fresh is not None:
                    emit(("status", (account.iid, fresh)))
        self.log(f"签到结束：成功 {success}/{len(targets)}",
                 "ok" if success == len(targets) else "error")
        return (success, len(targets))

    def _checkin_done(self, payload) -> None:
        self._set_busy(False)
        self._update_summary()
        success, total = payload
        self.notify(f"签到完成：成功 {success}/{total}", "ok" if success == total else "warn")

    def _fail(self, message: str) -> None:
        self._set_busy(False)
        self.log(message, "error")
        self.notify("操作失败，详情见运行日志", "error")

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

    def _login_text(self, account: Account) -> str:
        return account.expires_short()

    def _refresh_caches(self) -> None:
        self._checked = service.checked_ids()
        self._ledger = ledger.summaries()

    def _render_accounts(self, accounts: list[Account]) -> None:
        self.accounts = {account.iid: account for account in accounts}
        self.live_lots = {}
        self._balances = {}
        self._balance_error = {}
        self._expiry = {}
        self._soon = {}
        self._needs = {account.iid: account.requires_checkin for account in accounts}
        self._refresh_caches()
        rows = []
        for account in accounts:
            iid = account.iid
            rows.append({
                "id": iid,
                "platform": account.platform_label,
                "name": account.name,
                "login": self._login_text(account),
                "today": "" if not account.requires_checkin else
                         ("已签到" if iid in self._checked else "—"),
                "balance": "—",
                "expire": "—",
                "soon": "—",
                "skip": "免签到" if not account.requires_checkin else
                        ("跳过" if iid in self.skip else "参与"),
                "source": Path(account.source).name if account.source else "—",
                "_fg_map": self._colors(iid, account),
            })
        self.table.set_rows(rows)
        for account in accounts:
            self._update_credit_cells(account.iid)
        self._update_summary()

    def _colors(self, iid: str, account: Account) -> dict:
        colors = {}
        if account.expired:
            colors["login"] = PALETTE["danger"]
        if iid in self._checked:
            colors["today"] = PALETTE["ok"]
        if not account.requires_checkin or iid in self.skip:
            colors["skip"] = PALETTE["muted"]
        if not account.requires_checkin:
            colors["today"] = PALETTE["muted"]
        return colors

    def _render_status(self, iid: str, result: StatusResult) -> None:
        if iid not in self.accounts:
            return
        account = self.accounts[iid]
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
        checked = result.checked_in if result.checked_in is not None else (iid in self._checked)
        if not requires:
            today = ""
        else:
            today = "已签到" if checked else ("待签到" if checked is False else "—")
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
            changes["today"] = ""
        self.table.model.update(iid, **changes)
        self._update_credit_cells(iid)
        self._update_summary()

    def _render_checkin(self, account: Account, result: ActionResult) -> None:
        iid = account.iid
        if iid not in self.accounts:
            return
        if result.ok and account.requires_checkin:
            self._checked.add(iid)
            # 台账刚写过这一条，只重算这一个账号
            self._ledger[iid] = ledger.summary(iid)
        colors = self._colors(iid, account)
        if not account.requires_checkin:
            today = ""
        else:
            today = "已签到" if result.ok else "失败"
            colors["today"] = PALETTE["ok"] if result.ok else PALETTE["danger"]
        self.table.model.update(
            iid, today=today,
            balance=self._balance_text(iid),
            _fg_map=colors,
            _tip_map={"balance": result.message})
        self._update_credit_cells(iid)
        self._update_summary()
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
        self._soon[iid] = soon
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
            existing["soon"] = PALETTE["warn"]
        tips["expire"] = (
            f"{expiry} 到期 {nearest_amount:g} 积分；"
            f"{ledger.SOON_DAYS} 天内合计 {soon:g}"
            if expiry else "平台未返回积分批次到期时间")
        self.table.model.update(iid,
                                balance=self._balance_text(iid),
                                expire=expire_text,
                                soon=f"{soon:g}" if soon else "—",
                                _fg_map=existing,
                                _tip_map=tips)

    def _update_summary(self) -> None:
        total = len(self.accounts)
        if not total:
            self.summary.setText("未发现账号")
            self.win.set_summary("checkin", "未发现账号")
            return
        active = sum(1 for iid, account in self.accounts.items()
                     if account.requires_checkin and iid not in self.skip)
        checked = sum(1 for iid in self.accounts if iid in self._checked)
        balance = sum(value for value in self._balances.values() if value is not None)
        soon = sum(self._soon.values())
        dated = [value for value in self._expiry.values() if value]
        nearest = min(dated) if dated else ""
        text = f"{total} 个账号 · 参与 {active} · 今日已签到 {checked}"
        if self._balances:
            text += f" · 剩余 {balance:g} 积分"
        if nearest:
            text += f" · 最近到期 {nearest}"
        if soon:
            text += f" · 7 天内到期 {soon:g}"
        self.summary.setText(text)
        self.win.set_summary("checkin", text)

    # ------------------------------------------------------------------ 动作
    def toggle_skip(self) -> None:
        row = self.table.current_row()
        if not row:
            self.info("免签到", "请先在列表中选择一个账号。")
            return
        iid = row["id"]
        account = self.accounts.get(iid)
        if not account:
            return
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
        self.table.model.update(iid, skip="跳过" if iid in self.skip else "参与",
                                today="—" if iid in self.skip else row.get("today", "—"),
                                _fg_map=colors)
        self._update_summary()
        self.log(f"[{account.platform_label}] {account.name} {note}", "dim")
        if resumed:
            self.query(selected_only=True)

    def add_account(self) -> None:
        row = self.table.current_row()
        platform = "workbuddy"
        if row and row["id"] in self.accounts:
            account = self.accounts[row["id"]]
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
        row = self.table.current_row()
        if not row:
            self.info("打开客户端", "请先在列表中选择一个账号。")
            return
        account = self.accounts.get(row["id"])
        if not account:
            return
        exe = service.client_exe(account.platform, account)
        if not exe or not Path(exe).exists():
            self.warn("打开客户端", f"未找到 {account.platform_label} 的可执行文件。")
            return
        if not winproc.start_exe(exe):
            self.warn("打开客户端", f"启动 {Path(exe).name} 失败，请手动打开。")
            return
        self.log(f"已启动 {Path(exe).name}，刷新登录态后点「刷新登录态」", "dim")

    def show_help(self) -> None:
        self.info(
            "签到说明",
            "· 账号来自各客户端已登录的本地凭证，签到走官方 Web 接口，不模拟界面点击。\n"
            "· 免签到：勾选后「全部签到」和每日自动都会跳过它，但仍会刷新积分与到期。\n"
            "· WorkBuddy 国际版没有签到玩法，自动标记为免签到，不参与后台签到。\n"
            "· 手动「签到选中」对免签到账号依然有效（强制签到）。\n"
            "· 剩余积分来自平台实时接口；最近到期和临期积分按积分批次计算，"
            "Qoder 部分账号不返回批次时依据本工具的领取台账。\n"
            "· 自动化签到可能违反平台服务条款，请自行评估风险。",
        )

    # ------------------------------------------------------------------ 定时
    def _load_auto(self) -> None:
        self.auto_check.setChecked(bool(self.cfg.get("auto")))
        text = str(self.cfg.get("auto_time") or "08:30")
        try:
            hour, minute = (int(part) for part in text.split(":"))
            self.time_edit.set_time(QTime(hour, minute))
        except (ValueError, TypeError):
            self.time_edit.set_time(QTime(8, 30))

    def apply_auto_settings(self) -> None:
        """设置页改了自动签到后同步到这里。"""
        self._load_auto()

    def _save_auto(self, *_args) -> None:
        current = self.time_edit.time()
        self.cfg["auto"] = self.auto_check.isChecked()
        self.cfg["auto_time"] = f"{current.hour():02d}:{current.minute():02d}"
        self.win.save_cfg()

    def _tick_auto(self) -> None:
        if not self.auto_check.isChecked() or self.busy:
            return
        if not self.accounts:
            return
        target = self.time_edit.time()
        now = datetime.now()
        if now.hour * 60 + now.minute < target.hour() * 60 + target.minute():
            return
        today = now.strftime("%Y-%m-%d")
        if self.cfg.get("last_auto") == today:
            return
        self.log("到达设定时间，触发每日自动签到", "dim")
        self.checkin(all_accounts=True)
        self.cfg["last_auto"] = today
        self.win.save_cfg()
        bus.BUS.tray_message.emit("AI Manager", "已按计划执行每日自动签到")
