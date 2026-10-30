# -*- coding: utf-8 -*-
"""对话框：添加账号（浏览器登录流程）。"""
from __future__ import annotations

import threading
import webbrowser

from PySide6.QtCore import Qt, QUrl
from PySide6.QtGui import QDesktopServices
from PySide6.QtWidgets import (QComboBox, QDialog, QHBoxLayout, QLabel, QLineEdit,
                               QVBoxLayout)

from ..checkin import logins, store
from ..checkin.base import Account
from ..checkin.registry import LABELS, LABEL_KEYS
from ..core import bus, tasks
from . import icons
from .models import Column
from .theme import PALETTE
from .widgets import Badge, DataTable, Spinner, hline, label, make_button

HINTS = {
    "trae": "会打开 trae.cn 授权页。在浏览器里登录并允许授权，页面会回调到本机 "
            "127.0.0.1，成功后自动加入左侧列表。",
    "qoder": "会打开 qoder.cn 设备授权页。登录并确认授权后，应用在后台轮询设备 token，"
             "无需复制任何内容。",
    "workbuddy": "会打开 WorkBuddy 国内版登录页（腾讯登录，桌面端同款）。"
                 "登录完成后自动取回 token。",
    "workbuddy_intl": "会打开 WorkBuddy 国际版登录页（workbuddy.ai）。"
                      "登录完成后自动取回 token。国际版没有签到，只查积分。",
    "zcode": "会打开 zcode.z.ai 授权页（BigModel 账号，手机号登录）。登录后应用自动取回"
             "登录态与额度配置，不影响客户端当前的登录。",
    "zcode_zai": "会打开 zcode.z.ai 授权页（z.ai 国际站账号，邮箱/第三方登录）。"
                 "登录后应用自动取回登录态与额度配置。",
}

ORDER = ("trae", "qoder", "workbuddy", "workbuddy_intl", "zcode", "zcode_zai")

COMBO_LABELS = {
    "trae": "Trae CN",
    "qoder": "Qoder CN",
    "workbuddy": "WorkBuddy 国内版",
    "workbuddy_intl": "WorkBuddy 国际版",
    "zcode": "ZCode（BigModel）",
    "zcode_zai": "ZCode（z.ai 国际站）",
}


class AddAccountDialog(QDialog):
    def __init__(self, win, platform: str = "workbuddy"):
        super().__init__(win)
        self.win = win
        self.setWindowTitle("添加账号")
        self.setMinimumSize(760, 620)
        self.setModal(True)

        self._flow = None
        self._stop = threading.Event()
        self._gen = 0

        root = QVBoxLayout(self)
        root.setContentsMargins(24, 20, 24, 20)
        root.setSpacing(12)

        title = label("添加账号", "PageTitle")
        root.addWidget(title)
        root.addWidget(label("用平台官方登录页授权，应用自动接收登录结果，"
                             "token 以 Windows DPAPI 加密保存在本机。", "PageSub"))

        row = QHBoxLayout()
        row.setSpacing(8)
        row.addWidget(label("平台", "StatLabel"))
        self.platform = QComboBox()
        self.platform.setFixedWidth(170)
        for key in ORDER:
            self.platform.addItem(COMBO_LABELS[key], key)
        self.platform.setCurrentIndex(ORDER.index(platform) if platform in ORDER else 2)
        self.platform.currentIndexChanged.connect(self._on_platform)
        row.addWidget(self.platform)
        row.addSpacing(4)
        self.start_btn = make_button("开始登录", "primary", "key")
        self.start_btn.clicked.connect(self._start)
        row.addWidget(self.start_btn)
        self.close_btn = make_button("关闭", "ghost")
        self.close_btn.clicked.connect(self.reject)
        row.addStretch(1)
        row.addWidget(self.close_btn)
        root.addLayout(row)

        self.hint = label("", "SectionHint")
        self.hint.setWordWrap(True)
        root.addWidget(self.hint)

        url_row = QHBoxLayout()
        url_row.setSpacing(8)
        url_row.addWidget(label("登录链接", "StatLabel"))
        self.url = QLineEdit()
        self.url.setReadOnly(True)
        self.url.setPlaceholderText("点「开始登录」后生成")
        url_row.addWidget(self.url, 1)
        self.copy_btn = make_button("复制", "ghost")
        self.copy_btn.setEnabled(False)
        self.copy_btn.clicked.connect(self._copy)
        url_row.addWidget(self.copy_btn)
        self.open_btn = make_button("浏览器打开", "ghost", "external")
        self.open_btn.setEnabled(False)
        self.open_btn.clicked.connect(self._open)
        url_row.addWidget(self.open_btn)
        root.addLayout(url_row)

        self.status = label("选择平台后点「开始登录」", "SectionHint")
        root.addWidget(self.status)
        self.progress = Spinner(self, 20)
        policy = self.progress.sizePolicy()
        policy.setRetainSizeWhenHidden(False)
        self.progress.setSizePolicy(policy)
        self.progress.setVisible(False)
        root.addWidget(self.progress, 0, Qt.AlignmentFlag.AlignLeft)

        root.addWidget(hline())
        list_head = QHBoxLayout()
        list_head.addWidget(label("已通过浏览器登录的账号", "SectionTitle"))
        list_head.addStretch(1)
        self.remove_btn = make_button("移除选中", "danger-ghost")
        self.remove_btn.clicked.connect(self._remove)
        list_head.addWidget(self.remove_btn)
        root.addLayout(list_head)

        columns = [
            Column("platform", "平台", 110),
            Column("name", "账号", 260, stretch=True),
            Column("expire", "登录态到期", 120),
            Column("switch", "可写回客户端", 110, align="center"),
        ]
        self.table = DataTable(columns, key="id", multi=False, parent=self)
        root.addWidget(self.table, 1)

        self._on_platform()
        self._reload()

    # ------------------------------------------------------------- 交互
    def _key(self) -> str:
        return self.platform.currentData() or "workbuddy"

    def _on_platform(self) -> None:
        if self._flow:
            self._cancel(silent=True)
        self.hint.setText(HINTS[self._key()])

    def _start(self) -> None:
        self._cancel(silent=True)
        self._gen += 1
        generation = self._gen
        self._stop = threading.Event()
        self.url.clear()
        self.status.setText("正在生成登录链接…")
        self.progress.setVisible(True)
        self.start_btn.setEnabled(False)
        self.copy_btn.setEnabled(False)
        self.open_btn.setEnabled(False)
        key = self._key()
        platform, variant = {"zcode": ("zcode", "bigmodel"),
                             "zcode_zai": ("zcode", "zai")}.get(key, (key, ""))

        tasks.submit(
            lambda emit: logins.create_login(platform, variant),
            on_done=lambda flow: self._flow_ready(generation, flow),
            on_error=lambda err: self._failed(generation, err.splitlines()[0]),
        serial=True )

    def _flow_ready(self, generation: int, flow) -> None:
        if generation != self._gen or not self.isVisible():
            flow.cancel()
            return
        self._flow = flow
        self.url.setText(flow.url)
        self.copy_btn.setEnabled(True)
        self.open_btn.setEnabled(True)
        self.status.setText("等待浏览器完成登录…")
        QDesktopServices.openUrl(QUrl(flow.url))
        stop = self._stop
        tasks.submit(
            lambda emit: flow.poll(stop),
            on_done=lambda account: self._success(generation, account),
            on_error=lambda err: self._failed(generation, err.splitlines()[0]),
        serial=True )

    def _success(self, generation: int, account: Account) -> None:
        if generation != self._gen:
            return
        item = store.to_item(account)
        # 重新登录同一个账号时不要把原来的名字覆盖成通用占位名
        existing = store.find(account.platform, account.account_id)
        placeholder = {"", "ZCode 账号", account.platform_label}
        if existing and str(existing.get("name") or "").strip() \
                and str(item.get("name") or "") in placeholder:
            item["name"] = existing["name"]
        store.upsert(item)
        self._flow = None
        self.progress.setVisible(False)
        self.start_btn.setEnabled(True)
        self.status.setText(f"登录成功：{account.name}（{account.platform_label}）")
        self.win.log(f"浏览器登录成功：{account.platform_label} / {account.name}", "ok")
        self.win.notify(f"已添加 {account.name}", "ok")
        self._reload()
        bus.BUS.data_changed.emit("checkin")

    def _failed(self, generation: int, message: str) -> None:
        if generation != self._gen:
            return
        self._flow = None
        self.progress.setVisible(False)
        self.start_btn.setEnabled(True)
        if message.strip() == "已取消":
            self.status.setText("已取消，可重新开始")
            return
        self.status.setText(f"登录失败：{message}")
        self.win.log(f"浏览器登录失败：{message}", "error")

    def _cancel(self, silent: bool = False) -> None:
        self._stop.set()
        if self._flow:
            try:
                self._flow.cancel()
            except Exception:
                pass
            self._flow = None
        self.progress.setVisible(False)
        self.start_btn.setEnabled(True)
        if not silent:
            self.status.setText("已取消，可重新开始")

    def _copy(self) -> None:
        text = self.url.text().strip()
        if text:
            self.win.clipboard().setText(text)
            self.status.setText("登录链接已复制，可以粘贴到浏览器打开")

    def _open(self) -> None:
        text = self.url.text().strip()
        if text:
            QDesktopServices.openUrl(QUrl(text))

    # ------------------------------------------------------------- 账号列表
    def _reload(self) -> None:
        rows = []
        for item in store.load():
            platform = str(item.get("platform") or "")
            account_id = str(item.get("account_id") or "")
            meta = item.get("meta") or {}
            plat_label = LABELS.get(platform, platform)
            if platform == "workbuddy" and meta.get("edition") == "intl":
                plat_label = "WorkBuddy 国际版"
            expires = str(item.get("expires_at") or "")
            expires = expires[:10] if expires else "—"
            rows.append({
                "id": f"{platform}|{account_id}",
                "platform": plat_label,
                "name": str(item.get("name") or account_id),
                "expire": expires,
                "switch": "支持" if platform in ("qoder", "workbuddy", "zcode") else "不支持",
                "_fg_map": {} if platform in ("qoder", "workbuddy", "zcode")
                else {"switch": PALETTE["muted"]},
            })
        self.table.set_rows(rows)

    def _remove(self) -> None:
        row = self.table.current_row()
        if not row:
            self.win.info("移除账号", "请先在列表里选中一条记录。")
            return
        platform, _, account_id = str(row["id"]).partition("|")
        if not self.win.confirm("移除账号",
                                "从本工具移除这个浏览器登录账号？\n不会影响官方客户端的登录状态。"):
            return
        store.remove(platform, account_id)
        self._reload()
        self.win.notify("已移除账号", "info")
        bus.BUS.data_changed.emit("checkin")

    def done(self, result: int) -> None:  # noqa: D102 - 关闭时一定要停掉轮询
        self._stop.set()
        if self._flow:
            try:
                self._flow.cancel()
            except Exception:
                pass
            self._flow = None
        super().done(result)
