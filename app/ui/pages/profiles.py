# -*- coding: utf-8 -*-
"""已废弃：账号档案已并入账号中心的统一账号表（account_center.py）。

本文件不再注册到 PAGES，保留仅为追溯旧实现，不要在此继续加功能。
"""
from __future__ import annotations

import os

from PySide6.QtWidgets import QComboBox, QFrame, QHBoxLayout, QVBoxLayout

from ...core import bus, paths, tasks, winproc
from ...core.format import fmt_size
from ...sessions import profiles
from ...sessions.editions import Edition
from .. import button_config
from ..models import Column
from ..theme import PALETTE
from ..widgets import Badge, Card, DataTable, EmptyState, hline, label, make_button
from .base import BasePage

COLUMNS = [
    Column("account", "账号", 210),
    Column("sessions", "会话数", 80, "right"),
    Column("saved", "保存时间", 150, "center"),
    Column("size", "占用", 90, "right"),
    Column("path", "档案位置", 340),
]


class ProfilesPage(BasePage):
    key = "profiles"
    title = "账号档案"
    subtitle = "WorkBuddy 登录态 + 本机会话整包备份，用于免扫码切回旧账号"

    def __init__(self, win):
        super().__init__(win)
        self._gen = 0
        self._build()
        self.reload()

    # ------------------------------------------------------------------ 界面
    def _build(self) -> None:
        self.btn_bar = button_config.ButtonBar(self.cfg)
        header_buttons = self.btn_bar.build(
            self.header.actions, "profiles", "header", {
                "profiles.help": self.show_help,
                "profiles.open_dir": self.open_dir,
                "profiles.save_current": self.save_current,
            }, ctx=self._btn_ctx)
        self.btn_save = header_buttons.get("profiles.save_current")

        card = QFrame()
        card.setObjectName("Card")
        layout = QVBoxLayout(card)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(0)

        head = QHBoxLayout()
        head.setContentsMargins(16, 12, 16, 12)
        head.setSpacing(10)
        head.addWidget(label("版本", "StatLabel"))
        self.edition = QComboBox()
        self.edition.addItems(["WorkBuddy 国内版", "WorkBuddy 国际版"])
        self.edition.setFixedWidth(160)
        self.edition.currentIndexChanged.connect(lambda _i: self.reload())
        head.addWidget(self.edition)
        self.current_label = label("", "SectionHint")
        head.addWidget(self.current_label)
        head.addStretch(1)
        self.status_badge = Badge("", "muted")
        head.addWidget(self.status_badge)
        layout.addLayout(head)
        layout.addWidget(hline())

        self.table = DataTable(COLUMNS, key="uid", multi=False, parent=self)
        self.table.selectionModel().selectionChanged.connect(lambda *_: self._on_select())
        layout.addWidget(self.table, 1)
        self.empty = EmptyState("还没有保存过账号档案",
                                "先点右上角「保存当前账号」，之后就能在档案间来回切换")
        layout.addWidget(self.empty, 1)
        footer = QHBoxLayout()
        footer.setContentsMargins(16, 12, 16, 14)
        footer.setSpacing(8)
        hint = label("切换会先关闭并重启客户端；当前账号会自动保存，随时可以切回来。", "KeyLabel")
        footer.addWidget(hint)
        footer.addStretch(1)
        footer_buttons = self.btn_bar.build(footer, "profiles", "footer", {
            "profiles.delete": self.delete_pack,
            "profiles.logout": self.logout,
            "profiles.switch_restart": self.switch_to,
        }, ctx=self._btn_ctx)
        self.btn_delete = footer_buttons.get("profiles.delete")
        self.btn_logout = footer_buttons.get("profiles.logout")
        self.btn_switch = footer_buttons.get("profiles.switch_restart")
        layout.addLayout(footer)

        self.body.addWidget(card, 1)

    # ------------------------------------------------------------------ 数据
    def _edition(self) -> Edition:
        return Edition("cn" if self.edition.currentIndex() == 0 else "intl")

    def on_show(self) -> None:
        self.reload()

    def reload(self) -> None:
        """档案目录要遍历大小、会话数要开 SQLite，统一放后台算。

        档案包动辄几十上百 MB，以前在界面线程 rglob 统计，点一次菜单就卡一下。
        """
        self._gen += 1
        generation = self._gen
        ep = self._edition()
        self.current_label.setText("正在统计账号档案…")
        tasks.submit(lambda emit: self._collect(ep),
                     on_done=lambda data: self._apply(generation, data),
                     on_error=self._load_failed)

    @staticmethod
    def _collect(ep: Edition) -> dict:
        return {
            "uid": profiles.current_uid(ep),
            "packs": profiles.list_packs(ep),
            "running": winproc.process_running(ep.proc),
            "sessions": profiles.sessions_count(ep),
        }

    def _apply(self, generation: int, data: dict) -> None:
        if generation != self._gen:      # 期间切换了版本，结果作废
            return
        uid = data["uid"]
        packs = data["packs"]
        running = data["running"]
        self.current_label.setText(
            f"当前账号 {uid[:8] or '未登录'} · 本地会话 {data['sessions']} 条"
            f" · 档案 {len(packs)} 个")
        self.status_badge.set("运行中" if running else "已关闭",
                              "warn" if running else "ok")

        rows = []
        for pack in packs:
            current = "（当前）" if pack["uid"] == uid else ""
            rows.append({
                "uid": pack["uid"],
                "account": f"{pack['uid'][:8]}{current}",
                "sessions": pack["sessions"],
                "saved": str(pack["saved_at"]).replace("T", " "),
                "size": fmt_size(pack["size"]),
                "path": pack["path"],
                "_fg_map": {"account": PALETTE["accent"] if pack["uid"] == uid
                            else PALETTE["text"]},
            })
        self.table.set_rows(rows)
        self.table.setVisible(bool(rows))
        self.empty.setVisible(not rows)
        self._sync_buttons()
        self.win.set_summary("profiles", f"{len(packs)} 个档案")

    def _btn_ctx(self) -> dict:
        row = self.table.current_row()
        return {"rows": bool(self.table.model.rowCount()), "selected": row is not None}

    def _sync_buttons(self) -> None:
        self.btn_bar.sync({"busy": self.busy, **self._btn_ctx()})

    def _set_busy(self, busy: bool, text: str = "") -> None:
        super()._set_busy(busy, text)
        self._sync_buttons()

    def _load_failed(self, message: str) -> None:
        self.current_label.setText("档案统计失败，可点右上角「打开档案目录」检查")
        self.log(f"读取账号档案失败：{message.splitlines()[0]}", "error")

    def _on_select(self) -> None:
        row = self.table.current_row()
        if row:
            self.win.set_status(f"已选择档案 {row['account']}")
        self._sync_buttons()

    # ------------------------------------------------------------------ 动作
    def show_help(self) -> None:
        self.info(
            "账号档案是什么",
            "账号档案 = 某个 WorkBuddy 账号的「登录态 + 本机会话」整包备份。\n\n"
            "包含三样东西：\n"
            "· app/session：Chromium 登录态（Cookies / Local Storage）\n"
            "· storage：账号标识与部分本地设置\n"
            "· workbuddy.db：该账号的本地会话库\n\n"
            "它解决两个问题：\n"
            "1. 切回旧账号不用重新扫码登录；\n"
            "2. 不同账号的本地会话不会混在一起，切换后仍能看到各自的会话。\n\n"
            "和「账号中心」的区别：\n"
            "· 账号中心：只把 token 写回客户端登录文件，轻量、不搬会话；\n"
            "· 账号档案：整包换登录态 + 会话，体积大，需要关闭并重启客户端。\n\n"
            "注意：档案只保存在本机，不是云端备份；目前只支持 WorkBuddy 国内/国际版。"
            "Qoder / Trae 请用账号中心切换或手动登录。",
        )

    def _run(self, action, title: str, message: str, restart: bool = True) -> None:
        if self.busy:
            return
        if not self.confirm(title, message):
            return
        ep = self._edition()
        if not profiles.ensure_closed(ep):
            self.error("关闭失败", f"{ep.label} 没能关闭，请手动退出后重试。")
            return

        self._set_busy(True, "正在处理账号档案…")

        def worker(emit):
            try:
                return action(ep)
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
            self.reload()

        def failed(message: str):
            self._set_busy(False)
            self.log(message, "error")
            self.error("操作失败", message.splitlines()[0])
            if restart:
                winproc.start_exe(ep.exe)

        tasks.submit(worker, on_done=done, on_error=failed)

    def save_current(self) -> None:
        self._run(lambda ep: f"已保存到 {profiles.save_current(ep)}",
                  "保存当前账号",
                  "为避免拷贝到写了一半的登录态，会先关闭客户端，保存完自动重新打开。\n"
                  "档案包含登录凭据与会话库，仅保存在本机。继续吗？")

    def switch_to(self) -> None:
        row = self.table.current_row()
        if not row:
            self.info("未选择", "请先选中要切换到的账号档案。")
            return
        uid = row["uid"]

        def action(ep):
            ok, message = profiles.switch(ep, uid)
            if not ok:
                raise RuntimeError(message)
            return message

        self._run(action, "切换账号",
                  f"将关闭客户端，换入 {row['account']} 的登录态与本机会话，然后重启。\n"
                  "当前账号会先自动保存。继续吗？")

    def logout(self) -> None:
        def action(ep):
            ok, message = profiles.logout_and_restart(ep)
            if not ok:
                raise RuntimeError(message)
            return message

        self._run(action, "登录其它账号",
                  "将先保存当前账号，然后清空登录态并重启客户端，停在登录页。\n"
                  "登录后新账号的会话会重新累积，旧账号的会话仍在档案里。\n继续吗？",
                  restart=False)

    def delete_pack(self) -> None:
        row = self.table.current_row()
        if not row:
            self.info("未选择", "请先选中要删除的账号档案。")
            return
        uid = row["uid"]
        if not self.confirm("删除档案",
                            f"删除 {row['account']} 的档案？\n"
                            "删除后切回该账号需要重新登录。继续吗？"):
            return
        ep = self._edition()
        if profiles.delete_pack(ep, uid):
            self.log(f"已删除账号档案 {uid[:8]}", "dim")
            self.notify("已删除档案")
            self.reload()
        else:
            self.error("删除失败", "档案目录可能被占用。")

    def open_dir(self) -> None:
        ep = self._edition()
        target = paths.PROFILE_ROOT / ep.key
        target.mkdir(parents=True, exist_ok=True)
        os.startfile(str(target))
