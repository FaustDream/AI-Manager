# -*- coding: utf-8 -*-
"""总览：今日签到一屏看完，顶部只留四个关键数字。"""
from __future__ import annotations

from PySide6.QtWidgets import QFrame, QHBoxLayout, QVBoxLayout

from ...checkin import creditlog
from ...checkin import service as checkin_service
from ...checkin.base import CreditSnapshot, StatusResult, summarize_lots
from ...core import tasks
from ..models import Column
from ..theme import PALETTE
from ..widgets import DataTable, StatCard, hline, label, make_button
from .base import BasePage

# 表格独占整行，宽列让账号名吸收多余空间，其余列保持紧凑
CHECKIN_COLS = [
    Column("platform", "平台", 140, "center"),
    Column("name", "账号", 220, stretch=True),
    Column("today", "今日", 96, "center"),
    Column("balance", "剩余积分", 110, "right"),
    Column("expire", "最近到期", 150, "center"),
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


class OverviewPage(BasePage):
    key = "overview"
    title = "总览"

    def __init__(self, win):
        super().__init__(win)
        self.accounts = []
        self._by_iid: dict[str, object] = {}
        self._balances: dict[str, float | None] = {}
        self._balance_error: dict[str, str] = {}
        self._expiry: dict[str, str] = {}
        self._done: set[str] = set()
        self._stats: dict = {}
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
        self.card_checkin = StatCard("checkin", "签到账号")
        self.card_credit = StatCard("bolt", "积分总额")
        self.card_gain = StatCard("plus", "今日增加", tone="ok")
        self.card_spend = StatCard("clock", "今日消耗", tone="warn")
        for card in (self.card_checkin, self.card_credit, self.card_gain, self.card_spend):
            stats.addWidget(card, 1)
        self.body.addLayout(stats)

        # ---- 今日签到：整行铺满 ----
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
        self.body.addWidget(checkin_card, 1)

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
            # 余额快照在上面每个账号查完时就已落盘，这里统一算一次合计
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

    def _apply_stats(self, stats: dict | None) -> None:
        self._stats = stats or {}
        total = float(self._stats.get("total") or 0)
        gained = float(self._stats.get("gained") or 0)
        spent = float(self._stats.get("spent") or 0)
        self.card_credit.set_value(f"{total:g}")
        self.card_gain.set_value(f"+{gained:g}" if gained else "0")
        self.card_spend.set_value(f"-{spent:g}" if spent else "0")
        self._update_cards()

    def _update_cards(self) -> None:
        done = sum(1 for iid in self._done if iid in self._by_iid)
        self.card_checkin.set_value(str(len(self.accounts)))
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
