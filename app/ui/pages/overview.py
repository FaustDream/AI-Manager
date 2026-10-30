# -*- coding: utf-8 -*-
"""总览：今日签到一屏看完，顶部只留四个关键数字。"""
from __future__ import annotations

from PySide6.QtWidgets import QFrame, QHBoxLayout, QVBoxLayout

from ...checkin import creditlog, statuslog
from ...checkin import service as checkin_service
from ...checkin.base import CreditSnapshot, StatusResult, summarize_lots
from ...core import tasks
from ...hub import service as hub_service
from ..models import Column
from ..theme import PALETTE
from ..widgets import DataTable, StatCard
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
        self._cached: dict[str, dict] = {}
        self._build()

    # ------------------------------------------------------------------ 界面
    def _build(self) -> None:
        # 页头整块去掉：左侧导航已经标出当前页，也不用手动刷新（见 on_show）
        self.header.hide()

        stats = QHBoxLayout()
        stats.setSpacing(12)
        self.card_credit = StatCard("bolt", "积分总额")
        self.card_gain = StatCard("plus", "今日增加", tone="ok")
        self.card_skill = StatCard("hub", "技能")
        self.card_mcp = StatCard("plug", "MCP")
        for card in (self.card_credit, self.card_gain, self.card_skill, self.card_mcp):
            stats.addWidget(card, 1)
        self.body.addLayout(stats)

        # ---- 签到表：整行铺满，不再有标题栏与按钮 ----
        checkin_card = QFrame()
        checkin_card.setObjectName("Card")
        checkin_layout = QVBoxLayout(checkin_card)
        checkin_layout.setContentsMargins(0, 0, 0, 0)
        checkin_layout.setSpacing(0)
        self.checkin_table = DataTable(CHECKIN_COLS, key="id", row_height=34)
        self.checkin_table.setMinimumHeight(220)
        checkin_layout.addWidget(self.checkin_table)
        self.body.addWidget(checkin_card, 1)

    # ------------------------------------------------------------------ 数据
    def on_show(self) -> None:
        # 没有手动刷新入口：每次切回总览都重扫一遍积分；签到状态当天只问一次接口
        self.refresh()

    def refresh(self) -> None:
        if self.busy:
            return
        self._set_busy(True)

        def worker(emit):
            cached = statuslog.today()      # 当天已确认过的签到状态，读盘放后台
            emit(("status_cache", cached))
            # 技能 / MCP 只数条目，先发出去，不用等账号查询跑完
            emit(("hub", hub_service.counts()))
            accounts = checkin_service.collect(self.cfg, lambda m, lv="info": self.log(m, lv))
            emit(("accounts", accounts))
            skip = set(self.cfg.get("skip") or [])
            answers: dict[str, bool] = {}

            def query(account) -> None:
                iid = account.iid
                if iid in cached:
                    # 今天的状态已经确认过：只刷积分，不再打签到接口
                    snapshot = checkin_service.query_credit(
                        account, lambda m: self.log(m, "dim"))
                    emit(("credit", (iid, snapshot)))
                    return
                kind, key, result = checkin_service.query_one(
                    account, iid in skip, lambda m: self.log(m, "dim"))
                if kind == "status":
                    answers[key] = {"checked": result.checked_in,
                                    "next_at": result.next_at}
                emit((kind, (key, result)))

            tasks.parallel_map(accounts, query)
            statuslog.note_many(answers)
            # 余额快照在上面每个账号查完时就已落盘，这里统一算一次合计
            emit(("stats", creditlog.summary()))
            return True

        tasks.submit(worker, on_done=lambda _r: self._set_busy(False),
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
        elif kind == "hub":
            self._apply_hub(payload)
        elif kind == "status_cache":
            self._cached = dict(payload or {})

    def _render_accounts(self) -> None:
        checked_ids = checkin_service.checked_ids(self.accounts)
        self._by_iid = {account.iid: account for account in self.accounts}
        self._balances = {}
        self._balance_error = {}
        self._expiry = {}
        # 当天已经查过的状态先用上，免得切回页面时整表闪一下「待签到」
        self._done = set(checked_ids) | {iid for iid, item in self._cached.items()
                                        if item["checked"]}
        rows = []
        for account in self.accounts:
            done = account.iid in self._done
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
        self._sync_summary()

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
        self._sync_summary()

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
        self._sync_summary()

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
        baseline = str(self._stats.get("baseline") or "")
        self.card_credit.set_value(f"{total:g}")
        # 平台不给流水，增减只能靠余额快照比出来：把基准日写进说明，0 才不会看着像没生效
        self.card_gain.set_value(
            f"+{gained:g}" if gained else "0",
            f"较 {baseline[5:]} 的余额" if baseline else "还没有可对比的记录")

    def _apply_hub(self, counts: dict | None) -> None:
        counts = counts or {}
        self.card_skill.set_value(str(counts.get("skills") or 0))
        self.card_mcp.set_value(str(counts.get("mcp") or 0))

    def _sync_summary(self) -> None:
        done = sum(1 for iid in self._done if iid in self._by_iid)
        total = sum(v for v in self._balances.values() if v is not None)
        self.win.set_summary(
            "overview",
            f"{len(self.accounts)} 个账号 · 今日已签到 {done} · 剩余 {total:g} 积分")
