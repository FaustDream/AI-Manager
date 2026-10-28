# -*- coding: utf-8 -*-
"""总览：一屏看清今日签到与积分收支。"""
from __future__ import annotations

from datetime import datetime

from PySide6.QtCore import Qt
from PySide6.QtWidgets import QFrame, QHBoxLayout, QProgressBar, QVBoxLayout, QWidget

from ...checkin import creditlog
from ...checkin import service as checkin_service
from ...checkin.base import CreditSnapshot, StatusResult, summarize_lots
from ...core import tasks
from ..models import Column
from ..theme import PALETTE
from ..widgets import (Card, DataTable, StatCard, TrendBars, hline, label,
                       make_button)
from .base import BasePage

CHECKIN_COLS = [
    Column("platform", "平台", 118, "center"),
    Column("name", "账号", 170),
    Column("today", "今日", 76, "center"),
    Column("balance", "剩余积分", 92, "right"),
    Column("expire", "最近到期", 128, "center", stretch=True),
]

# 右侧「各账号积分」最多列几个：再多就把卡片撑得比左边还高
BALANCE_ROWS = 8


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
    subtitle = "今日签到与积分收支"

    def __init__(self, win):
        super().__init__(win)
        self.accounts = []
        self._by_iid: dict[str, object] = {}
        self._balances: dict[str, float | None] = {}
        self._balance_error: dict[str, str] = {}
        self._expiry: dict[str, str] = {}
        self._done: set[str] = set()
        self._stats: dict = {}
        self.balance_rows: dict[str, tuple] = {}
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

        stats = QHBoxLayout()
        stats.setSpacing(12)
        self.card_checkin = StatCard("checkin", "签到账号", "—", "正在读取…")
        self.card_credit = StatCard("bolt", "积分总额", "—", "正在读取…")
        self.card_gain = StatCard("plus", "今日增加", "—", "正在读取…", "ok")
        self.card_spend = StatCard("clock", "今日消耗", "—", "正在读取…", "warn")
        for card in (self.card_checkin, self.card_credit, self.card_gain, self.card_spend):
            stats.addWidget(card, 1)
        self.body.addLayout(stats)

        columns = QHBoxLayout()
        columns.setSpacing(12)

        # ---- 左：今日签到 ----
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
        columns.addWidget(checkin_card, 1)

        # ---- 右：积分趋势 + 各账号积分 ----
        right = QVBoxLayout()
        right.setSpacing(12)

        trend_card = Card(padding=16, spacing=8)
        trend_card.add_title("积分趋势", f"近 {creditlog.TREND_DAYS} 天余额合计")
        self.trend = TrendBars()
        trend_card.body.addWidget(self.trend)
        self.trend_hint = label("", "KeyLabel")
        self.trend_hint.setWordWrap(True)
        trend_card.body.addWidget(self.trend_hint)
        right.addWidget(trend_card)

        balance_card = Card(padding=16, spacing=8)
        balance_card.add_title("各账号积分", "按当前余额")
        self.balance_box = QVBoxLayout()
        self.balance_box.setSpacing(7)
        balance_card.body.addLayout(self.balance_box)
        self.balance_empty = label("正在读取…", "SectionHint")
        self.balance_empty.setWordWrap(True)
        balance_card.body.addWidget(self.balance_empty)
        right.addWidget(balance_card)
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
            # 余额快照在上面每个账号查完时就已落盘，这里统一算一次合计与走势
            emit(("stats", creditlog.summary()))
            return True

        tasks.submit(worker, on_done=lambda _r: self._set_busy(False),
                     on_error=self._failed, on_partial=self._apply)

    def refresh_credits(self) -> None:
        """只刷新剩余积分与到期，不查签到状态。"""
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
            emit(("stats", creditlog.summary()))
            return len(results)

        def done(count: int):
            self._set_busy(False)
            self.log(f"积分已刷新（{count} 个账号）", "ok")

        tasks.submit(worker, on_done=done,
                     on_error=self._failed, on_partial=self._apply)

    def on_credits(self, snapshots: dict) -> None:
        """定时轮询回来的积分快照：静默更新表格与统计。"""
        if not self._by_iid:
            return
        touched = False
        for iid, snapshot in snapshots.items():
            if iid in self._by_iid:
                self._render_credit(iid, snapshot)
                touched = True
        if touched:
            self._reload_stats()

    def _reload_stats(self) -> None:
        if self.busy:
            return
        tasks.submit(lambda _emit: creditlog.summary(),
                     on_done=self._apply_stats,
                     on_error=lambda message: self.log(
                         f"积分统计失败：{message.splitlines()[0]}", "dim"))

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
        elif kind == "stats":
            self._apply_stats(payload)

    def _render_accounts(self) -> None:
        checked_ids = checkin_service.checked_ids(self.accounts)
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
        self._build_balance_rows()
        self._update_cards()

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
        if result.checked_in is not None:
            checked = result.checked_in
        elif checkin_service.next_from_api(account.platform):
            checked = None
        else:
            checked = True if iid in self._done else None
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
        self._update_cards()

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
        self._update_cards()

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

    # ------------------------------------------------------------ 积分仪表盘
    def _build_balance_rows(self) -> None:
        """账号换了才重建：平时只改数值，避免每次刷新都抖动一次。"""
        while self.balance_box.count():
            item = self.balance_box.takeAt(0)
            widget = item.widget()
            if widget is not None:
                widget.deleteLater()
        self.balance_rows = {}
        if not self.accounts:
            self.balance_empty.setText("还没有发现账号")
            self.balance_empty.show()
            return
        self.balance_empty.hide()
        for account in self.accounts[:BALANCE_ROWS]:
            row = QWidget()
            layout = QHBoxLayout(row)
            layout.setContentsMargins(0, 0, 0, 0)
            layout.setSpacing(8)
            name = label(account.name, "StatLabel")
            name.setFixedWidth(92)
            name.setToolTip(account.name)
            bar = QProgressBar()
            bar.setObjectName("ThinBar")
            bar.setTextVisible(False)
            bar.setRange(0, 100)
            bar.setValue(0)
            value = label("—", "ValueLabel")
            value.setFixedWidth(62)
            value.setAlignment(Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter)
            layout.addWidget(name)
            layout.addWidget(bar, 1)
            layout.addWidget(value)
            self.balance_box.addWidget(row)
            self.balance_rows[account.iid] = (bar, value)
        extra = len(self.accounts) - BALANCE_ROWS
        if extra > 0:
            self.balance_empty.setText(f"另有 {extra} 个账号未显示")
            self.balance_empty.show()
        self._update_balance_bars()

    def _update_balance_bars(self) -> None:
        values = [v for v in self._balances.values() if v is not None]
        peak = max(values) if values else 0
        for iid, (bar, value) in self.balance_rows.items():
            current = self._balances.get(iid)
            if current is None:
                bar.setValue(0)
                value.setText(self._balance_error.get(iid) or "—")
                continue
            bar.setValue(int(current / peak * 100) if peak else 0)
            value.setText(f"{current:g}")

    def _apply_stats(self, stats: dict | None) -> None:
        self._stats = stats or {}
        total = float(self._stats.get("total") or 0)
        gained = float(self._stats.get("gained") or 0)
        spent = float(self._stats.get("spent") or 0)
        baseline = str(self._stats.get("baseline") or "")
        dated = [v for v in self._expiry.values() if v]
        nearest = min(dated) if dated else ""
        hint = f"{self._stats.get('accounts') or 0} 个账号有余额记录"
        if nearest:
            hint += f" · 最近到期 {nearest}"
        self.card_credit.set_value(f"{total:g}", hint)

        compare = f"较 {baseline[5:]} 的记录" if baseline else "还没有可对比的记录"
        self.card_gain.set_value(f"+{gained:g}" if gained else "0", compare)
        self.card_spend.set_value(f"-{spent:g}" if spent else "0", compare)

        trend = self._stats.get("trend") or []
        self.trend.set_points(trend)
        if len(trend) >= 2:
            first, last = trend[0][1], trend[-1][1]
            delta = last - first
            self.trend_hint.setText(
                f"{first:g} → {last:g}（{delta:+g}）" if delta else f"稳定在 {last:g}")
        elif trend:
            self.trend_hint.setText(f"今天首次记录 {trend[-1][1]:g}，明天起这里显示变化")
        else:
            self.trend_hint.setText("历史从第一次刷新开始累积，之后每次刷新都会留下记录")
        self._update_cards()

    def _update_cards(self) -> None:
        done = sum(1 for iid in self._done if iid in self._by_iid)
        no_checkin = sum(1 for a in self.accounts if not a.requires_checkin)
        hint = f"今日已签到 {done} · 免签到 {no_checkin}"
        self.card_checkin.set_value(str(len(self.accounts)), hint)
        self._update_balance_bars()
        total = sum(v for v in self._balances.values() if v is not None)
        self.win.set_summary(
            "overview",
            f"{len(self.accounts)} 个账号 · 今日已签到 {done} · 剩余 {total:g} 积分")

    # ------------------------------------------------------------------ 动作
    def checkin_all(self) -> None:
        self.win.navigate("checkin")
        page = self.win.page("checkin")
        if page is not None:
            page.checkin(all_accounts=True)
