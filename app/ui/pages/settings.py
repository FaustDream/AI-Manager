# -*- coding: utf-8 -*-
"""设置：自动签到、数据目录、桌面快捷方式、关于与风险提示。"""
from __future__ import annotations

import os

from PySide6.QtCore import Qt
from PySide6.QtWidgets import (QCheckBox, QComboBox, QFrame, QHBoxLayout, QLabel,
                               QScrollArea, QVBoxLayout, QWidget)

from ... import APP_VERSION
from ...checkin import daily, service
from ...core import autostart, paths, shortcut, tasks
from ..theme import PALETTE
from ..widgets import Badge, Card, hline, label, make_button
from .base import BasePage


class SettingsPage(BasePage):
    key = "settings"
    title = "设置"
    subtitle = ""

    def __init__(self, win):
        super().__init__(win)
        self._build()
        self.reload()

    # ------------------------------------------------------------------ 界面
    def _build(self) -> None:
        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setFrameShape(QFrame.Shape.NoFrame)
        holder = QWidget()
        column = QVBoxLayout(holder)
        column.setContentsMargins(0, 0, 6, 0)
        column.setSpacing(12)

        # ---------------------------------------------------------- 签到
        daily_card = Card(padding=16, spacing=10)
        daily_card.add_title(
            "每日签到", f"启动时检查一次，之后每 {daily.INTERVAL_HOURS} 小时轮询")
        row = QHBoxLayout()
        row.setSpacing(10)
        row.addWidget(label("轮询机制", "StatLabel"))
        self.daily_badge = Badge("已启用", "ok")
        row.addWidget(self.daily_badge)
        self.daily_hint = label("", "SectionHint")
        self.daily_hint.setWordWrap(True)
        row.addWidget(self.daily_hint, 1)
        self.btn_selftest = make_button("运行只读自检", "ghost")
        self.btn_selftest.clicked.connect(self.selftest)
        row.addWidget(self.btn_selftest)
        daily_card.body.addLayout(row)

        note = label(
            "每轮顺序：确认已进入新的一天 → 校验各账号签到状态 → 只对可签的账号调用签到接口。"
            "同一账号当天签到成功后不再重复调用；接口给出下一轮开放时刻时，会等窗口开了再签。",
            "SectionHint")
        note.setWordWrap(True)
        daily_card.body.addWidget(note)
        daily_card.body.addWidget(hline())

        row2 = QHBoxLayout()
        row2.setSpacing(10)
        row2.addWidget(label("积分轮询间隔", "ValueLabel"))
        self.poll_combo = QComboBox()
        self.poll_combo.addItem("关闭", 0)
        self.poll_combo.addItem("2 分钟", 2)
        self.poll_combo.addItem("5 分钟", 5)
        self.poll_combo.addItem("10 分钟", 10)
        self.poll_combo.addItem("30 分钟", 30)
        self.poll_combo.currentIndexChanged.connect(self._save_poll)
        row2.addWidget(self.poll_combo)
        row2.addWidget(label("到点自动刷新各账号剩余积分与到期，不影响签到", "SectionHint"))
        row2.addStretch(1)
        daily_card.body.addLayout(row2)
        column.addWidget(daily_card)

        # ---------------------------------------------------------- 目录
        dirs = Card(padding=16, spacing=8)
        dirs.add_title("数据与目录", "整个 data 目录可以随程序一起拷走")
        for name, path, note in (
            ("数据目录", paths.DATA_DIR, "配置、账号库、积分台账、账号档案"),
            ("运行日志", paths.LOG_FILE, "按时间戳记录每次操作"),
            ("备份目录", paths.BACKUP_ROOT, "同步前的数据库与正文备份"),
            ("回收站", paths.TRASH_ROOT, "被删除会话的临时存放处"),
        ):
            dirs.body.addLayout(self._path_row(name, path, note))
        column.addWidget(dirs)

        # ---------------------------------------------------------- 启动
        launch = Card(padding=16, spacing=8)
        launch.add_title("启动方式", "不打包也能双击启动")
        row = QHBoxLayout()
        row.setSpacing(10)
        row.addWidget(label("桌面快捷方式", "StatLabel"))
        self.shortcut_badge = Badge("检测中", "muted")
        row.addWidget(self.shortcut_badge)
        self.shortcut_hint = label("", "SectionHint")
        row.addWidget(self.shortcut_hint)
        row.addStretch(1)
        self.btn_shortcut = make_button("创建 / 更新快捷方式", "primary", "external")
        self.btn_shortcut.clicked.connect(self.create_shortcut)
        row.addWidget(self.btn_shortcut)
        launch.body.addLayout(row)
        launch.body.addWidget(hline())

        boot = QHBoxLayout()
        boot.setSpacing(10)
        boot.addWidget(label("开机自启", "StatLabel"))
        self.boot_check = QCheckBox("登录 Windows 后自动启动，启动时执行一次签到轮询")
        self.boot_check.stateChanged.connect(self._save_autostart)
        boot.addWidget(self.boot_check)
        boot.addStretch(1)
        launch.body.addLayout(boot)
        launch.body.addWidget(hline())
        column.addWidget(launch)

        # ---------------------------------------------------------- 快捷操作
        quick = Card(padding=16, spacing=8)
        quick.add_title("快捷操作", "直接跳到常用页面，总览页不再放这些入口")
        row = QHBoxLayout()
        row.setSpacing(8)
        for text, icon_name, key in (
            ("签到中心", "checkin", "checkin"),
            ("会话管理", "sessions", "sessions"),
            ("账号档案", "profiles", "profiles"),
        ):
            button = make_button(text, "ghost", icon_name)
            button.clicked.connect(lambda _c=False, k=key: self.win.navigate(k))
            row.addWidget(button)
        row.addStretch(1)
        quick.body.addLayout(row)
        column.addWidget(quick)

        # ---------------------------------------------------------- 关于
        about = Card(padding=16, spacing=8)
        about.add_title(f"关于 AI Manager {APP_VERSION}")
        text = QLabel(
            "功能范围\n"
            "· 签到：Trae CN / Qoder CN / Qoder 国际版 / WorkBuddy 国内版 / WorkBuddy 国际版的"
            "本地登录态扫描、签到、剩余积分与到期统计、免签到、每日轮询签到、"
            "浏览器授权添加账号、写回客户端登录态。\n"
            "· 会话：WorkBuddy 国内版 ⇄ 国际版会话同步、CodeBuddy IDE ⇄ IDEA 插件历史同步、"
            "本机旧账号/已删除会话恢复可见、删除进回收站、黑名单、备份回滚、"
            "CodeBuddy 空壳清理、账号档案切换。\n\n"
            "风险提示\n"
            "· 自动化签到与客户端登录态写回可能违反各平台服务条款，请自行评估。\n"
            "· 客户端升级可能改变密钥或存储格式，届时相关功能需要更新。\n"
            "· 会话同步基于本地 SQLite/JSON 文件，写入前请保持客户端关闭，"
            "工具会先备份，但仍建议重要数据自行留档。"
        )
        text.setWordWrap(True)
        text.setStyleSheet(f"color: {PALETTE['text_2']}; font-size: 12px; line-height: 1.7;")
        about.body.addWidget(text)
        column.addWidget(about)
        column.addStretch(1)

        scroll.setWidget(holder)
        self.body.addWidget(scroll, 1)

    def _path_row(self, name: str, path, note: str) -> QHBoxLayout:
        row = QHBoxLayout()
        row.setSpacing(10)
        name_label = label(name, "StatLabel")
        name_label.setFixedWidth(72)
        row.addWidget(name_label)
        value = label(str(path), "ValueLabel")
        value.setTextInteractionFlags(Qt.TextInteractionFlag.TextSelectableByMouse)
        row.addWidget(value, 1)
        row.addWidget(label(note, "KeyLabel"))
        button = make_button("打开", "ghost", "folder")
        button.clicked.connect(lambda _c=False, p=path: self._open(p))
        row.addWidget(button)
        return row

    def _open(self, path) -> None:
        target = path if path.exists() else path.parent
        if not target.exists():
            self.warn("目录不存在", f"{target} 还没有生成。")
            return
        os.startfile(str(target))

    # ------------------------------------------------------------------ 数据
    def on_show(self) -> None:
        self.reload()

    def reload(self) -> None:
        self.daily_hint.setText(daily.describe())

        self.poll_combo.blockSignals(True)
        minutes = 5 if "credit_interval" not in self.cfg \
            else int(self.cfg.get("credit_interval") or 0)
        index = self.poll_combo.findData(minutes)
        self.poll_combo.setCurrentIndex(index if index >= 0 else 2)
        self.poll_combo.blockSignals(False)

        exists = shortcut.shortcut_exists()
        if exists:
            self.shortcut_badge.set("已创建", "ok")
            self.shortcut_hint.setText(str(exists))
        else:
            self.shortcut_badge.set("未创建", "warn")
            self.shortcut_hint.setText("点右侧按钮在桌面生成快捷方式，之后双击即可启动")

        self.boot_check.blockSignals(True)
        self.boot_check.setChecked(autostart.is_enabled())
        self.boot_check.blockSignals(False)

    def _save_poll(self, *_args) -> None:
        self.cfg["credit_interval"] = int(self.poll_combo.currentData() or 0)
        self.win.save_cfg()
        self.win.apply_credit_interval()
        self.log(f"积分轮询间隔：{self.poll_combo.currentText()}", "dim")

    def _save_autostart(self, *_args) -> None:
        enabled = self.boot_check.isChecked()
        try:
            command = autostart.enable() if enabled else autostart.disable()
        except OSError as exc:
            self.boot_check.blockSignals(True)
            self.boot_check.setChecked(not enabled)
            self.boot_check.blockSignals(False)
            self.error("开机自启", f"写入注册表失败：{exc}")
            return
        if enabled:
            self.log(f"已开启开机自启：{command}", "dim")
            self.info("已开启开机自启",
                      "下次登录 Windows 会自动启动，并立即执行一次签到轮询："
                      "确认进入新的一天 → 校验账号状态 → 对可签的账号签到。"
                      f"\n\n注册表命令：\n{command}")
        else:
            self.log("已关闭开机自启", "dim")

    # ------------------------------------------------------------------ 动作
    def create_shortcut(self) -> None:
        self._set_busy(True, "正在创建桌面快捷方式…")

        def worker(emit):
            return shortcut.create_desktop_shortcut()

        def done(path):
            self._set_busy(False)
            self.log(f"已创建桌面快捷方式：{path}", "ok")
            self.info("已创建", f"桌面快捷方式：\n{path}")
            self.reload()

        def failed(message: str):
            self._set_busy(False)
            self.log(message, "error")
            self.error("创建失败", message.splitlines()[0])

        tasks.submit(worker, on_done=done, on_error=failed)

    def selftest(self) -> None:
        self._set_busy(True, "正在自检…")

        def worker(emit):
            return service.selftest()

        def done(code):
            self._set_busy(False)
            self.log("自检完成，结果写入 data/logs/selftest.log",
                     "ok" if code == 0 else "error")
            self.info("自检完成",
                      ("全部通过。" if code == 0 else "存在失败项，详见 data/logs/selftest.log")
                      + "\n\n结果同时打印在运行日志与自检文件里。")

        def failed(message: str):
            self._set_busy(False)
            self.log(message, "error")
            self.error("自检失败", message.splitlines()[0])

        tasks.submit(worker, on_done=done, on_error=failed)
