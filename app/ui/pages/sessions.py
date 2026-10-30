# -*- coding: utf-8 -*-
"""会话管理：本机会话的浏览、正文预览与删除。

不做跨版本同步——账号之间的会话恢复在账号中心完成，这一页只回答
「本机有哪些会话、里面写了什么、删哪些」。
"""
from __future__ import annotations

import html
from pathlib import Path

from PySide6.QtCore import Qt, QTimer
from PySide6.QtWidgets import (QCheckBox, QComboBox, QFrame, QHBoxLayout, QInputDialog,
                               QLineEdit, QSplitter, QTextBrowser, QVBoxLayout)

from ...core import bus, tasks, winproc
from ...core.format import fmt_size, fmt_time
from ...sessions import bridge, preview, service
from ...sessions.editions import Edition
from ..models import Column
from ..theme import PALETTE
from ..widgets import Card, DataTable, hline, label, make_button
from .base import BasePage

COLUMNS = [
    Column("title", "会话标题", 240, stretch=True),
    Column("account", "账号", 200),
    Column("state", "状态", 66, "center"),
    Column("last", "最后活动", 132, "center"),
    Column("msgs", "记录", 50, "right"),
    Column("size", "大小", 70, "right"),
]

# 状态筛选是唯一的维度筛选；平台来源由分组标题行的标签区分，不再单列筛选
STATE_FILTERS = ["全部", "可见", "有正文", "已归档", "已删除", "云端", "无正文", "旧账号"]

# 表头第几列按哪个字段排序；文本列升序，时间/数量列默认降序
SORT_COLUMNS = {"title": 0, "account": 1, "state": 2, "last": 3, "msgs": 4, "size": 5}
COLUMN_SORT = {index: key for key, index in SORT_COLUMNS.items()}
NUMERIC_SORTS = ("last", "msgs", "size")
STATE_RANK = {"可见": 0, "有正文": 0, "云端": 1, "已归档": 1,
              "旧账号": 2, "无正文": 3, "已删除": 4}


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
        self._labels: dict[tuple, str] = {}
        self._names: dict[str, str] = {}
        self._known_uids: tuple[str, str] = ("", "")
        self._preview_cache: dict[str, dict] = {}
        self._preview_token = 0
        self._detail_row: dict | None = None
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
        line.addWidget(label("状态", "StatLabel"))
        self.f_state = QComboBox()
        self.f_state.addItems(STATE_FILTERS)
        self.f_state.setFixedWidth(110)
        self.f_state.currentIndexChanged.connect(self._on_state_change)
        line.addWidget(self.f_state)
        line.addWidget(label("搜索", "StatLabel"))
        self.search = QLineEdit()
        self.search.setPlaceholderText("标题或会话 ID")
        self.search.setFixedWidth(220)
        # 三百多行每敲一个字就整表重建会明显发涩，加 180ms 防抖
        self._search_timer = QTimer(self)
        self._search_timer.setSingleShot(True)
        self._search_timer.setInterval(180)
        self._search_timer.timeout.connect(self._fill_table)
        self.search.textChanged.connect(lambda _t: self._search_timer.start())
        line.addWidget(self.search)
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
        head.addStretch(1)
        self.check_all = QCheckBox("全选")
        self.check_all.clicked.connect(self._toggle_all)
        head.addWidget(self.check_all)
        self.btn_import = make_button("导入 WorkBuddy", "ghost")
        self.btn_import.setToolTip("把选中的 CodeBuddy IDE / IDEA 会话转成 WorkBuddy 会话")
        self.btn_import.clicked.connect(self.import_to_workbuddy)
        # 跨软件导入默认下线：格式转换对会话数据的影响未验证过，开关在设置页
        self.btn_import.setVisible(bool(self.cfg.get("allow_cross_software")))
        head.addWidget(self.btn_import)
        self.btn_delete = make_button("删除", "danger-ghost")
        self.btn_delete.clicked.connect(self.delete_selected)
        head.addWidget(self.btn_delete)
        self.btn_delete_all = make_button("清空全部", "danger-ghost")
        self.btn_delete_all.clicked.connect(self.delete_all)
        head.addWidget(self.btn_delete_all)
        table_layout.addLayout(head)
        table_layout.addWidget(hline())
        self.table = DataTable(COLUMNS, key="rowid", checkable=True, parent=self)
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
        self.detail.setObjectName("SessionDetail")
        self.detail.setOpenExternalLinks(False)
        self.detail.setFrameShape(QFrame.Shape.NoFrame)
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
        state = str(opts.get("state") or "全部")
        self.f_state.setCurrentText(state if state in STATE_FILTERS else "全部")
        self._sort_key = opts.get("sort") if opts.get("sort") in SORT_COLUMNS else "last"
        self._sort_desc = bool(opts.get("sort_desc", True))
        self._update_sort_indicator()

    def _save_options(self) -> None:
        self.cfg["session_opts"] = {
            "state": self.f_state.currentText(),
            "sort": self._sort_key,
            "sort_desc": self._sort_desc,
        }
        self.win.save_cfg()

    def _on_state_change(self, *_args) -> None:
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
            self.btn_import.setVisible(bool(self.cfg.get("allow_cross_software")))
            self.refresh()

    def refresh(self) -> None:
        if self.busy:
            return
        # 阶段一：索引快照先上屏（只读 data/catalog.db），列表立刻可读；
        # 阶段二：后台按源指纹增量校对，完成后整表校正。
        snapshot = service.cached_rows()
        if snapshot:
            self._apply_catalog({"cat": service.catalog_from_rows(snapshot),
                                 "names": self._names, "cached": True})
        self._set_busy(True, "正在校对本机会话…")

        def worker(emit):
            cat = service.build_catalog(log=lambda m, lv="info": self.log(m, lv))
            return {"cat": cat, "names": service.account_names()}

        tasks.submit(worker, on_done=self._apply_catalog, on_error=self._load_failed)

    def _account_names(self) -> dict[str, str]:
        """昵称解析已下沉到 service（建索引时做），这里只为兼容旧调用保留。"""
        return service.account_names(lambda m, lv="info": self.log(m, lv))

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
        self.log(message, "error")
        self.notify("读取会话失败，详情见运行日志", "error")

    def _apply_catalog(self, payload) -> None:
        if isinstance(payload, dict):
            cat = payload.get("cat")
            self._names = payload.get("names") or self._names
        else:
            cat = payload
        self.cat = cat
        self._labels = {}
        # 快照先上屏时不清预览缓存、不松 busy：真正的校对结果随后就到
        if not (isinstance(payload, dict) and payload.get("cached")):
            self._preview_cache = {}
            self._set_busy(False)

        self._fill_table()
        self.win.set_summary("sessions", f"{len(cat.unified)} 条会话")
        self.log(f"刷新完成：WorkBuddy 国内 {cat.cn_total} / 国际 {cat.intl_total}，"
                 f"CodeBuddy {len(cat.cb_sessions)} 条，合并 {len(cat.unified)} 条", "dim")

    # ------------------------------------------------------------------ 表格
    def _shown_account(self, row: dict) -> str:
        """账号列的显示值：优先用建索引时解析好的昵称，界面不出现 uid。"""
        return row.get("account_name") or self._account_label(row["account"], row["kind"])

    def _account_label(self, uid: str, kind: str = "") -> str:
        key = (kind, uid)
        cached = self._labels.get(key)
        if cached is not None:
            return cached
        text = self._build_account_label(uid, kind)
        self._labels[key] = text
        return text

    def _build_account_label(self, uid: str, kind: str = "") -> str:
        # Qoder / ZCode / Trae 的库没有账号字段，同一客户端换号共用一份库
        if kind in ("qoder", "zcode", "trae"):
            return "本机"
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

    def _passes(self, row: dict) -> bool:
        state = self.f_state.currentText()
        if state != "全部" and row["state"] != state:
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
        checked = {row["rowid"] for row in self.table.checked_rows()}
        rows = [row for row in self.cat.unified if self._passes(row)]
        rows.sort(key=self._sort_value, reverse=self._sort_desc)
        self.rows = rows

        # 按平台分组，每组一行分组标签：软件来源靠它区分，组内沿用当前排序
        groups: dict[str, list[dict]] = {}
        for row in rows:
            groups.setdefault(row["source_label"], []).append(row)
        order = {label: i for i, label in enumerate(service.source_labels())}

        display = []
        self.row_index = {}
        for label in sorted(groups, key=lambda name: order.get(name, 99)):
            display.append({"rowid": f"section|{label}", "title": label,
                            "account": "", "state": "", "last": "",
                            "msgs": "", "size": "",
                            "_section": True, "_bold": True,
                            "_fg": PALETTE["accent"]})
            for row in groups[label]:
                # rowid 不含行号：排序或筛选后选中项还能对上
                rowid = f'{row["kind"]}|{row["source_key"]}|{row["account"]}|{row["id"]}'
                row["rowid"] = rowid
                self.row_index[rowid] = row
                display.append({
                    "rowid": rowid,
                    "title": row["title"],
                    "account": self._shown_account(row),
                    "state": row["state"],
                    "last": fmt_time(row["last"]),
                    "msgs": row["msgs"] or "—",
                    "size": fmt_size(row["size"]) if row["size"] else "—",
                    "_checked": rowid in checked,
                    "_fg_map": {"state": self._state_color(row["state"])},
                    "_tip_map": {"title": row["title"]},
                })
        self.table.set_rows(display)
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
        if state in ("旧账号", "无正文", "已归档"):
            return PALETTE["warn"]
        if state == "云端":
            return PALETTE["accent"]
        return PALETTE["muted"]

    def _show_placeholder(self) -> None:
        self._preview_token += 1
        self._detail_row = None
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

    def refresh_theme(self) -> None:
        """换肤后重画详情：段落颜色是渲染时内联进 HTML 的，不重画会留在旧配色上。"""
        row = self._detail_row
        if row is None:
            self._show_placeholder()
        else:
            self._paint_detail(row, self._preview_cache.get(row["rowid"]))

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
        self._detail_row = row
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

        tasks.submit(worker, on_done=done, on_error=failed, serial=True )

    def _paint_detail(self, row: dict, result: dict | None) -> None:
        muted, text, text2 = PALETTE["muted"], PALETTE["text"], PALETTE["text_2"]

        def item(key: str, value) -> str:
            return (f'<span style="color:{muted}">{key}</span> '
                    f'<span style="color:{text}">{html.escape(str(value))}</span>')

        # 顶部只留两行摘要：面板高度有限，纵向空间要留给对话内容
        lines = [" · ".join([item("软件", row["source_label"]),
                             item("账号", self._shown_account(row))])]
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
        elif row["kind"] in ("qoder", "zcode"):
            lines.append(" · ".join([item("状态", row["state"]),
                                     item("最后活动", fmt_time(row["last"])),
                                     item("记录", f"{row['msgs']} 条"),
                                     item("大小", fmt_size(row["size"]))]))
            location = f"本地会话库：{row['path']}"
            extra = [("工作目录", row.get("cwd") or "")]
        elif row["kind"] == "trae":
            lines.append(" · ".join([item("状态", row["state"])]))
            location = "对话正文在服务端，本工具读不到本地内容；查看或删除请用 Trae 客户端。"
            extra = []
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
        """勾选优先（批量操作主路径），没有勾选时退回当前选中行。"""
        checked = [self.row_index[item["rowid"]] for item in self.table.checked_rows()
                   if item["rowid"] in self.row_index]
        if checked:
            return checked
        return [self.row_index[item["rowid"]] for item in self.table.selected_rows()
                if item["rowid"] in self.row_index]

    def _toggle_all(self, checked: bool) -> None:
        self.table.set_checked_all(checked)

    def delete_selected(self) -> None:
        if self.busy or not self.cat:
            return
        rows = self._selected()
        if not rows:
            self.info("未选择", "请先勾选要删除的会话（表头「全选」可一键勾选）。")
            return
        wb_ids = list(dict.fromkeys(row["id"] for row in rows
                                    if row["kind"] == "workbuddy" and row["state"] != "云端"))
        cb_rows = [row["raw"] for row in rows if row["kind"] == "codebuddy"]
        other_rows = [row for row in rows if row["kind"] in ("qoder", "zcode")]
        cloud = [row for row in rows if row["state"] == "云端"]
        if not wb_ids and not cb_rows and not other_rows:
            self.info("无法删除",
                      "云端会话的正文在服务端，本工具删不掉，请打开对应客户端删除。"
                      if cloud else "没有可删除的会话。")
            return

        parts = []
        if wb_ids:
            parts.append(f"WorkBuddy {len(wb_ids)} 条")
        if cb_rows:
            parts.append(f"CodeBuddy {len(cb_rows)} 条")
        if other_rows:
            counts: dict[str, int] = {}
            for row in other_rows:
                counts[row["source_label"]] = counts.get(row["source_label"], 0) + 1
            parts.extend(f"{label} {count} 条" for label, count in counts.items())
        detail = (f"将删除 {'、'.join(parts)}会话。\n\n"
                  "数据库记录与正文文件会先移入回收站，可在「备份与清理」里恢复；"
                  "删除过的会话会加入黑名单，之后「恢复会话」不会再把它关联回来。")
        if cb_rows:
            detail += "\n\nCodeBuddy 的索引文件会被改写，建议先退出 IDE / IDEA。"
        if other_rows:
            detail += "\n\nQoder / ZCode 正在运行时会被自动关闭，删除后它们可能从云端重新拉回。"
        detail += "\n\n正在运行的相关客户端会先关闭、写完自动重启，以保证数据一致。"
        if cloud:
            detail += f"\n\n另有 {len(cloud)} 条云端会话已跳过（本工具无法删除服务端数据）。"
        if not self.confirm("确认删除", detail):
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
            if other_rows:
                stats["others"] = service.delete_others(
                    other_rows, lambda m, lv="info": self.log(m, lv))
            return stats

        def done(_stats):
            self._set_busy(False)
            count = len(wb_ids) + len(cb_rows) + len(other_rows)
            self.log(f"已删除 {count} 条会话（文件在回收站）", "ok")
            self.notify(f"已删除 {count} 条会话")
            bus.BUS.data_changed.emit("sessions")
            self.refresh()

        tasks.submit(worker, on_done=done, on_error=self._failed, serial=True )

    def delete_all(self) -> None:
        if self.busy or not self.cat:
            return
        ids = [row["id"] for row in self.cat.rows]
        cb_sessions = list(self.cat.cb_sessions)
        other_rows = [row for row in self.cat.others if row["kind"] in ("qoder", "zcode")]
        if not ids and not cb_sessions and not other_rows:
            self.info("没有会话", "本机没有可删除的历史会话。")
            return
        text, ok = QInputDialog.getText(
            self, "清空全部会话",
            f"这会删除本机全部会话：WorkBuddy {len(ids)} 条、"
            f"CodeBuddy {len(cb_sessions)} 条、其他软件 {len(other_rows)} 条。\n"
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
            stats = {"workbuddy": 0, "codebuddy": 0, "others": 0}
            if ids:
                stats["workbuddy"] = service.delete_ids(
                    ids, lambda m, lv="info": self.log(m, lv))["deleted"]
            if cb_sessions:
                stats["codebuddy"] = service.delete_codebuddy(
                    cb_sessions, lambda m, lv="info": self.log(m, lv))["sessions"]
            if other_rows:
                stats["others"] = service.delete_others(
                    other_rows, lambda m, lv="info": self.log(m, lv))["deleted"]
            return stats

        def done(stats):
            self._set_busy(False)
            self.log(f"清空完成：WorkBuddy {stats['workbuddy']} 条、"
                     f"CodeBuddy {stats['codebuddy']} 条、其他 {stats['others']} 条", "ok")
            self.notify("会话已清空")
            bus.BUS.data_changed.emit("sessions")
            self.refresh()

        tasks.submit(worker, on_done=done, on_error=self._failed, serial=True )

    # ------------------------------------------------------------------ 导入 WorkBuddy
    def import_to_workbuddy(self) -> None:
        """把选中的 CodeBuddy IDE / IDEA 会话转成 WorkBuddy 会话。

        两边格式不能互认，只能重建：只保留对话文本，工具调用、推理过程与附件
        不带过去。目标归属该版本当前登录的账号，导入后马上能在这张表里看到。
        """
        if self.busy:
            return
        if not self.cfg.get("allow_cross_software"):
            self.info("跨软件导入未启用",
                      "跨软件的格式转换对会话数据的影响尚未验证，默认关闭。\n\n"
                      "确认风险后可在「设置 → 会话索引与兼容」里开启。")
            return
        picked = [row for row in self._selected() if row["kind"] == "codebuddy"]
        if picked:
            rows = picked
        else:
            # 几百条会话一条条点不现实：没选中时按当前筛选全量导入
            rows = [row for row in (self.cat.unified if self.cat else [])
                    if row["kind"] == "codebuddy" and self._passes(row)]
            if not rows:
                self.info("无法导入",
                          "请选中 CodeBuddy IDE 或 IDEA 插件的会话。\n"
                          "WorkBuddy 之间的会话在账号中心用「恢复会话」。")
                return
            if not self.confirm("导入全部",
                                f"没有选中会话。要把当前筛选下的 {len(rows)} 条 "
                                f"CodeBuddy / IDEA 会话全部导入吗？\n\n"
                                "已经导入过的会自动跳过。"):
                return
        ep = self._pick_workbuddy_edition()
        if ep is None:
            return
        uid = ep.current_uid()[0]
        if not uid:
            self.info("无法导入", f"读不到 {ep.label} 当前登录的账号，"
                                   "先打开客户端登录一次再来导入。")
            return
        if not self.confirm(
                f"导入到 {ep.label}",
                f"把选中的 {len(rows)} 条会话导入 {ep.short}，归属当前登录账号？\n\n"
                "会重新生成 WorkBuddy 的会话正文：只保留对话文本，工具调用、"
                "推理过程与附件不导入；插件里的原会话不受影响。\n"
                "客户端会先关闭，导入完成后需要手动打开。继续吗？"):
            return
        if winproc.process_running(ep.proc):
            self.log(f"正在关闭 {ep.label}…")
            if not winproc.kill_process(ep.proc):
                self.error("关闭失败", f"{ep.label} 没能关闭，请手动退出后重试。")
                return

        self._set_busy(True, "正在导入会话…")
        payload = [row["raw"] for row in rows]
        log = lambda m, lv="info": self.log(m, lv)  # noqa: E731 - 回调只需要一个闭包

        def done(stats: dict) -> None:
            self._set_busy(False)
            detail = f"新增 {stats['imported']}，已存在 {stats['exists']}"
            if stats["skipped"]:
                detail += f"，跳过 {stats['skipped']}"
            if stats["failed"]:
                detail += f"，失败 {stats['failed']}"
            self.log(f"导入到 {ep.label}：{detail}",
                     "ok" if stats["imported"] else "warn")
            self.notify(f"已导入 {stats['imported']} 条会话")
            bus.BUS.data_changed.emit("sessions")
            self.refresh()

        tasks.submit(
            lambda _emit: bridge.import_sessions(payload, ep, uid, log, allow_cross=True),
            on_done=done,
            on_error=self._failed,
        serial=True )

    def _pick_workbuddy_edition(self) -> Edition | None:
        """导入目标：本机有会话库的版本；两个都在时让用户选。"""
        eds = [ep for ep in (Edition("cn"), Edition("intl")) if ep.exists]
        if not eds:
            self.info("没有会话库", "本机没有 WorkBuddy 的会话库，先打开客户端用一次。")
            return None
        if len(eds) == 1:
            return eds[0]
        items = [ep.label for ep in eds]
        text, ok = QInputDialog.getItem(self, "导入到哪个版本", "目标版本：",
                                        items, 0, False)
        return eds[items.index(text)] if ok else None

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
        if row["kind"] in ("qoder", "zcode"):
            path = Path(row["path"])
            if path.parent.exists():
                winproc.open_path(path.parent)
            else:
                self.info("目录不存在", "找不到这条会话所在的本地会话库。")
            return
        if row["kind"] == "trae":
            self.info("正文在云端", "Trae 的对话记录不在本地，请在客户端里查看。")
            return
        for item in row["files"] or []:
            parent = Path(item).parent
            if parent.exists():
                winproc.open_path(parent)
                return
        self.info("没有正文", "这条会话没有正文文件。")
