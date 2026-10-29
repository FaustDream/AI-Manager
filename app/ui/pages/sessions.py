# -*- coding: utf-8 -*-
"""会话管理：本机会话的浏览、正文预览与删除。

不做跨版本同步——账号之间的会话恢复在账号中心完成，这一页只回答
「本机有哪些会话、里面写了什么、删哪些」。
"""
from __future__ import annotations

import html
import json
import os
from pathlib import Path

from PySide6.QtCore import Qt, QTimer
from PySide6.QtWidgets import (QCheckBox, QComboBox, QFrame, QHBoxLayout, QInputDialog,
                               QLineEdit, QSplitter, QTextBrowser, QVBoxLayout)

from ...core import bus, tasks, winproc
from ...core.format import fmt_size, fmt_time
from ...sessions import preview, service
from ...sessions.editions import Edition
from ..models import Column
from ..theme import PALETTE
from ..widgets import Card, DataTable, hline, label, make_button
from .base import BasePage

COLUMNS = [
    Column("title", "会话标题", 240, stretch=True),
    Column("source", "软件", 136),
    Column("account", "账号", 210),
    Column("state", "状态", 66, "center"),
    Column("last", "最后活动", 132, "center"),
    Column("msgs", "记录", 50, "right"),
    Column("size", "大小", 70, "right"),
]

SOURCES = ["全部", "WorkBuddy 国内版", "WorkBuddy 国际版", "CodeBuddy IDE", "IDEA 插件"]

# 表头第几列按哪个字段排序；文本列升序，时间/数量列默认降序
SORT_COLUMNS = {"title": 0, "source": 1, "account": 2, "state": 3,
                "last": 4, "msgs": 5, "size": 6}
COLUMN_SORT = {index: key for key, index in SORT_COLUMNS.items()}
NUMERIC_SORTS = ("last", "msgs", "size")
STATE_RANK = {"可见": 0, "有正文": 0, "云端": 1, "旧账号": 2, "无正文": 3, "已删除": 4}


class SessionsPage(BasePage):
    key = "sessions"
    title = "会话管理"
    subtitle = ""

    def __init__(self, win):
        super().__init__(win)
        # 页头（标题 + 统计）整体不要：入口在左侧导航已经写清楚了
        self.header.setVisible(False)
        self.cat: service.Catalog | None = None
        self.rows: list[dict] = []
        self.row_index: dict[str, dict] = {}
        self._labels: dict[str, str] = {}
        self._names: dict[str, str] = {}
        self._known_uids: tuple[str, str] = ("", "")
        self._preview_cache: dict[str, dict] = {}
        self._preview_token = 0
        self._first_load = True
        self._sort_key = "last"
        self._sort_desc = True
        self._build()
        self._load_options()

        # 账号切换检测：客户端换号后 uid 会变，变了就自动重拉
        self._uid_timer = QTimer(self)
        self._uid_timer.setInterval(30_000)
        self._uid_timer.timeout.connect(self._check_account_change)
        self._uid_timer.start()

    # ------------------------------------------------------------------ 界面
    def _build(self) -> None:
        filters = Card(padding=14, spacing=9)
        line = QHBoxLayout()
        line.setSpacing(8)
        line.addWidget(label("软件", "StatLabel"))
        self.f_source = QComboBox()
        self.f_source.addItems(SOURCES)
        self.f_source.setFixedWidth(148)
        self.f_source.currentIndexChanged.connect(lambda _i: self._fill_table())
        line.addWidget(self.f_source)
        line.addWidget(label("账号", "StatLabel"))
        self.f_account = QComboBox()
        self.f_account.setFixedWidth(190)
        self.f_account.currentIndexChanged.connect(lambda _i: self._fill_table())
        line.addWidget(self.f_account)
        line.addWidget(label("搜索", "StatLabel"))
        self.search = QLineEdit()
        self.search.setPlaceholderText("标题或会话 ID")
        self.search.setFixedWidth(200)
        # 三百多行每敲一个字就整表重建会明显发涩，加 180ms 防抖
        self._search_timer = QTimer(self)
        self._search_timer.setSingleShot(True)
        self._search_timer.setInterval(180)
        self._search_timer.timeout.connect(self._fill_table)
        self.search.textChanged.connect(lambda _t: self._search_timer.start())
        line.addWidget(self.search)
        self.opt_body = QCheckBox("只看有正文")
        self.opt_body.stateChanged.connect(self._on_opt_change)
        line.addWidget(self.opt_body)
        line.addStretch(1)
        self.btn_refresh = make_button("刷新", "ghost", "refresh")
        self.btn_refresh.clicked.connect(self.refresh)
        line.addWidget(self.btn_refresh)
        filters.body.addLayout(line)
        self.body.addWidget(filters)

        # 上下分栏：表格占满宽度才放得下七列，正文也才有足够横向空间阅读
        splitter = QSplitter(Qt.Orientation.Vertical)
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
        self.btn_delete_all = make_button("清空全部", "danger-ghost")
        self.btn_delete_all.clicked.connect(self.delete_all)
        head.addWidget(self.btn_delete_all)
        table_layout.addLayout(head)
        table_layout.addWidget(hline())
        self.table = DataTable(COLUMNS, key="rowid", parent=self)
        self.table.selectionModel().selectionChanged.connect(lambda *_: self._on_select())
        self.table.doubleClicked.connect(lambda _i: self.open_session_dir())
        header = self.table.horizontalHeader()
        header.setSectionsClickable(True)
        header.setSortIndicatorShown(True)
        header.sectionClicked.connect(self._on_header_click)
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

        actions = QHBoxLayout()
        actions.setSpacing(6)
        open_button = make_button("打开目录", "ghost")
        open_button.clicked.connect(self.open_session_dir)
        actions.addWidget(open_button)
        actions.addStretch(1)
        self.btn_delete_one = make_button("删除", "danger-ghost")
        self.btn_delete_one.clicked.connect(self.delete_selected)
        actions.addWidget(self.btn_delete_one)
        detail_layout.addLayout(actions)

        detail_card.setMinimumHeight(160)
        splitter.addWidget(detail_card)
        splitter.setStretchFactor(0, 1)
        splitter.setStretchFactor(1, 0)
        splitter.setSizes([360, 230])
        # 没选中会话时收起详情：列表是主界面，800px 高的窗口里行数比空面板重要
        detail_card.setVisible(False)
        self.detail_card = detail_card
        self.splitter = splitter
        self.body.addWidget(splitter, 1)

        self._show_placeholder()

    # ------------------------------------------------------------------ 选项
    def _load_options(self) -> None:
        opts = self.cfg.get("session_opts") or {}
        self.opt_body.setChecked(bool(opts.get("only_body", False)))
        self._sort_key = opts.get("sort") if opts.get("sort") in SORT_COLUMNS else "last"
        self._sort_desc = bool(opts.get("sort_desc", True))
        self._update_sort_indicator()

    def _save_options(self) -> None:
        self.cfg["session_opts"] = {
            "only_body": self.opt_body.isChecked(),
            "sort": self._sort_key,
            "sort_desc": self._sort_desc,
        }
        self.win.save_cfg()

    def _on_opt_change(self, *_args) -> None:
        self._save_options()
        if self.cat is not None:
            self._fill_table()

    def _update_sort_indicator(self) -> None:
        column = SORT_COLUMNS.get(self._sort_key, 4)
        order = (Qt.SortOrder.DescendingOrder if self._sort_desc
                 else Qt.SortOrder.AscendingOrder)
        self.table.horizontalHeader().setSortIndicator(column, order)

    def _on_header_click(self, index: int) -> None:
        key = COLUMN_SORT.get(index)
        if not key:
            return
        if key == self._sort_key:
            self._sort_desc = not self._sort_desc
        else:
            self._sort_key = key
            self._sort_desc = key in NUMERIC_SORTS
        self._update_sort_indicator()
        self._save_options()
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
        self._set_busy(True, "正在读取本机会话…")
        self.list_hint.setText("正在读取本机会话…")

        def worker(emit):
            cat = service.build_catalog(log=lambda m, lv="info": self.log(m, lv))
            return {"cat": cat, "names": self._account_names()}

        tasks.submit(worker, on_done=self._apply_catalog, on_error=self._load_failed)

    def _account_names(self) -> dict[str, str]:
        """uid → 昵称：会话表里只显示昵称，不把 uid 摆出来。

        两个来源：账号库（当前保存的账号）+ 账号档案里的 account-snapshot
        （已经不在账号库里、但会话还留着的旧账号，快照里存着昵称）。
        不用 checkin_service.collect：它只返回当前能读到登录态的账号。
        """
        names: dict[str, str] = {}
        try:
            from ...checkin import store as account_store
            for item in account_store.load():
                if item.get("platform") != "workbuddy":
                    continue
                meta = item.get("meta") or {}
                uid = str(meta.get("uid") or item.get("account_id") or "")
                name = str(item.get("name") or "").strip()
                if uid and name:
                    names[uid] = name
        except Exception as exc:
            self.log(f"读取账号库失败：{exc}", "dim")

        try:
            from ...core import paths
            for snap in paths.PROFILE_ROOT.glob("*/*/storage/skeleton/account-snapshot.json"):
                uid = snap.parents[2].name
                if uid in names:
                    continue
                try:
                    primary = json.loads(snap.read_text(encoding="utf-8")).get("primary") or {}
                except (OSError, ValueError):
                    continue
                nickname = str(primary.get("nickname") or "").strip()
                if nickname:
                    names[str(primary.get("uid") or uid)] = nickname
        except OSError as exc:
            self.log(f"读取账号档案失败：{exc}", "dim")
        return names

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
            self.log("检测到客户端账号变化，自动刷新会话列表", "dim")
            if not self.busy:
                self.refresh()

    def _load_failed(self, message: str) -> None:
        self._set_busy(False)
        self.list_hint.setText("读取失败，可点「刷新」重试")
        self.log(message, "error")
        self.notify("读取会话失败，详情见运行日志", "error")

    def _apply_catalog(self, payload) -> None:
        if isinstance(payload, dict):
            cat = payload.get("cat")
            self._names = payload.get("names") or {}
        else:
            cat = payload
            self._names = {}
        self.cat = cat
        self._labels = {}
        self._preview_cache = {}
        self._set_busy(False)

        accounts = ["全部"] + sorted(
            {self._account_label(row["account"]) for row in cat.unified
             if self._account_label(row["account"]) not in ("不限账号", "未登录")})
        current = self.f_account.currentText()
        self.f_account.blockSignals(True)
        self.f_account.clear()
        self.f_account.addItems(accounts)
        self.f_account.setCurrentText(current if current in accounts else "全部")
        self.f_account.blockSignals(False)

        self._fill_table()
        self.win.set_summary("sessions", f"{len(cat.unified)} 条会话")
        self.log(f"刷新完成：WorkBuddy 国内 {cat.cn_total} / 国际 {cat.intl_total}，"
                 f"CodeBuddy {len(cat.cb_sessions)} 条，合并 {len(cat.unified)} 条", "dim")

    # ------------------------------------------------------------------ 表格
    def _account_label(self, uid: str) -> str:
        cached = self._labels.get(uid)
        if cached is not None:
            return cached
        text = self._build_account_label(uid)
        self._labels[uid] = text
        return text

    def _build_account_label(self, uid: str) -> str:
        if not uid or uid == "default":
            return "未登录"
        tag = ""
        if self.cat is not None:
            if uid == self.cat.cn_uid:
                tag = "国内版"
            elif uid == self.cat.intl_uid:
                tag = "国际版"
        nickname = self._names.get(uid)
        if nickname:
            return f"{nickname}（{tag}）" if tag else nickname
        return tag or "未知账号"

    def _active_filters(self) -> list[str]:
        """当前生效的筛选条件；列表提示里要写出来，否则「看不见某类会话」分不清是
        真的没有、还是被上一个筛选挡住了。"""
        out = []
        if self.f_source.currentText() != "全部":
            out.append(f"软件={self.f_source.currentText()}")
        account = self.f_account.currentText()
        if account not in ("", "全部"):
            out.append(f"账号={account}")
        if self.opt_body.isChecked():
            out.append("只看有正文")
        query = self.search.text().strip()
        if query:
            out.append(f"搜索={query}")
        return out

    def _hint_text(self, shown: int) -> str:
        """带上各来源的总条数：国际版到底有没有会话，一眼能对出来。"""
        counts = {"cn": 0, "intl": 0, "codebuddy": 0}
        for row in self.cat.unified:
            key = "codebuddy" if row["kind"] == "codebuddy" else row["source_key"]
            counts[key] = counts.get(key, 0) + 1
        parts = [f"显示 {shown} / {len(self.cat.unified)} 条",
                 f"国内 {counts['cn']}", f"国际 {counts['intl']}",
                 f"CodeBuddy {counts['codebuddy']}"]
        active = self._active_filters()
        if active:
            parts.append("筛选：" + "、".join(active))
        if self.cat.errors:
            parts.append("读取异常，详见运行日志")
        return " · ".join(parts)

    def _passes(self, row: dict) -> bool:
        if self.f_source.currentText() != "全部" \
                and row["source_label"] != self.f_source.currentText():
            return False
        account = self.f_account.currentText()
        if account not in ("", "全部") \
                and self._account_label(row["account"]) != account:
            return False
        if self.opt_body.isChecked() and not row["has_body"]:
            return False
        query = self.search.text().strip().lower()
        if query and query not in str(row["title"]).lower() and query not in row["id"].lower():
            return False
        return True

    def _sort_value(self, row: dict):
        key = self._sort_key
        if key == "state":
            return STATE_RANK.get(row["state"], 9)
        if key in NUMERIC_SORTS:
            return int(row.get(key) or 0)
        return str(row.get(key) or "").lower()

    def _fill_table(self) -> None:
        if not self.cat:
            return
        selected = {row["rowid"] for row in self.table.selected_rows()}
        rows = [row for row in self.cat.unified if self._passes(row)]
        rows.sort(key=self._sort_value, reverse=self._sort_desc)
        self.rows = rows
        display = []
        self.row_index = {}
        for row in rows:
            # rowid 不含行号：排序或筛选后选中项还能对上
            rowid = f'{row["kind"]}|{row["source_key"]}|{row["account"]}|{row["id"]}'
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
        self.list_hint.setText(self._hint_text(len(display)))
        keep = [rowid for rowid in selected if rowid in self.row_index]
        if keep:
            self.table.select_key(keep[0])
        else:
            self._show_placeholder()

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

    def _show_placeholder(self) -> None:
        self._preview_token += 1
        self.detail.setHtml(
            f'<div style="color:{PALETTE["muted"]};font-size:12px">'
            "选中左侧任意一条会话，这里会显示它的对话内容。</div>")
        self._show_detail(False)

    def _show_detail(self, visible: bool) -> None:
        if self.detail_card.isVisible() == visible:
            return
        self.detail_card.setVisible(visible)
        if visible:
            # 刚显示时布局还没算完，等这一轮事件处理完再分配高度
            QTimer.singleShot(0, self._balance_splitter)

    def _balance_splitter(self) -> None:
        total = self.splitter.height()
        if total <= 0:
            return
        detail = max(180, min(300, int(total * 0.42)))
        self.splitter.setSizes([total - detail, detail])

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
        self._show_detail(True)
        cached = self._preview_cache.get(row["rowid"])
        if cached is not None:
            self._paint_detail(row, cached)
            return
        self._paint_detail(row, None)
        self._preview_token += 1
        token = self._preview_token

        def worker(_emit):
            return preview.load(row)

        # 正文在本地文件里，大的 jsonl 要读几百毫秒；走全局转圈，不占页面的 busy
        self.win.set_busy(True, "正在读取会话正文…", "preview")

        def done(result):
            self.win.set_busy(False, "", "preview")
            if token != self._preview_token:
                return                     # 用户已经选了别的会话
            self._preview_cache[row["rowid"]] = result
            self._paint_detail(row, result)

        def failed(message: str):
            self.win.set_busy(False, "", "preview")
            if token != self._preview_token:
                return
            self._paint_detail(row, {"messages": [], "total": 0, "dropped": 0,
                                     "truncated": False, "note": message})

        tasks.submit(worker, on_done=done, on_error=failed)

    def _paint_detail(self, row: dict, result: dict | None) -> None:
        muted, text, text2 = PALETTE["muted"], PALETTE["text"], PALETTE["text_2"]

        def item(key: str, value) -> str:
            return (f'<span style="color:{muted}">{key}</span> '
                    f'<span style="color:{text}">{html.escape(str(value))}</span>')

        # 顶部只留两行摘要：面板高度有限，纵向空间要留给对话内容
        lines = [" · ".join([item("软件", row["source_label"]),
                             item("账号", self._account_label(row["account"]))])]
        if row["kind"] == "workbuddy":
            lines.append(" · ".join([item("状态", row["state"]),
                                     item("最后活动", fmt_time(row["last"])),
                                     item("记录", f"{row['msgs']} 条"),
                                     item("大小", fmt_size(row["size"]))]))
            if row.get("state") == "云端":
                location = "云端（正文在服务端，登录该账号的客户端即可继续对话）"
            else:
                location = "\n".join(row["files"]) if row["files"] else "（无正文文件）"
            extra = [("工作目录", row.get("cwd") or "")]
            if row.get("client_status"):
                extra.append(("客户端状态", row["client_status"]))
        else:
            lines.append(" · ".join([item("状态", row["state"]),
                                     item("最后活动", fmt_time(row["last"])),
                                     item("消息文件", f"{row['msgs']} 个"),
                                     item("大小", fmt_size(row["size"]))]))
            location = row["path"] or "（无正文目录）"
            extra = []

        parts = [f'<div style="font-size:11px;line-height:1.85">', "<br>".join(lines), "</div>",
                 f'<div style="border-top:1px solid {PALETTE["border"]};margin:8px 0"></div>']
        parts.append(self._messages_html(row, result))

        # 诊断信息放最后：平时不需要，需要时往下滚一眼
        tail = [f'<div style="color:{muted};font-size:10px;margin-top:14px">正文位置</div>',
                f'<div style="color:{text2};font-size:10px;word-wrap:break-word">'
                f'{html.escape(location)}</div>']
        for key, value in extra:
            if value:
                tail.append(f'<div style="color:{muted};font-size:10px;margin-top:6px">{key}</div>'
                            f'<div style="color:{text2};font-size:10px;word-wrap:break-word">'
                            f'{html.escape(str(value))}</div>')
        parts.extend(tail)
        self.detail.setHtml("".join(parts))
        self.detail.verticalScrollBar().setValue(0)

    def _messages_html(self, row: dict, result: dict | None) -> str:
        head = (f'<div style="color:{PALETTE["muted"]};font-size:10px">对话内容</div>')
        if result is None:
            return head + (f'<div style="color:{PALETTE["muted"]};font-size:11px;'
                           'margin-top:4px">正在读取正文…</div>')
        messages = result.get("messages") or []
        if not messages:
            note = result.get("note") or "这条会话没有可显示的正文"
            return head + (f'<div style="color:{PALETTE["muted"]};font-size:11px;'
                           f'margin-top:4px">{html.escape(note)}</div>')
        blocks = [head]
        if result.get("note"):
            blocks.append(f'<div style="color:{PALETTE["muted"]};font-size:10px;'
                          f'margin-top:2px">{html.escape(result["note"])}</div>')
        for message in messages:
            if message.get("role") == "gap":
                blocks.append(
                    f'<div style="color:{PALETTE["muted"]};font-size:10px;text-align:center;'
                    f'margin:8px 0">{html.escape(message.get("text") or "")}</div>')
                continue
            mine = message.get("role") == "user"
            tone = PALETTE["accent"] if mine else PALETTE["text_2"]
            who = "你" if mine else "AI"
            body = html.escape(message.get("text") or "").replace("\n", "<br>")
            blocks.append(
                f'<div style="margin-top:10px">'
                f'<div style="color:{tone};font-size:10px">{who}</div>'
                f'<div style="color:{PALETTE["text"]};font-size:11px;line-height:1.6">'
                f'{body}</div></div>')
        return "".join(blocks)

    # ------------------------------------------------------------------ 删除
    def _selected(self) -> list[dict]:
        return [self.row_index[item["rowid"]] for item in self.table.selected_rows()
                if item["rowid"] in self.row_index]

    def delete_selected(self) -> None:
        if self.busy or not self.cat:
            return
        rows = self._selected()
        if not rows:
            self.info("未选择", "请先在列表里选中要删除的会话（可 Ctrl 多选）。")
            return
        wb_ids = list(dict.fromkeys(row["id"] for row in rows
                                    if row["kind"] == "workbuddy" and row["state"] != "云端"))
        cb_rows = [row["raw"] for row in rows if row["kind"] == "codebuddy"]
        cloud = [row for row in rows if row["state"] == "云端"]
        if not wb_ids and not cb_rows:
            self.info("无法删除",
                      "云端会话的正文在服务端，本工具删不掉，请打开对应客户端删除。"
                      if cloud else "没有可删除的会话。")
            return

        parts = []
        if wb_ids:
            parts.append(f"WorkBuddy {len(wb_ids)} 条")
        if cb_rows:
            parts.append(f"CodeBuddy {len(cb_rows)} 条")
        detail = (f"将删除 {'、'.join(parts)}会话。\n\n"
                  "数据库记录与正文文件会先移入回收站，可在「备份与清理」里恢复；"
                  "删除过的会话会加入黑名单，之后「恢复会话」不会再把它关联回来。")
        if cb_rows:
            detail += "\n\nCodeBuddy 的索引文件会被改写，建议先退出 IDE / IDEA。"
        if cloud:
            detail += f"\n\n另有 {len(cloud)} 条云端会话已跳过（本工具无法删除服务端数据）。"
        if not self.confirm("确认删除", detail):
            return

        running = [ep for ep in (Edition("cn"), Edition("intl"))
                   if wb_ids and winproc.process_running(ep.proc)]
        if running and not self.confirm(
                "需要关闭客户端",
                "、".join(ep.label for ep in running)
                + " 正在运行，删除前必须关闭，否则客户端会把会话写回来。\n\n现在自动关闭吗？"):
            return

        self._set_busy(True, "正在删除会话…")

        def worker(_emit):
            stats = {}
            if wb_ids:
                stats["workbuddy"] = service.delete_ids(
                    wb_ids, lambda m, lv="info": self.log(m, lv))
            if cb_rows:
                stats["codebuddy"] = service.delete_codebuddy(
                    cb_rows, lambda m, lv="info": self.log(m, lv))
            return stats

        def done(_stats):
            self._set_busy(False)
            count = len(wb_ids) + len(cb_rows)
            self.log(f"已删除 {count} 条会话（文件在回收站）", "ok")
            self.notify(f"已删除 {count} 条会话")
            bus.BUS.data_changed.emit("sessions")
            self.refresh()

        tasks.submit(worker, on_done=done, on_error=self._failed)

    def delete_all(self) -> None:
        if self.busy or not self.cat:
            return
        ids = [row["id"] for row in self.cat.rows]
        cb_sessions = list(self.cat.cb_sessions)
        if not ids and not cb_sessions:
            self.info("没有会话", "本机没有可删除的历史会话。")
            return
        text, ok = QInputDialog.getText(
            self, "清空全部会话",
            f"这会删除本机全部会话：WorkBuddy {len(ids)} 条、"
            f"CodeBuddy {len(cb_sessions)} 条。\n"
            "文件会移入回收站，会话会进入黑名单，之后「恢复会话」不会再带回它们。\n\n"
            "请输入「删除」确认：")
        if not ok:
            return
        if text.strip() != "删除":
            self.info("已取消", "输入内容不匹配，未做任何删除。")
            return
        if not self.confirm("确认清空",
                            "两个版本的客户端都需要关闭，删除期间请勿打开。\n\n继续吗？"):
            return

        self._set_busy(True, "正在清空会话…")

        def worker(_emit):
            stats = {"workbuddy": 0, "codebuddy": 0}
            if ids:
                stats["workbuddy"] = service.delete_ids(
                    ids, lambda m, lv="info": self.log(m, lv))["deleted"]
            if cb_sessions:
                stats["codebuddy"] = service.delete_codebuddy(
                    cb_sessions, lambda m, lv="info": self.log(m, lv))["sessions"]
            return stats

        def done(stats):
            self._set_busy(False)
            self.log(f"清空完成：WorkBuddy {stats['workbuddy']} 条、"
                     f"CodeBuddy {stats['codebuddy']} 条", "ok")
            self.notify("会话已清空")
            bus.BUS.data_changed.emit("sessions")
            self.refresh()

        tasks.submit(worker, on_done=done, on_error=self._failed)

    def _failed(self, message: str) -> None:
        self._set_busy(False)
        self.log(message, "error")
        self.error("操作失败", message.splitlines()[0])

    # ------------------------------------------------------------------ 其他
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
                winproc.open_path(path)
            else:
                self.info("目录不存在", "这条 CodeBuddy 会话没有正文目录。")
            return
        for item in row["files"] or []:
            parent = Path(item).parent
            if parent.exists():
                winproc.open_path(parent)
                return
        self.info("没有正文", "这条会话没有正文文件。")
