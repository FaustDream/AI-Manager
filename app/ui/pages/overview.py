# -*- coding: utf-8 -*-
"""总览：一眼看清签到、会话、备份的整体状态。"""
from __future__ import annotations

import os
from datetime import datetime

from PySide6.QtCore import Qt
from PySide6.QtWidgets import QFrame, QHBoxLayout, QProgressBar, QVBoxLayout, QWidget

from ...checkin import service as checkin_service
from ...checkin.base import CreditSnapshot, StatusResult, summarize_lots
from ...core import paths, tasks, winproc
from ...core.format import fmt_size
from ...sessions import service as session_service
from ...sessions.editions import Edition
from ..models import Column
from ..theme import PALETTE
from ..widgets import Badge, Card, DataTable, StatCard, hline, label, make_button
from .base import BasePage

CHECKIN_COLS = [
    Column("platform", "平台", 118, "center"),
    Column("name", "账号", 170),
    Column("today", "今日", 76, "center"),
    Column("balance", "剩余积分", 92, "right"),
    Column("expire", "最近到期", 128, "center", stretch=True),
]

SOURCES = ("WorkBuddy 国内版", "WorkBuddy 国际版", "CodeBuddy IDE", "IDEA 插件")


def _error_text(balance, summary: str) -> str:
    if balance is not None:
        return ""
    text = str(summary or "")
    if "失败" in text:
        return "读取失败"
    if "过期" in text:
        return "已过期"
    return ""


class OverviewPage(BasePage):
    key = "overview"
    title = "总览"
    subtitle = "签到、会话、账号档案的整体状态"

    def __init__(self, win):
        super().__init__(win)
        self.snap = None
        self.accounts = []
        self._by_iid: dict[str, object] = {}
        self._balances: dict[str, float | None] = {}
        self._balance_error: dict[str, str] = {}
        self._expiry: dict[str, str] = {}
        self._done: set[str] = set()
        self._first = True
        self._build()

    # ------------------------------------------------------------------ 界面
    def _build(self) -> None:
        self.btn_refresh = make_button("刷新全部", "ghost", "refresh")
        self.btn_refresh.clicked.connect(self.refresh)
        self.header.add_action(self.btn_refresh)
        self.btn_credit = make_button("刷新积分", "ghost", "clock")
        self.btn_credit.clicked.connect(self.refresh_credits)
        self.header.add_action(self.btn_credit)
        self.btn_sync = make_button("一键同步", "primary", "sync")
        self.btn_sync.clicked.connect(self.goto_sync)
        self.header.add_action(self.btn_sync)

        stats = QHBoxLayout()
        stats.setSpacing(12)
        self.card_checkin = StatCard("checkin", "签到账号", "—", "正在读取…")
        self.card_pending = StatCard("sync", "待同步会话", "—", "正在读取…", "accent")
        self.card_sessions = StatCard("sessions", "会话总数", "—", "正在读取…", "accent")
        self.card_trash = StatCard("archive", "回收站", "—", "正在读取…", "warn")
        for card in (self.card_checkin, self.card_pending, self.card_sessions, self.card_trash):
            stats.addWidget(card, 1)
        self.body.addLayout(stats)

        columns = QHBoxLayout()
        columns.setSpacing(12)

        left = QVBoxLayout()
        left.setSpacing(12)

        checkin_card = QFrame()
        checkin_card.setObjectName("Card")
        checkin_layout = QVBoxLayout(checkin_card)
        checkin_layout.setContentsMargins(0, 0, 0, 0)
        checkin_layout.setSpacing(0)
        head = QHBoxLayout()
        head.setContentsMargins(16, 12, 16, 10)
        head.setSpacing(8)
        head.addWidget(label("今日签到", "SectionTitle"))
        self.checkin_hint = label("", "SectionHint")
        head.addWidget(self.checkin_hint)
        head.addStretch(1)
        self.btn_all = make_button("全部签到", "primary", "bolt")
        self.btn_all.clicked.connect(self.checkin_all)
        head.addWidget(self.btn_all)
        checkin_layout.addLayout(head)
        checkin_layout.addWidget(hline())
        self.checkin_table = DataTable(CHECKIN_COLS, key="id", row_height=34)
        self.checkin_table.setMinimumHeight(220)
        checkin_layout.addWidget(self.checkin_table, 1)
        left.addWidget(checkin_card, 1)

        status_card = Card(padding=16, spacing=8)
        status_card.add_title("运行状态", "写入会话前需关闭对应客户端")
        self.status_rows = {}
        for key, name in (("cn", "WorkBuddy 国内版"), ("intl", "WorkBuddy 国际版")):
            row = QHBoxLayout()
            row.setSpacing(8)
            row.addWidget(label(name, "StatLabel"))
            row.addStretch(1)
            badge = Badge("—", "muted")
            row.addWidget(badge)
            self.status_rows[key] = badge
            status_card.body.addLayout(row)
        status_card.body.addWidget(hline())
        self.backup_hint = label("", "KeyLabel")
        status_card.body.addWidget(self.backup_hint)
        left.addWidget(status_card)

        columns.addLayout(left, 1)

        right = QVBoxLayout()
        right.setSpacing(12)

        source_card = Card(padding=16, spacing=9)
        source_card.add_title("会话来源", "四个来源合计")
        self.source_bars = {}
        for name in SOURCES:
            row = QHBoxLayout()
            row.setSpacing(8)
            tag = label(name, "StatLabel")
            tag.setFixedWidth(112)
            row.addWidget(tag)
            bar = QProgressBar()
            bar.setObjectName("ThinBar")
            bar.setTextVisible(False)
            bar.setRange(0, 100)
            bar.setValue(0)
            row.addWidget(bar, 1)
            value = label("0", "ValueLabel")
            value.setFixedWidth(44)
            value.setAlignment(Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter)
            row.addWidget(value)
            source_card.body.addLayout(row)
            self.source_bars[name] = (bar, value)
        source_card.body.addWidget(hline())
        self.size_hint = label("", "KeyLabel")
        source_card.body.addWidget(self.size_hint)
        right.addWidget(source_card)

        quick = Card(padding=16, spacing=8)
        quick.add_title("快捷操作")
        for text, icon_name, handler in (
            ("打开签到中心", "checkin", lambda: self.win.navigate("checkin")),
            ("打开会话管理", "sessions", lambda: self.win.navigate("sessions")),
            ("切换账号档案", "profiles", lambda: self.win.navigate("profiles")),
            ("打开数据目录", "folder", lambda: os.startfile(str(paths.DATA_DIR))),
        ):
            button = make_button(text, "ghost", icon_name)
            button.clicked.connect(handler)
            quick.body.addWidget(button)
        right.addWidget(quick)
        right.addStretch(1)

        holder = QWidget()
        holder.setLayout(right)
        holder.setFixedWidth(320)
        columns.addWidget(holder)
        self.body.addLayout(columns, 1)

    # ------------------------------------------------------------------ 数据
    def on_show(self) -> None:
        if self._first:
            self._first = False
            self.refresh()

    def refresh(self) -> None:
        if self.busy:
            return
        self._set_busy(True)

        def worker(emit):
            accounts = checkin_service.collect(self.cfg, lambda m, lv="info": self.log(m, lv))
            emit(("accounts", accounts))
            skip = set(self.cfg.get("skip") or [])

            def query(account) -> None:
                kind, iid, result = checkin_service.query_one(
                    account, account.iid in skip, lambda m: self.log(m, "dim"))
                emit((kind, (iid, result)))

            tasks.parallel_map(accounts, query)
            try:
                snap = session_service.build_snapshot(True, with_cloud=False)
            except Exception as exc:
                self.log(f"读取会话失败：{exc}", "error")
                snap = None
            emit(("snapshot", snap))
            return True

        tasks.submit(worker, on_done=lambda _r: self._set_busy(False),
                     on_error=self._failed, on_partial=self._apply)

    def refresh_credits(self) -> None:
        """只刷新剩余积分与到期，不查签到状态、不扫会话。"""
        if self.busy:
            return
        targets = list(self._by_iid.values())
        if not targets:
            self.refresh()
            return
        self._set_busy(True)

        def worker(emit):
            results = checkin_service.query_credits(
                targets, lambda m: self.log(m, "dim"))
            for iid, snapshot in results.items():
                emit(("credit", (iid, snapshot)))
            return len(results)

        def done(count: int):
            self._set_busy(False)
            self.log(f"积分已刷新（{count} 个账号）", "ok")

        tasks.submit(worker, on_done=done,
                     on_error=self._failed, on_partial=self._apply)

    def on_credits(self, snapshots: dict) -> None:
        """定时轮询回来的积分快照：静默更新今日签到卡片。"""
        if not self._by_iid:
            return
        for iid, snapshot in snapshots.items():
            if iid in self._by_iid:
                self._render_credit(iid, snapshot)

    def _failed(self, message: str) -> None:
        self._set_busy(False)
        self.log(message, "error")
        self.notify("读取失败，详情见运行日志", "error")

    def _apply(self, event) -> None:
        kind, payload = event
        if kind == "accounts":
            self.accounts = payload
            self._render_accounts()
        elif kind == "status":
            self._render_status(*payload)
        elif kind == "credit":
            self._render_credit(*payload)
        elif kind == "snapshot":
            self.snap = payload
            self._render_snapshot()

    def _render_accounts(self) -> None:
        checked_ids = checkin_service.checked_ids()
        skip = set(self.cfg.get("skip") or [])
        self._by_iid = {account.iid: account for account in self.accounts}
        self._balances = {}
        self._balance_error = {}
        self._expiry = {}
        self._done = set(checked_ids)
        rows = []
        for account in self.accounts:
            done = account.iid in checked_ids
            rows.append({
                "id": account.iid,
                "platform": account.platform_label,
                "name": account.name,
                "today": "" if not account.requires_checkin else
                         ("已签到" if done else "待签到"),
                "balance": "—",
                "expire": "—",
                "_fg_map": {
                    "today": PALETTE["muted"] if not account.requires_checkin else
                             (PALETTE["ok"] if done else PALETTE["warn"]),
                    "balance": PALETTE["muted"],
                },
            })
        self.checkin_table.set_rows(rows)
        self.checkin_hint.setText("" if rows else "未发现账号")
        self.header.set_subtitle(
            f"{datetime.now():%Y年%m月%d日} · {len(self.accounts)} 个签到账号")
        no_checkin = sum(1 for a in self.accounts if not a.requires_checkin)
        skipped = sum(1 for a in self.accounts if a.requires_checkin and a.iid in skip)
        self.card_checkin.set_value(
            str(len(self.accounts)),
            f"今日已签到 {sum(1 for iid in self._done if iid in self._by_iid)}"
            f" · 免签到 {no_checkin + skipped}")
        self._update_card_credits()

    def _render_status(self, iid: str, result: StatusResult) -> None:
        account = self._by_iid.get(iid)
        if account is None:
            return
        self._balances[iid] = result.balance
        self._balance_error[iid] = _error_text(result.balance, result.summary)
        if result.checked_in is True:
            self._done.add(iid)
        elif result.checked_in is False:
            self._done.discard(iid)
        if result.checked_in is None:
            checked = True if iid in self._done else None
        else:
            checked = result.checked_in
        self._set_row(iid, result.lots)
        requires = account.requires_checkin
        if not requires:
            today = ""
        else:
            today = "已签到" if checked else ("待签到" if checked is False else "—")
        self.checkin_table.model.update(
            iid, today=today,
            balance=self._balance_text(iid),
            _fg_map={"today": PALETTE["muted"] if not requires else
                     (PALETTE["ok"] if checked else
                      (PALETTE["warn"] if checked is False else PALETTE["muted"])),
                     "balance": PALETTE["muted"]})
        self._update_card_credits()

    def _render_credit(self, iid: str, snapshot: CreditSnapshot) -> None:
        account = self._by_iid.get(iid)
        if account is None:
            return
        self._balances[iid] = snapshot.balance
        self._balance_error[iid] = _error_text(snapshot.balance, snapshot.summary)
        self._set_row(iid, snapshot.lots)
        changes = {"balance": self._balance_text(iid),
                   "_fg_map": {"balance": PALETTE["muted"]}}
        if not account.requires_checkin:
            changes["today"] = ""
            changes["_fg_map"]["today"] = PALETTE["muted"]
        self.checkin_table.model.update(iid, **changes)
        self._update_card_credits()

    def _set_row(self, iid: str, lots) -> None:
        nearest_amount = 0.0
        if lots is not None:
            merged = list(lots)
            account = self._by_iid.get(iid)
            if account is not None and account.platform.startswith("qoder"):
                from ...checkin import ledger
                merged.extend(ledger.credit_lots(iid))
            expiry, _soon, _total, nearest_amount = summarize_lots(merged)
            self._expiry[iid] = expiry
        else:
            # Qoder / Trae 不返回批次时，到期统计依赖领取台账
            from ...checkin import ledger
            expiry, _soon, _count = ledger.summary(iid)
            self._expiry[iid] = expiry
        balance = self._balances.get(iid) or 0
        if self._expiry[iid]:
            expire_text = (f"{self._expiry[iid]} · {nearest_amount:g}"
                           if nearest_amount else self._expiry[iid])
        else:
            expire_text = "未提供" if balance > 0 else "—"
        self.checkin_table.model.update(iid, expire=expire_text)

    def _balance_text(self, iid: str) -> str:
        value = self._balances.get(iid)
        if value is None:
            return self._balance_error.get(iid) or "—"
        return f"{value:g}"

    def _update_card_credits(self) -> None:
        balance = sum(v for v in self._balances.values() if v is not None)
        dated = [v for v in self._expiry.values() if v]
        nearest = min(dated) if dated else ""
        hint = (f"今日已签到 {sum(1 for iid in self._done if iid in self._by_iid)}"
                f" · 免签到 {sum(1 for a in self.accounts if not a.requires_checkin)}")
        if self._balances:
            hint += f" · 剩余 {balance:g} 积分"
        if nearest:
            hint += f" · 最近到期 {nearest}"
        self.card_checkin.set_value(str(len(self.accounts)), hint)
        self.win.set_summary("overview", hint)

    def _render_snapshot(self) -> None:
        if self.snap is None:
            self.card_pending.set_value("—", "未找到 WorkBuddy 数据目录")
            self.card_sessions.set_value("—", "读取失败")
            self.card_trash.set_value("—", "读取失败")
            return
        snap = self.snap
        restore_total = sum(len(rows) for rows in snap.restores.values())
        self.card_pending.set_value(
            str(snap.pending), f"待恢复可见 {restore_total} 条 · 同步前自动备份")
        self.card_sessions.set_value(
            str(len(snap.unified)),
            f"有正文 {sum(1 for r in snap.unified if r['has_body'])} 条 · "
            f"{fmt_size(snap.total_size)}")
        self.card_trash.set_value(
            str(len(snap.trash)), f"黑名单 {snap.tomb_count} 条 · 备份 {len(snap.backups)} 份")

        counts = {}
        for row in snap.unified:
            counts[row["source_label"]] = counts.get(row["source_label"], 0) + 1
        peak = max(list(counts.values()) + [1])
        for name, (bar, value) in self.source_bars.items():
            count = counts.get(name, 0)
            bar.setValue(int(count / peak * 100))
            value.setText(str(count))

        for key, badge in self.status_rows.items():
            ep = Edition(key)
            running = winproc.process_running(ep.proc)
            badge.setText("运行中" if running else "已关闭")
            badge.set_tone("warn" if running else "ok")

        if snap.backups:
            latest = snap.backups[0]
            self.backup_hint.setText(
                f"最近备份 {latest['tag']} · {latest['count']} 条 · "
                f"{str(latest['created']).replace('T', ' ')}")
        else:
            self.backup_hint.setText("还没有备份记录，首次同步时会自动创建")
        self.size_hint.setText(
            f"WorkBuddy 国内 {snap.cn_total} 条 / 国际 {snap.intl_total} 条，"
            f"当前可见 {snap.cn_visible} / {snap.intl_visible}")

    # ------------------------------------------------------------------ 动作
    def checkin_all(self) -> None:
        self.win.navigate("checkin")
        page = self.win.page("checkin")
        if page is not None:
            page.checkin(all_accounts=True)

    def goto_sync(self) -> None:
        self.win.navigate("sessions")
        page = self.win.page("sessions")
        if page is not None:
            page.sync_all()
