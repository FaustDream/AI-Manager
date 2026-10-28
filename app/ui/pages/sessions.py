# -*- coding: utf-8 -*-
"""会话管理：四个来源的会话汇总、预览、同步、删除。"""
from __future__ import annotations

import os
from pathlib import Path

from PySide6.QtCore import Qt, QTimer
from PySide6.QtWidgets import (QCheckBox, QComboBox, QFrame, QHBoxLayout, QInputDialog,
                               QLineEdit, QSplitter, QTextBrowser, QVBoxLayout, QWidget)

from ...core import bus, tasks, winproc
from ...core.format import fmt_size, fmt_time
from ...sessions import service
from ...sessions.editions import Edition
from ..models import Column
from ..theme import PALETTE
from ..widgets import Badge, Card, DataTable, hline, label, make_button
from .base import BasePage

COLUMNS = [
    Column("title", "会话标题", 320, stretch=True),
    Column("source", "软件", 130),
    Column("account", "账号", 150),
    Column("state", "状态", 68, "center"),
    Column("last", "最后活动", 118, "center"),
    Column("msgs", "记录", 56, "right"),
    Column("size", "大小", 76, "right"),
]

SOURCES = ["全部", "WorkBuddy 国内版", "WorkBuddy 国际版", "CodeBuddy IDE", "IDEA 插件"]
DIRECTIONS = {"双向同步": "both", "国内版 → 国际版": "cn2intl", "国际版 → 国内版": "intl2cn"}
DIRECTION_LABEL = {v: k for k, v in DIRECTIONS.items()}


class SessionsPage(BasePage):
    key = "sessions"
    title = "会话管理"
    subtitle = ""

    def __init__(self, win):
        super().__init__(win)
        self.snap = None
        self.rows: list[dict] = []
        self.row_index: dict[str, dict] = {}
        self.targets: list[dict] = []
        self._target_map: dict[str, dict] = {}
        self._first_load = True
        self._labels: dict[str, str] = {}
        self._known_uids: tuple[str, str] = ("", "")
        self._build()
        self._load_options()

        # 账号切换检测：客户端换号后 uid 会变，变了就自动重拉（含云端会话列表）
        self._uid_timer = QTimer(self)
        self._uid_timer.setInterval(30_000)
        self._uid_timer.timeout.connect(self._check_account_change)
        self._uid_timer.start()

    # ------------------------------------------------------------------ 界面
    def _build(self) -> None:
        self.btn_refresh = make_button("刷新", "ghost", "refresh")
        self.btn_refresh.clicked.connect(self.refresh)
        self.header.add_action(self.btn_refresh)
        self.btn_preview = make_button("预览计划", "ghost")
        self.btn_preview.clicked.connect(self.preview)
        self.header.add_action(self.btn_preview)
        self.btn_sync = make_button("一键同步", "primary", "sync")
        self.btn_sync.clicked.connect(self.sync_all)
        self.header.add_action(self.btn_sync)

        filters = Card(padding=14, spacing=9)
        line1 = QHBoxLayout()
        line1.setSpacing(8)
        line1.addWidget(label("软件", "StatLabel"))
        self.f_source = QComboBox()
        self.f_source.addItems(SOURCES)
        self.f_source.setFixedWidth(148)
        self.f_source.currentIndexChanged.connect(lambda _i: self._fill_table())
        line1.addWidget(self.f_source)
        line1.addWidget(label("账号", "StatLabel"))
        self.f_account = QComboBox()
        self.f_account.setFixedWidth(190)
        self.f_account.currentIndexChanged.connect(lambda _i: self._fill_table())
        line1.addWidget(self.f_account)
        line1.addWidget(label("搜索", "StatLabel"))
        self.search = QLineEdit()
        self.search.setPlaceholderText("标题或会话 ID")
        self.search.setFixedWidth(200)
        # 三百多行每敲一个字就整表重建会明显发涩，加 180ms 防抖
        self._search_timer = QTimer(self)
        self._search_timer.setSingleShot(True)
        self._search_timer.setInterval(180)
        self._search_timer.timeout.connect(self._fill_table)
        self.search.textChanged.connect(lambda _t: self._search_timer.start())
        line1.addWidget(self.search)
        self.opt_body = QCheckBox("只看有正文")
        self.opt_body.stateChanged.connect(self._on_opt_change)
        line1.addWidget(self.opt_body)
        line1.addStretch(1)
        self.pending_badge = Badge("待同步 —", "accent")
        line1.addWidget(self.pending_badge)
        filters.body.addLayout(line1)

        line2 = QHBoxLayout()
        line2.setSpacing(8)
        line2.addWidget(label("方向", "StatLabel"))
        self.direction = QComboBox()
        self.direction.addItems(list(DIRECTIONS.keys()))
        self.direction.setFixedWidth(140)
        self.direction.currentTextChanged.connect(lambda _t: self._save_options())
        line2.addWidget(self.direction)
        self.opt_restore = QCheckBox("恢复可见")
        self.opt_deleted = QCheckBox("含已删除")
        self.opt_restart = QCheckBox("完成后重启客户端")
        for box in (self.opt_restore, self.opt_deleted, self.opt_restart):
            box.stateChanged.connect(self._on_opt_change)
            line2.addWidget(box)
        line2.addStretch(1)
        self.btn_delete_all = make_button("清空全部会话", "danger-ghost")
        self.btn_delete_all.clicked.connect(self.delete_all)
        line2.addWidget(self.btn_delete_all)
        filters.body.addLayout(line2)
        self.body.addWidget(filters)

        splitter = QSplitter(Qt.Orientation.Horizontal)
        splitter.setChildrenCollapsible(False)

        table_card = QFrame()
        table_card.setObjectName("Card")
        table_layout = QVBoxLayout(table_card)
        table_layout.setContentsMargins(0, 0, 0, 0)
        table_layout.setSpacing(0)
        head = QHBoxLayout()
        head.setContentsMargins(16, 12, 16, 10)
        head.setSpacing(8)
        head.addWidget(label("会话列表", "SectionTitle"))
        self.list_hint = label("", "SectionHint")
        head.addWidget(self.list_hint)
        head.addStretch(1)
        self.btn_delete = make_button("删除选中", "danger-ghost")
        self.btn_delete.clicked.connect(self.delete_selected)
        head.addWidget(self.btn_delete)
        table_layout.addLayout(head)
        table_layout.addWidget(hline())
        self.table = DataTable(COLUMNS, key="rowid", parent=self)
        self.table.selectionModel().selectionChanged.connect(lambda *_: self._on_select())
        self.table.doubleClicked.connect(lambda _i: self.open_session_dir())
        table_layout.addWidget(self.table, 1)
        splitter.addWidget(table_card)

        detail_card = QFrame()
        detail_card.setObjectName("Card")
        detail_layout = QVBoxLayout(detail_card)
        detail_layout.setContentsMargins(16, 14, 16, 14)
        detail_layout.setSpacing(10)
        detail_layout.addWidget(label("会话详情", "SectionTitle"))
        detail_layout.addWidget(hline())

        self.detail = QTextBrowser()
        self.detail.setOpenExternalLinks(False)
        self.detail.setFrameShape(QFrame.Shape.NoFrame)
        self.detail.setStyleSheet(
            f"QTextBrowser {{ background: transparent; border: none; color: {PALETTE['text']}; }}")
        detail_layout.addWidget(self.detail, 1)

        target_row = QHBoxLayout()
        target_row.setSpacing(6)
        target_row.addWidget(label("同步到", "StatLabel"))
        self.target = QComboBox()
        self.target.setMinimumWidth(150)
        target_row.addWidget(self.target, 1)
        detail_layout.addLayout(target_row)

        self.btn_sync_one = make_button("同步选中会话", "primary", "sync")
        self.btn_sync_one.clicked.connect(self.sync_selected)
        detail_layout.addWidget(self.btn_sync_one)

        actions = QHBoxLayout()
        actions.setSpacing(6)
        for text, handler in (("复制 ID", self.copy_id), ("打开目录", self.open_session_dir)):
            button = make_button(text, "ghost")
            button.clicked.connect(handler)
            actions.addWidget(button)
        actions.addStretch(1)
        delete_button = make_button("删除", "danger-ghost")
        delete_button.clicked.connect(self.delete_selected)
        actions.addWidget(delete_button)
        detail_layout.addLayout(actions)

        detail_card.setMinimumWidth(330)
        detail_card.setMaximumWidth(420)
        splitter.addWidget(detail_card)
        splitter.setStretchFactor(0, 1)
        splitter.setStretchFactor(1, 0)
        splitter.setSizes([820, 360])
        self.body.addWidget(splitter, 1)

        self._show_placeholder()

    # ------------------------------------------------------------------ 选项
    def _load_options(self) -> None:
        opts = self.cfg.get("session_opts") or {}
        self.direction.setCurrentText(
            DIRECTION_LABEL.get(opts.get("direction", "both"), "双向同步"))
        self.opt_restore.setChecked(bool(opts.get("restore", True)))
        self.opt_deleted.setChecked(bool(opts.get("include_deleted", True)))
        self.opt_restart.setChecked(bool(opts.get("restart", True)))
        self.opt_body.setChecked(bool(opts.get("only_body", False)))

    def _save_options(self) -> None:
        self.cfg["session_opts"] = {
            "direction": DIRECTIONS.get(self.direction.currentText(), "both"),
            "restore": self.opt_restore.isChecked(),
            "include_deleted": self.opt_deleted.isChecked(),
            "restart": self.opt_restart.isChecked(),
            "only_body": self.opt_body.isChecked(),
        }
        self.win.save_cfg()

    def _on_opt_change(self, *_args) -> None:
        self._save_options()
        if self.snap is not None:
            self._fill_table()

    # ------------------------------------------------------------------ 加载
    def on_show(self) -> None:
        if self._first_load:
            self._first_load = False
            self.refresh()

    def on_data_changed(self, scope: str) -> None:
        if scope == "sessions":
            self.refresh()

    def refresh(self) -> None:
        if self.busy:
            return
        self._set_busy(True, "正在读取四个来源的会话…")
        include_deleted = self.opt_deleted.isChecked()
        tasks.submit(
            lambda emit: service.build_snapshot(include_deleted, with_cloud=True,
                                                log=lambda m, lv="info": self.log(m, lv)),
            on_done=self._apply_snapshot, on_error=self._load_failed)

    def _check_account_change(self) -> None:
        """定时对比客户端当前登录 uid，变了（换号了）就自动刷新列表。"""
        cn_uid = Edition("cn").current_uid()[0]
        intl_uid = Edition("intl").current_uid()[0]
        current = (cn_uid, intl_uid)
        if self._known_uids == ("", ""):
            self._known_uids = current
            return
        if current != self._known_uids:
            self._known_uids = current
            self.log("检测到客户端账号变化，自动刷新会话列表（含云端）", "dim")
            if not self.busy:
                self.refresh()

    def _load_failed(self, message: str) -> None:
        self._set_busy(False)
        self.log(message, "error")
        self.notify("读取会话失败，详情见运行日志", "error")

    def _apply_snapshot(self, snap) -> None:
        self.snap = snap
        self._labels = {}
        self._set_busy(False)
        counts = {}
        for row in snap.unified:
            counts[row["source_label"]] = counts.get(row["source_label"], 0) + 1
        summary = " · ".join(f"{name} {counts.get(name, 0)}" for name in SOURCES[1:])
        self.header.set_subtitle(
            f"{summary}　|　合计 {len(snap.unified)} 条，有正文 "
            f"{sum(1 for r in snap.unified if r['has_body'])} 条　|　"
            f"国内版可见 {snap.cn_visible} / 国际版可见 {snap.intl_visible}")
        restore_total = sum(len(rows) for rows in snap.restores.values())
        self.pending_badge.setText(f"待同步 {snap.pending} · 待恢复 {restore_total}")
        self.pending_badge.set_tone("accent" if snap.pending or restore_total else "muted")
        self.win.set_summary(
            "sessions", f"{len(snap.unified)} 条会话 · 待同步 {snap.pending} 条")

        accounts = ["全部", "当前账号"] + sorted({self._account_label(row["account"])
                                                  for row in snap.unified
                                                  if self._account_label(row["account"]) != "当前账号"})
        current = self.f_account.currentText()
        self.f_account.blockSignals(True)
        self.f_account.clear()
        self.f_account.addItems(accounts)
        # 默认只看当前账号的会话；旧账号的切换到「全部」或按账号筛选才出现
        self.f_account.setCurrentText(
            current if current in accounts else "当前账号")
        self.f_account.blockSignals(False)

        self._fill_targets()
        self._fill_table()
        self._show_placeholder()
        self.log(f"刷新完成：WorkBuddy 国内 {snap.cn_total} / 国际 {snap.intl_total}，"
                 f"CodeBuddy {len(snap.cb_sessions)} 条，合并 {len(snap.unified)} 条", "dim")

    # ------------------------------------------------------------------ 表格
    def _account_label(self, uid: str) -> str:
        label_text = self._labels.get(uid)
        if label_text is None:
            label_text = self._build_account_label(uid)
            self._labels[uid] = label_text
        return label_text

    def _build_account_label(self, uid: str) -> str:
        if not uid or uid == "default":
            return "未登录"
        names = {}
        if self.snap:
            names[self.snap.cn_uid] = "国内版"
            names[self.snap.intl_uid] = "国际版"
        tag = names.get(uid)
        short = uid[:8]
        return f"{short}（{tag}）" if tag else f"{short}（旧账号）"

    def _passes(self, row: dict) -> bool:
        if self.f_source.currentText() != "全部" \
                and row["source_label"] != self.f_source.currentText():
            return False
        account = self.f_account.currentText()
        if account == "当前账号":
            # 旧账号的会话保留本地但不进当前账号列表；无主（default/空）视为当前
            if row.get("state") == "旧账号":
                return False
        elif account != "全部" and self._account_label(row["account"]) != account:
            return False
        if self.opt_body.isChecked() and not row["has_body"]:
            return False
        query = self.search.text().strip().lower()
        if query and query not in str(row["title"]).lower() and query not in row["id"].lower():
            return False
        return True

    def _fill_table(self) -> None:
        if not self.snap:
            return
        selected = {row["rowid"] for row in self.table.selected_rows()}
        self.rows = [row for row in self.snap.unified if self._passes(row)]
        display = []
        self.row_index = {}
        for index, row in enumerate(self.rows):
            rowid = f'{row["source_key"]}|{row["account"]}|{row["id"]}|{index}'
            row["rowid"] = rowid
            self.row_index[rowid] = row
            display.append({
                "rowid": rowid,
                "title": row["title"],
                "source": row["source_label"],
                "account": self._account_label(row["account"]),
                "state": row["state"],
                "last": fmt_time(row["last"]),
                "msgs": row["msgs"] or "—",
                "size": fmt_size(row["size"]) if row["size"] else "—",
                "_fg_map": {"state": self._state_color(row["state"])},
                "_tip_map": {"title": row["title"]},
            })
        self.table.set_rows(display)
        self.list_hint.setText(f"显示 {len(display)} / {len(self.snap.unified)} 条")
        keep = [rowid for rowid in selected if rowid in self.row_index]
        if keep:
            self.table.select_key(keep[0])

    @staticmethod
    def _state_color(state: str) -> str:
        if state in ("可见", "有正文"):
            return PALETTE["ok"]
        if state == "已删除":
            return PALETTE["danger"]
        if state in ("旧账号", "无正文"):
            return PALETTE["warn"]
        if state == "云端":
            return PALETTE["accent"]
        return PALETTE["muted"]

    def _fill_targets(self) -> None:
        self.targets = service.all_targets(self.snap) if self.snap else []
        self._target_map = {item["label"]: item for item in self.targets}
        current = self.target.currentText()
        self.target.blockSignals(True)
        self.target.clear()
        self.target.addItems([item["label"] for item in self.targets])
        if current in self._target_map:
            self.target.setCurrentText(current)
        self.target.blockSignals(False)

    def _show_placeholder(self) -> None:
        self.detail.setHtml("")

    # ------------------------------------------------------------------ 详情
    def _on_select(self) -> None:
        rows = self.table.selected_rows()
        if not rows:
            return
        row = self.row_index.get(rows[0]["rowid"])
        if not row:
            return
        self._render_detail(row)

    def _render_detail(self, row: dict) -> None:
        pairs = [
            ("标题", row["title"]),
            ("软件", row["source_label"]),
            ("账号", self._account_label(row["account"])),
            ("状态", row["state"]),
            ("会话 ID", row["id"]),
            ("最后活动", fmt_time(row["last"])),
        ]
        if row["kind"] == "workbuddy":
            pairs.append(("消息记录", f"{row['msgs']} 条"))
            pairs.append(("占用大小", fmt_size(row["size"])))
            if row.get("cwd"):
                pairs.append(("工作目录", row["cwd"]))
            if row.get("client_status"):
                pairs.append(("客户端状态", row["client_status"]))
            if row.get("state") == "云端":
                body = "（云端会话：正文在服务端，打开对应客户端登录该账号即可继续对话）"
            else:
                body = "\n".join(row["files"]) if row["files"] else "（无正文文件）"
        else:
            pairs.append(("消息文件", f"{row['msgs']} 个"))
            pairs.append(("占用大小", fmt_size(row["size"])))
            pairs.append(("工作区", row["raw"]["workspace"][:12] if row.get("raw") else ""))
            body = row["path"] or "（无正文目录）"

        html = ['<div style="font-size:12px;line-height:1.7">']
        for key, value in pairs:
            html.append(
                f'<div style="color:{PALETTE["muted"]};font-size:10px;margin-top:8px">{key}</div>'
                f'<div style="color:{PALETTE["text"]};word-wrap:break-word">{value}</div>')
        html.append(
            f'<div style="color:{PALETTE["muted"]};font-size:10px;margin-top:10px">正文位置</div>'
            f'<div style="color:{PALETTE["text_2"]};font-size:11px;word-wrap:break-word">'
            f'{body}</div></div>')
        self.detail.setHtml("".join(html))

        kinds = {row["kind"]}
        self.btn_sync_one.setEnabled(bool(kinds))
        self._preferred_target(row)

    def _preferred_target(self, row: dict) -> None:
        options = [item for item in self.targets if item["kind"] == row["kind"]]
        if not options:
            return
        labels = [item["label"] for item in options]
        self.target.blockSignals(True)
        self.target.clear()
        self.target.addItems(labels)
        self.target.blockSignals(False)
        if row["kind"] == "workbuddy":
            for item in options:
                if item["key"] != row["source_key"]:
                    self.target.setCurrentText(item["label"])
                    return
        else:
            for item in options:
                if item["key"] != (row["source_key"], row["account"]):
                    self.target.setCurrentText(item["label"])
                    return
        self.target.setCurrentIndex(0)

    # ------------------------------------------------------------------ 动作
    def preview(self) -> None:
        if self.busy:
            return
        mode = DIRECTIONS.get(self.direction.currentText(), "both")
        include_deleted = self.opt_deleted.isChecked()
        self._set_busy(True, "正在生成同步计划…")

        def worker(emit):
            lines = []
            total = 0
            for src_key, dst_key in service.directions(mode):
                src, dst = Edition(src_key), Edition(dst_key)
                if not (src.exists and dst.exists):
                    lines.append(f"{src.label} 或 {dst.label} 数据目录不存在，跳过")
                    continue
                from ...sessions import wbdb
                items, skipped = wbdb.build_plan(src, dst, include_deleted, True)
                total += len(items)
                lines.append(f"【{src.label} → {dst.label}】{len(items)} 条")
                for item in items[:12]:
                    action = {"insert": "新增", "update": "更新", "resurrect": "恢复"}[item.action]
                    lines.append(f"   {action}  {item.title or '(无标题)'} "
                                 f"[{fmt_size(item.size)}]")
                if len(items) > 12:
                    lines.append(f"   … 其余 {len(items) - 12} 条")
                for row, why in skipped[:6]:
                    lines.append(f"   跳过 {row.get('title') or row['id']}：{why}")
            return total, lines

        def done(payload):
            self._set_busy(False)
            total, lines = payload
            for line in lines:
                self.log(line)
            restore_hint = ""
            if self.snap:
                restore_hint = ("\n本机恢复：" + "，".join(
                    f"{Edition(key).short} {len(rows)} 条"
                    for key, rows in self.snap.restores.items())) \
                    if self.snap.restores and self.opt_restore.isChecked() else ""
            self.log(f"预览：跨版本待同步 {total} 条{restore_hint}", "ok")
            self.info("同步计划",
                      f"跨版本待同步 {total} 条{restore_hint}\n\n详细清单见下方运行日志。")

        tasks.submit(worker, on_done=done, on_error=self._load_failed)

    def sync_all(self) -> None:
        if self.busy:
            return
        if not self.snap:
            self.info("请稍候", "会话数据还在读取，请稍后再试。")
            return
        mode = DIRECTIONS.get(self.direction.currentText(), "both")
        include_deleted = self.opt_deleted.isChecked()
        do_restore = self.opt_restore.isChecked()
        restart = self.opt_restart.isChecked()
        total = self.snap.pending
        restore_total = sum(len(rows) for rows in self.snap.restores.values()) \
            if do_restore else 0
        if total == 0 and restore_total == 0:
            self.info("无需同步", "两边已经是同一份数据，没有需要搬运或恢复的会话。")
            return
        if not self.confirm(
                "确认同步",
                f"将同步 {total} 条会话、恢复 {restore_total} 条本机会话。\n"
                "同步前会自动备份两个版本的数据库与将被覆盖的正文，可随时回滚。\n\n继续吗？"):
            return

        running = [ep for ep in (Edition("cn"), Edition("intl"))
                   if winproc.process_running(ep.proc)]
        if running:
            names = "、".join(ep.label for ep in running)
            if not self.confirm(
                    "需要关闭客户端",
                    f"{names} 正在运行，写入前必须关闭，否则内存缓存会覆盖改动。\n\n"
                    "现在自动关闭并在结束后重新打开吗？"):
                return

        self._set_busy(True, "正在同步…")

        def worker(emit):
            return service.perform_sync(mode, include_deleted, do_restore,
                                        lambda m, lv="info": self.log(m, lv), restart)

        def done(stats):
            self._set_busy(False)
            if stats.get("skipped"):
                self.info("无需同步", "两边已经是同一份数据。")
                return
            text = (f"搬运 {stats['moved']} 条，恢复 {stats['restored']} 条，"
                    f"失败 {stats['failed']} 次")
            self.log(f"同步完成：{text} · 备份 {stats['tag']}", "ok")
            self.notify(f"同步完成：{text}")
            bus.BUS.data_changed.emit("sessions")
            self.refresh()

        tasks.submit(worker, on_done=done, on_error=self._sync_failed)

    def _sync_failed(self, message: str) -> None:
        self._set_busy(False)
        self.log(message, "error")
        self.error("同步失败", message.splitlines()[0])

    def sync_selected(self) -> None:
        if self.busy or not self.snap:
            return
        rows = [self.row_index[r["rowid"]] for r in self.table.selected_rows()
                if r["rowid"] in self.row_index]
        if not rows:
            self.info("未选择", "请先在列表里选中要同步的会话（可 Ctrl 多选）。")
            return
        kinds = {row["kind"] for row in rows}
        if len(kinds) != 1:
            self.info("混合选择",
                      "WorkBuddy 与 CodeBuddy 的会话不能放在一次同步里，请分两次选。")
            return
        kind = kinds.pop()
        target = self._target_map.get(self.target.currentText())
        if not target:
            self.info("选择目标", "请先在「同步到」里选择目标软件和账号。")
            return
        if kind == "workbuddy":
            todo = [row for row in rows if row["source_key"] != target["key"]]
            target_desc = target["label"]
        else:
            todo = [row for row in rows
                    if (row["source_key"], row["account"]) != target["key"]]
            target_desc = target["label"]
        if not todo:
            self.info("无需同步", "选中的会话已经在目标里了。")
            return
        if not self.confirm("确认同步",
                            f"将把 {len(todo)} 条会话写入 {target_desc}。\n"
                            + ("WorkBuddy 写入数据库前需要关闭对应客户端。\n\n"
                               if kind == "workbuddy" else "目标端需要重开 IDE 或插件才能看到。\n\n")
                            + "继续吗？"):
            return

        self._set_busy(True, "正在同步选中会话…")

        def worker(emit):
            return service.copy_selected(todo, kind, target["key"],
                                         lambda m, lv="info": self.log(m, lv))

        def done(stats):
            self._set_busy(False)
            text = f"成功 {stats['ok']}，跳过 {stats['skip']}，失败 {stats['fail']}"
            self.log(f"同步选中完成：{text}", "ok" if not stats["fail"] else "error")
            self.notify(f"同步完成：{text}")
            bus.BUS.data_changed.emit("sessions")
            self.refresh()

        tasks.submit(worker, on_done=done, on_error=self._sync_failed)

    def delete_selected(self) -> None:
        if self.busy or not self.snap:
            return
        rows = [self.row_index[r["rowid"]] for r in self.table.selected_rows()
                if r["rowid"] in self.row_index]
        if not rows:
            self.info("未选择", "请先在列表里选中要删除的会话（可 Ctrl 多选）。")
            return
        if any(row["kind"] != "workbuddy" for row in rows):
            self.info("暂不支持",
                      "CodeBuddy 的会话删除未开放，这里只能删除 WorkBuddy 的会话。\n"
                      "先在「软件」筛选里选 WorkBuddy 再试。")
            return
        ids = list(dict.fromkeys(row["id"] for row in rows))
        self._delete_flow(ids, f"删除选中的 {len(ids)} 条会话")

    def delete_all(self) -> None:
        if self.busy or not self.snap:
            return
        ids = list({row["id"] for row in self.snap.rows})
        if not ids:
            self.info("没有会话", "两个版本都没有历史会话。")
            return
        text, ok = QInputDialog.getText(
            self, "清空全部会话",
            f"这会删除本机全部 {len(ids)} 条会话（国内版与国际版一起）。\n"
            "文件会移入回收站，但会话会进入黑名单，之后同步也不会再出现。\n\n"
            "请输入「删除」确认：")
        if not ok:
            return
        if text.strip() != "删除":
            self.info("已取消", "输入内容不匹配，未做任何删除。")
            return
        self._delete_flow(ids, "清空本机全部会话")

    def _delete_flow(self, ids: list[str], action: str) -> None:
        warn = ""
        if self.snap and self.snap.cloud_total:
            warn = (f"\n注意：检测到 {self.snap.cloud_total} 条云端会话，"
                    "云端数据无法由本工具删除，重新登录后可能再次出现。\n")
        if not self.confirm(
                "确认删除",
                f"{action}。\n\n"
                "将从国内版与国际版同时删除数据库记录、正文文件和任务文件。\n"
                "删除后加入黑名单，之后同步不会再把这些会话带回来。\n"
                f"文件先移入回收站，可在「备份与清理」里恢复。{warn}\n确定删除吗？"):
            return

        running = [ep for ep in (Edition("cn"), Edition("intl"))
                   if winproc.process_running(ep.proc)]
        if running and not self.confirm(
                "需要关闭客户端",
                "、".join(ep.label for ep in running)
                + " 正在运行，删除前必须关闭，否则客户端会把会话写回来。\n\n现在自动关闭吗？"):
            return

        self._set_busy(True, f"正在删除 {len(ids)} 条会话…")

        def worker(emit):
            return service.delete_ids(ids, lambda m, lv="info": self.log(m, lv))

        def done(stats):
            self._set_busy(False)
            self.log(f"已删除 {len(stats.get('ids') or ids)} 条会话并加入黑名单", "ok")
            self.notify(f"已删除 {len(ids)} 条会话")
            bus.BUS.data_changed.emit("sessions")
            self.refresh()

        tasks.submit(worker, on_done=done, on_error=self._sync_failed)

    # ------------------------------------------------------------------ 其他
    def copy_id(self) -> None:
        rows = self.table.selected_rows()
        if not rows:
            self.info("未选择", "请先在左侧选中一条会话。")
            return
        row = self.row_index.get(rows[0]["rowid"])
        if row:
            self.win.clipboard().setText(row["id"])
            self.notify("会话 ID 已复制")

    def open_session_dir(self) -> None:
        rows = self.table.selected_rows()
        if not rows:
            self.info("未选择", "请先在左侧选中一条会话。")
            return
        row = self.row_index.get(rows[0]["rowid"])
        if not row:
            return
        if row["kind"] == "codebuddy":
            path = Path(row["path"])
            if path.exists():
                os.startfile(str(path))
            else:
                self.info("目录不存在", "这条 CodeBuddy 会话没有正文目录。")
            return
        for item in row["files"] or []:
            parent = Path(item).parent
            if parent.exists():
                os.startfile(str(parent))
                return
        self.info("没有正文", "这条会话没有正文文件。")
