# -*- coding: utf-8 -*-
"""已废弃：账号列表已并入账号中心的统一账号表（account_center.py）。

本文件不再注册到 PAGES，保留仅为追溯旧实现，不要在此继续加功能。
"""
from __future__ import annotations

from pathlib import Path

from PySide6.QtWidgets import QFrame, QHBoxLayout, QVBoxLayout

from ...checkin import service, store, switcher
from ...checkin.registry import LABELS, ORDER
from ...core import bus, tasks, winproc
from .. import button_config
from ..dialogs import AddAccountDialog
from ..models import Column
from ..theme import PALETTE
from ..widgets import Badge, Card, DataTable, EmptyState, hline, label, make_button
from .base import BasePage

COLUMNS = [
    Column("platform", "平台", 118, "center"),
    Column("name", "账号", 190),
    Column("current", "当前", 58, "center"),
    Column("login", "登录态到期", 110, "center"),
    Column("switch", "切换", 80, "center"),
    Column("source", "来源", 150, stretch=True),
]


class AccountsPage(BasePage):
    key = "accounts"
    title = "账号中心"
    subtitle = ""

    def __init__(self, win):
        super().__init__(win)
        self.accounts: dict[str, object] = {}
        self._build()
        self.reload()

    def _build(self) -> None:
        self.btn_bar = button_config.ButtonBar(self.cfg)
        header_buttons = self.btn_bar.build(
            self.header.actions, "accounts", "header",
            {"accounts.add": self.add_account}, ctx=self._btn_ctx)
        self.add_btn = header_buttons.get("accounts.add")

        split = QHBoxLayout()
        split.setSpacing(12)

        left = QFrame()
        left.setObjectName("Card")
        left_layout = QVBoxLayout(left)
        left_layout.setContentsMargins(0, 0, 0, 0)
        left_layout.setSpacing(0)

        head = QHBoxLayout()
        head.setContentsMargins(16, 12, 16, 10)
        head.setSpacing(8)
        head.addWidget(label("全部账号", "SectionTitle"))
        self.count_hint = label("", "SectionHint")
        head.addWidget(self.count_hint)
        head.addStretch(1)
        toolbar_buttons = self.btn_bar.build(head, "accounts", "toolbar", {
            "accounts.open_client": self.open_client,
            "accounts.switch_login": self.switch_client,
            "accounts.remove": self.remove_account,
        }, ctx=self._btn_ctx)
        self.btn_open = toolbar_buttons.get("accounts.open_client")
        self.btn_switch = toolbar_buttons.get("accounts.switch_login")
        self.btn_remove = toolbar_buttons.get("accounts.remove")
        left_layout.addLayout(head)
        left_layout.addWidget(hline())

        self.table = DataTable(COLUMNS, key="id", parent=self)
        self.table.selectionModel().selectionChanged.connect(lambda *_: self._sync_buttons())
        left_layout.addWidget(self.table, 1)
        self.empty = EmptyState("还没有发现任何账号",
                                "先打开对应客户端登录，或点右上角「添加账号」用浏览器授权")
        left_layout.addWidget(self.empty, 1)
        split.addWidget(left, 1)

        right = QVBoxLayout()
        right.setSpacing(12)

        capability = Card(padding=16, spacing=10)
        capability.add_title("平台能力", "写回客户端仅在关闭客户端时可用")
        for key in ORDER:
            row = QHBoxLayout()
            row.setSpacing(8)
            row.addWidget(label(LABELS[key], "ValueLabel"))
            row.addStretch(1)
            supported = key in ("qoder", "qoder_intl", "workbuddy")
            row.addWidget(Badge("支持写回" if supported else "仅查看", "ok" if supported else "muted"))
            capability.body.addLayout(row)
            capability.body.addWidget(label(switcher.describe(key), "KeyLabel"))
        capability.body.addWidget(hline())
        note = label("写回会自动备份原文件（同目录 .bak-时间戳），"
                     "失败时可手工还原。Trae 的登录态是设备绑定加密格式，"
                     "无法安全重写，请用「打开客户端」手动登录。", "KeyLabel")
        note.setWordWrap(True)
        capability.body.addWidget(note)
        right.addWidget(capability)
        right.addStretch(1)

        holder = QFrame()
        holder.setLayout(right)
        holder.setFixedWidth(340)
        split.addWidget(holder)

        self.body.addLayout(split, 1)

    # ------------------------------------------------------------------ 数据
    def on_show(self) -> None:
        self.reload()

    def on_data_changed(self, scope: str) -> None:
        if scope == "checkin":
            self.reload()

    def reload(self) -> None:
        # 账号中心要能看到“所有能切换的账号”，所以本机客户端账号和浏览器登录
        # 账号一起聚合；不再只列 store 里的浏览器账号。
        try:
            accounts = service.collect(self.cfg, lambda message, level="info": None)
        except Exception as exc:
            self.log(f"扫描账号失败：{exc}", "error")
            accounts = store.as_accounts()
        self.accounts = {account.iid: account for account in accounts}
        rows = []
        switchable = 0
        for account in accounts:
            current = switcher.is_current(account)
            can_switch = account.platform in ("qoder", "qoder_intl", "workbuddy")
            if can_switch and not current:
                switchable += 1
            rows.append({
                "id": account.iid,
                "platform": account.platform_label,
                "name": account.name,
                "current": "当前" if current else "",
                "login": account.expires_short(),
                "switch": "当前" if current else ("支持" if can_switch else "手动"),
                "source": (str(account.source).rsplit("\\", 1)[-1].rsplit("/", 1)[-1]
                           or "浏览器登录"),
                "_fg_map": {
                    "current": PALETTE["ok"] if current else PALETTE["muted"],
                    "login": PALETTE["danger"] if account.expired else PALETTE["text_2"],
                    "switch": PALETTE["text_2"] if can_switch else PALETTE["muted"],
                },
            })
        self.table.set_rows(rows)
        self.count_hint.setText(
            f"{len(rows)} 个 · 可切换 {switchable}" if rows else "")
        has_rows = bool(rows)
        self.table.setVisible(has_rows)
        self.empty.setVisible(not has_rows)
        self._sync_buttons()

    def _btn_ctx(self) -> dict:
        row = self.table.current_row()
        account = self.accounts.get(row["id"]) if row else None
        return {
            "rows": bool(self.accounts),
            "selected": row is not None,
            "stored_account": bool(account and getattr(account, "meta", {}).get("stored")),
        }

    def _sync_buttons(self) -> None:
        ctx = self._btn_ctx()
        self.btn_bar.sync({"busy": self.busy, **ctx})

    # ------------------------------------------------------------------ 动作
    def add_account(self) -> None:
        row = self.table.current_row()
        platform = "workbuddy"
        account = self.accounts.get(row["id"]) if row else None
        if account is not None:
            platform = account.platform
            if platform == "workbuddy" and account.meta.get("edition") == "intl":
                platform = "workbuddy_intl"
        if platform == "qoder_intl":
            self.info("添加账号",
                      "Qoder 国际版目前通过扫描本机客户端登录态接入，"
                      "请先打开 Qoder 客户端登录，再回到本页刷新。")
            return
        AddAccountDialog(self.win, platform).exec()

    def remove_account(self) -> None:
        row = self.table.current_row()
        if not row:
            self.info("移除账号", "请先在列表中选择一条记录。")
            return
        account = self.accounts.get(row["id"])
        if account is None or not account.meta.get("stored"):
            self.info("移除账号", "本机客户端账号不能在这里移除，请到对应客户端退出登录。")
            return
        platform, _, account_id = str(row["id"]).partition(":")
        if not self.confirm("移除账号",
                            f"从本工具移除 {row['name']}？\n不会影响官方客户端的登录状态。"):
            return
        store.remove(platform, account_id)
        self.log(f"已移除浏览器登录账号：{row['name']}", "dim")
        self.reload()
        self.notify("已移除账号")
        bus.BUS.data_changed.emit("checkin")

    def switch_client(self) -> None:
        row = self.table.current_row()
        if not row:
            self.info("切换账号", "请先在列表中选择一个账号。")
            return
        account = self.accounts.get(row["id"])
        if account is None:
            return
        if switcher.is_current(account):
            self.info("切换账号", f"{account.name} 就是当前客户端登录的账号。")
            return
        if account.platform == "trae":
            self.warn("切换账号",
                      "Trae 的登录态是设备绑定的加密格式，无法安全写回客户端。\n"
                      "请点「打开客户端」在 Trae 里手动登录。")
            return
        if account.platform not in ("qoder", "qoder_intl", "workbuddy"):
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
        if not self.confirm(
                "切换账号",
                f"将把 {account.platform_label} 官方客户端的登录态切换为：\n{account.name}\n\n"
                f"会修改官方客户端的登录文件，修改前自动备份。{hint}\n"
                "继续？"):
            return

        self._set_busy(True, "正在写回客户端登录态…")

        def worker(emit):
            return switcher.switch(account, lambda message: self.log(message))

        def done(message: str):
            self._set_busy(False)
            self.log(f"已切换 {account.platform_label} 客户端登录：{account.name}", "ok")
            self.info("切换账号", message)
            bus.BUS.data_changed.emit("checkin")
            # 账号换了，会话归属跟着变：通知会话管理重拉（含云端会话列表）
            bus.BUS.data_changed.emit("sessions")
            self.reload()

        tasks.submit(worker, on_done=done, on_error=self._switch_failed)

    def _switch_failed(self, message: str) -> None:
        self._set_busy(False)
        detail = message.splitlines()[0]
        self.error("切换账号", detail)
        self.log(message, "error")

    def open_client(self) -> None:
        row = self.table.current_row()
        account = self.accounts.get(row["id"]) if row else None
        platform = account.platform if account else "workbuddy"
        exe = service.client_exe(platform, account)
        if not exe or not Path(exe).exists():
            self.warn("打开客户端", f"未找到 {LABELS.get(platform, platform)} 的可执行文件。")
            return
        if not winproc.start_exe(exe):
            self.warn("打开客户端", f"启动 {Path(exe).name} 失败，请手动打开。")
            return
        self.log(f"已启动 {Path(exe).name}", "dim")
