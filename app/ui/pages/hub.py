# -*- coding: utf-8 -*-
"""技能与 MCP：技能放一次、MCP 登记一次，按平台自动复制 / 写入。

页面只负责选择和展示，扫描、复制、写配置全部走后台任务；
平台路径与格式差异由 app/hub 处理，这里不直接拼路径。
"""
from __future__ import annotations

import json
import time
from datetime import datetime
from pathlib import Path

from PySide6.QtCore import Qt
from PySide6.QtWidgets import (QButtonGroup, QCheckBox, QComboBox, QFileDialog,
                               QFrame, QHBoxLayout, QPlainTextEdit, QSizePolicy,
                               QStackedWidget, QVBoxLayout, QWidget)

from ...core import tasks, winproc
from ...hub import mcp, platforms, service, store, targets
from ..dialogs_hub import (ImportDialog, McpEditorDialog, McpQuickImportDialog,
                           TargetPickerDialog)
from ..models import Column
from ..theme import PALETTE
from ..widgets import Badge, Card, DataTable, EmptyState, hline, label, make_button
from .base import BasePage

SKILL_COLUMNS = [
    Column("name", "技能", 190, stretch=True),
    Column("description", "说明", 300, stretch=True),
    Column("platforms", "同步平台", 100),
    Column("state", "状态", 84),
    Column("updated", "更新时间", 108),
]

MCP_COLUMNS = [
    Column("name", "名称", 170, stretch=True),
    Column("kind", "类型", 66),
    Column("where", "命令 / 地址", 320, stretch=True, mono=True),
    Column("platforms", "同步平台", 100),
    Column("state", "状态", 84),
]

PLATFORM_SKILL_COLUMNS = [
    Column("name", "技能", 180, stretch=True),
    Column("platform", "来源平台", 132),
    Column("kind", "类型", 60),
    Column("description", "说明", 280, stretch=True),
    Column("path", "位置", 200, mono=True),
]

STATE_STYLE = {
    "synced": ("已同步", "ok"),
    "stale": ("待更新", "warn"),
    "untracked": ("平台已有", "warn"),
    "unassigned": ("未分配", "muted"),
    "invalid": ("源缺失", "danger"),
}


def _short_ts(value) -> str:
    try:
        return datetime.fromtimestamp(float(value)).strftime("%m-%d %H:%M")
    except (TypeError, ValueError, OSError):
        return "—"


def _platform_text(keys) -> str:
    return f"{len(keys)} 个平台" if keys else "未分配"


def _platform_tip(keys) -> str:
    return "\n".join(targets.labels(keys)) or "还没有分配平台"


def _clear_layout(layout) -> None:
    """立刻把旧徽标从父控件上摘掉：只 deleteLater 会在事件循环处理前
    继续留在卡片里，长文本还会把详情卡撑得比页面还宽。"""
    while layout.count():
        item = layout.takeAt(0)
        widget = item.widget()
        if widget is not None:
            widget.hide()
            widget.setParent(None)
            widget.deleteLater()


class HubPage(BasePage):
    key = "hub"
    title = "技能与 MCP"
    subtitle = "技能放一次、MCP 登记一次，按平台自动复制和写入"

    def __init__(self, win):
        super().__init__(win)
        self.state: dict = {"hub": store.defaults(), "targets": [], "skills": [],
                            "mcp": [], "library": "", "mcp_untracked": [],
                            "skill_untracked": []}
        self._platform_items: list[dict] = []
        self._loaded_at = 0.0
        self._build()
        self._fill()

    # ------------------------------------------------------------------ 界面
    def _build(self) -> None:
        # 页头整块去掉：左侧导航已经标出当前页，顶部只留一行操作区
        self.header.hide()

        segment = QFrame()
        segment.setObjectName("CardFlat")
        seg_layout = QHBoxLayout(segment)
        seg_layout.setContentsMargins(10, 6, 10, 6)
        seg_layout.setSpacing(4)

        self.btn_skills = make_button("技能", "ghost")
        self.btn_skills.setObjectName("Segment")
        self.btn_skills.setCheckable(True)
        self.btn_skills.setChecked(True)
        self.btn_mcp = make_button("MCP", "ghost")
        self.btn_mcp.setObjectName("Segment")
        self.btn_mcp.setCheckable(True)
        self.btn_platform = make_button("平台技能", "ghost")
        self.btn_platform.setObjectName("Segment")
        self.btn_platform.setCheckable(True)
        self.btn_platform.setToolTip("各平台里实际存在的技能：内置（官方）与用户目录（个人）分开标明")
        group = QButtonGroup(self)
        group.setExclusive(True)
        group.addButton(self.btn_skills, 0)
        group.addButton(self.btn_mcp, 1)
        group.addButton(self.btn_platform, 2)
        group.idClicked.connect(self._switch_tab)
        seg_layout.addWidget(self.btn_skills)
        seg_layout.addWidget(self.btn_mcp)
        seg_layout.addWidget(self.btn_platform)
        seg_layout.addStretch(1)
        self.auto_check = QCheckBox("启动时自动同步")
        self.auto_check.setToolTip("启动后把技能与 MCP 的变更自动补到各平台，"
                                   "不需要再手动逐个添加")
        self.auto_check.stateChanged.connect(self._toggle_auto)
        seg_layout.addWidget(self.auto_check)
        seg_layout.addSpacing(6)
        seg_layout.addWidget(self._action("打开技能库", "ghost", "folder",
                                          self.open_library))
        seg_layout.addWidget(self._action("立即同步", "primary", "refresh",
                                          self.sync_all))
        self.body.addWidget(segment)

        self.stack = QStackedWidget()
        self.stack.addWidget(self._build_skills())
        self.stack.addWidget(self._build_mcp())
        self.stack.addWidget(self._build_platform_skills())
        self.body.addWidget(self.stack, 1)

    def _action(self, text: str, kind: str, icon: str, handler):
        button = make_button(text, kind, icon, self)
        button.clicked.connect(handler)
        return button

    def _switch_tab(self, index: int) -> None:
        self.stack.setCurrentIndex(index)
        if index == 2 and not self._platform_items:
            self.refresh_platform_skills()      # 兜底：快照里没有才单独扫一次

    # ---------------------------------------------------------- 平台技能页
    def _build_platform_skills(self) -> QWidget:
        """各平台里实际存在的技能：官方（内置）与个人（用户目录）分开标。"""
        page = QWidget()
        layout = QVBoxLayout(page)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(12)

        # 平台标签条：一个平台一个标签，点谁看谁，不用去下拉里翻
        strip = QFrame()
        strip.setObjectName("CardFlat")
        strip_layout = QHBoxLayout(strip)
        strip_layout.setContentsMargins(10, 6, 10, 6)
        strip_layout.setSpacing(4)
        self.platform_group = QButtonGroup(self)
        self.platform_group.setExclusive(True)
        self.platform_tabs: dict = {}
        self._platform_order = [""] + [target.key for target in targets.TARGETS]
        self._platform_key = ""
        for index, key in enumerate(self._platform_order):
            button = make_button("全部" if not key else targets.TARGET_MAP[key].label, "ghost")
            button.setObjectName("Segment")
            button.setCheckable(True)
            button.setChecked(index == 0)
            self.platform_group.addButton(button, index)
            self.platform_tabs[key] = button
            strip_layout.addWidget(button)
        self.platform_group.idClicked.connect(self._pick_platform)
        strip_layout.addStretch(1)
        layout.addWidget(strip)

        toolbar = Card(padding=12, spacing=8)
        row = QHBoxLayout()
        row.setSpacing(6)
        row.addWidget(label("类型", "StatLabel"))
        self.kind_filter = QComboBox()
        self.kind_filter.setFixedWidth(92)
        self.kind_filter.addItem("全部", "")
        self.kind_filter.addItem("官方", "official")
        self.kind_filter.addItem("个人", "personal")
        self.kind_filter.currentIndexChanged.connect(self._fill_platform_skills)
        row.addWidget(self.kind_filter)
        row.addSpacing(8)
        row.addWidget(self._action("刷新", "ghost", "refresh", self.refresh_platform_skills))
        row.addWidget(self._action("导入到技能库", "ghost", "archive",
                                   self.import_platform_skill))
        row.addWidget(self._action("打开目录", "ghost", "folder", self.open_platform_skill))
        row.addWidget(self._action("删除", "danger-ghost", "trash",
                                   self.delete_platform_skill))
        self.platform_summary = label("正在扫描各平台技能…", "SectionHint")
        self.platform_summary.setSizePolicy(QSizePolicy.Policy.Ignored,
                                            QSizePolicy.Policy.Preferred)
        self.platform_summary.setAlignment(Qt.AlignmentFlag.AlignRight
                                           | Qt.AlignmentFlag.AlignVCenter)
        row.addWidget(self.platform_summary, 1)
        toolbar.body.addLayout(row)
        layout.addWidget(toolbar)

        card = QFrame()
        card.setObjectName("Card")
        card_layout = QVBoxLayout(card)
        card_layout.setContentsMargins(0, 0, 0, 0)
        card_layout.setSpacing(0)
        head = QHBoxLayout()
        head.setContentsMargins(16, 12, 16, 10)
        head.addWidget(label("平台技能", "SectionTitle"))
        self.platform_hint = label(
            "官方＝随客户端分发的内置技能（只读）；个人＝用户目录里自己加的", "SectionHint")
        head.addWidget(self.platform_hint, 1)
        card_layout.addLayout(head)
        card_layout.addWidget(hline())

        self.platform_table = DataTable(PLATFORM_SKILL_COLUMNS, key="id",
                                        multi=True, parent=self)
        self.platform_table.doubleClicked.connect(lambda _i: self.open_platform_skill())
        card_layout.addWidget(self.platform_table, 1)
        self.platform_empty = EmptyState(
            "没有找到技能",
            "选中的平台里既没有内置技能，用户目录也是空的")
        card_layout.addWidget(self.platform_empty, 1)
        layout.addWidget(card, 1)
        return page

    def refresh_platform_skills(self) -> None:
        if self.busy:
            return
        self._set_busy(True, "正在扫描各平台技能…")

        def done(items):
            self._set_busy(False)
            self._platform_items = items
            self._fill_platform_skills()
            self.log(f"平台技能盘点完成：共 {len(items)} 个", "dim")

        self._run(platforms.scan, "正在扫描各平台技能…", on_done=done)

    def _pick_platform(self, index: int) -> None:
        if 0 <= index < len(self._platform_order):
            self._platform_key = self._platform_order[index]
        self._fill_platform_skills()

    def _fill_platform_skills(self) -> None:
        want_kind = self.kind_filter.currentData() or ""
        counts: dict[str, int] = {}
        rows = []
        official = personal = 0
        for item in self._platform_items:
            counts[item["platform_key"]] = counts.get(item["platform_key"], 0) + 1
            if self._platform_key and item["platform_key"] != self._platform_key:
                continue
            if want_kind and item["kind"] != want_kind:
                continue
            if item["kind"] == "official":
                official += 1
            else:
                personal += 1
            rows.append({
                "id": item["id"],
                "name": item["name"],
                "platform": item["platform_label"],
                "kind": item["kind_label"],
                "description": item.get("description") or "—",
                "path": item.get("path_text") or "",
                "_fg_map": {"kind": PALETTE["accent"] if item["kind"] == "official"
                            else PALETTE["muted"]},
                "_tip_map": {
                    "name": item.get("path_text") or "",
                    "platform": item.get("group") or item["platform_label"],
                    "kind": ("内置技能，随客户端分发，只读"
                             if item["kind"] == "official"
                             else "用户目录里的技能，可导入技能库或删除"),
                    "description": item.get("description") or "",
                    "path": item.get("path") or "",
                },
            })
        self.platform_table.set_rows(rows)
        self.platform_empty.setVisible(not rows)
        self.platform_table.setVisible(bool(rows))
        total = len(rows)
        self.platform_summary.setText(
            f"共 {total} 个 · 官方 {official} / 个人 {personal}")
        # 每个平台标签都带上自己的数量，一眼看出哪里多
        for key, button in self.platform_tabs.items():
            base = "全部" if not key else targets.TARGET_MAP[key].label
            button.setText(f"{base} · {counts.get(key, 0) if key else len(self._platform_items)}")
        self.btn_platform.setText(f"平台技能 · {len(self._platform_items)}")

    def _selected_platform_skills(self) -> list[dict]:
        chosen = {row.get("id") for row in self.platform_table.selected_rows()}
        return [item for item in self._platform_items if item["id"] in chosen]

    def import_platform_skill(self) -> None:
        items = self._selected_platform_skills()
        if not items:
            self.info("导入到技能库", "请先在列表里选中技能。")
            return

        def done(result):
            text = f"导入 {len(result['imported'])} 个"
            if result["skipped"]:
                text += f"，跳过 {len(result['skipped'])} 个"
            if result["errors"]:
                text += f"，失败 {len(result['errors'])} 个"
            self.log(text, "ok" if not result["errors"] else "warn")
            if result["errors"]:
                self.warn("部分技能导入失败", "\n".join(result["errors"][:8]))
            self.reload()

        payload = [{"name": item["name"], "path": item["path"],
                    "sources": [item["platform_key"]]} for item in items]
        self._run(lambda: service.import_skills(payload), "正在导入技能…",
                  on_done=done)

    def open_platform_skill(self) -> None:
        for item in self._selected_platform_skills():
            if not platforms.open_dir(item):
                self.warn("打开目录", f"{item['name']} 的目录已不存在。")

    def delete_platform_skill(self) -> None:
        items = [item for item in self._selected_platform_skills()
                 if item["kind"] == "personal"]
        if not items:
            self.info("删除技能", "只能删除用户目录里的个人技能；内置技能随客户端分发，不可删。")
            return
        names = "、".join(item["name"] for item in items[:5])
        more = f" 等 {len(items)} 个" if len(items) > 5 else ""
        if not self.confirm("删除技能", f"把 {names}{more} 移到回收站？\n"
                                        "只影响该平台的用户目录，不影响技能库。"):
            return

        def work(_emit):
            removed, errors = [], []
            for item in items:
                try:
                    platforms.delete_personal(item)
                    removed.append(item["name"])
                except store.HubError as exc:
                    errors.append(f"{item['name']}：{exc}")
            return {"removed": removed, "errors": errors}

        def done(result):
            self.log(f"已删除 {len(result['removed'])} 个平台技能", "ok")
            if result["errors"]:
                self.warn("部分技能未能删除", "\n".join(result["errors"][:8]))
            self.refresh_platform_skills()

        self._run(work, "正在删除技能…", on_done=done)

    # -------------------------------------------------------------- 技能页
    def _build_skills(self) -> QWidget:
        page = QWidget()
        layout = QVBoxLayout(page)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(12)

        toolbar = Card(padding=12, spacing=8)
        row = QHBoxLayout()
        row.setSpacing(6)
        row.addWidget(self._action("添加技能", "primary", "plus", self.add_skill))
        row.addWidget(self._action("从平台导入", "ghost", "archive", self.import_skills))
        row.addWidget(self._action("调整平台", "ghost", "hub",
                                   self.adjust_skill_targets))
        row.addWidget(self._action("移除", "danger-ghost", "trash", self.remove_skill))
        self.skill_summary = label("正在扫描技能库…", "SectionHint")
        self.skill_summary.setSizePolicy(QSizePolicy.Policy.Ignored,
                                         QSizePolicy.Policy.Preferred)
        self.skill_summary.setAlignment(
            Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter)
        row.addWidget(self.skill_summary, 1)
        toolbar.body.addLayout(row)
        layout.addWidget(toolbar)

        card = QFrame()
        card.setObjectName("Card")
        card_layout = QVBoxLayout(card)
        card_layout.setContentsMargins(0, 0, 0, 0)
        card_layout.setSpacing(0)
        head = QHBoxLayout()
        head.setContentsMargins(16, 12, 16, 10)
        head.setSpacing(8)
        head.addWidget(label("技能库", "SectionTitle"))
        self.skill_path = label("", "PathHint")
        self.skill_path.setSizePolicy(QSizePolicy.Policy.Ignored,
                                      QSizePolicy.Policy.Preferred)
        self.skill_path.setTextInteractionFlags(
            Qt.TextInteractionFlag.TextSelectableByMouse)
        head.addWidget(self.skill_path, 1)
        card_layout.addLayout(head)
        card_layout.addWidget(hline())

        self.skill_table = DataTable(SKILL_COLUMNS, key="id", multi=False, parent=self)
        self.skill_table.doubleClicked.connect(
            lambda _index: self.adjust_skill_targets())
        self.skill_table.selectionModel().selectionChanged.connect(
            lambda *_: self._refresh_skill_detail())
        card_layout.addWidget(self.skill_table, 1)

        self.skill_empty = EmptyState(
            "技能库还是空的",
            "点「添加技能」选一个含 SKILL.md 的目录，或把技能直接放进 data/skills")
        card_layout.addWidget(self.skill_empty, 1)
        layout.addWidget(card, 1)

        self.skill_detail = self._build_skill_detail()
        self.skill_detail.setVisible(False)
        layout.addWidget(self.skill_detail)
        return page

    def _build_skill_detail(self) -> QWidget:
        card = Card(padding=14, spacing=9)
        card.setMaximumHeight(214)
        top = QHBoxLayout()
        top.setSpacing(8)
        self.skill_name = label("", "SectionTitle")
        self.skill_name.setMaximumWidth(420)
        top.addWidget(self.skill_name)
        self.skill_state = Badge("—", "muted")
        top.addWidget(self.skill_state)
        self.skill_meta = label("", "SectionHint")
        self.skill_meta.setSizePolicy(QSizePolicy.Policy.Ignored,
                                      QSizePolicy.Policy.Preferred)
        top.addWidget(self.skill_meta, 1)
        top.addWidget(self._action("立即同步", "ghost", "refresh",
                                   self.sync_skill))
        top.addWidget(self._action("调整平台", "ghost", "hub",
                                   self.adjust_skill_targets))
        card.body.addLayout(top)

        self.skill_desc = label("", "ValueLabel")
        self.skill_desc.setWordWrap(True)
        self.skill_desc.setMaximumHeight(72)
        card.body.addWidget(self.skill_desc)

        self.skill_chips = QHBoxLayout()
        self.skill_chips.setSpacing(6)
        card.body.addLayout(self.skill_chips)
        return card

    # -------------------------------------------------------------- MCP 页
    def _build_mcp(self) -> QWidget:
        page = QWidget()
        layout = QVBoxLayout(page)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(12)

        toolbar = Card(padding=12, spacing=8)
        row = QHBoxLayout()
        row.setSpacing(6)
        row.addWidget(self._action("从 JSON 导入", "primary", "external",
                                   self.quick_import_mcp))
        row.addWidget(self._action("从平台导入", "ghost", "archive", self.import_mcp))
        row.addWidget(self._action("添加 MCP", "ghost", "plus", self.add_mcp))
        row.addWidget(self._action("编辑", "ghost", "settings", self.edit_mcp))
        row.addWidget(self._action("调整平台", "ghost", "hub", self.adjust_mcp_targets))
        row.addWidget(self._action("移除", "danger-ghost", "trash", self.remove_mcp))
        self.mcp_summary = label("正在读取 MCP 配置…", "SectionHint")
        self.mcp_summary.setSizePolicy(QSizePolicy.Policy.Ignored,
                                       QSizePolicy.Policy.Preferred)
        self.mcp_summary.setAlignment(
            Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter)
        row.addWidget(self.mcp_summary, 1)
        toolbar.body.addLayout(row)
        layout.addWidget(toolbar)

        card = QFrame()
        card.setObjectName("Card")
        card_layout = QVBoxLayout(card)
        card_layout.setContentsMargins(0, 0, 0, 0)
        card_layout.setSpacing(0)
        head = QHBoxLayout()
        head.setContentsMargins(16, 12, 16, 10)
        head.setSpacing(8)
        head.addWidget(label("统一管理", "SectionTitle"))
        head.addWidget(label("各平台格式自动适配（标准 mcpServers / ZCode mcp.servers）；"
                             "只改同名项，写前备份", "SectionHint"))
        head.addStretch(1)
        card_layout.addLayout(head)
        card_layout.addWidget(hline())

        self.mcp_table = DataTable(MCP_COLUMNS, key="id", multi=False, parent=self)
        self.mcp_table.doubleClicked.connect(lambda _index: self.edit_mcp())
        self.mcp_table.selectionModel().selectionChanged.connect(
            lambda *_: self._refresh_mcp_detail())
        card_layout.addWidget(self.mcp_table, 1)

        self.mcp_empty = EmptyState(
            "还没有登记 MCP",
            "点「从 JSON 导入」直接粘贴配置，或「从平台导入」自动扫描；不用逐个平台改")
        card_layout.addWidget(self.mcp_empty, 1)
        layout.addWidget(card, 1)

        self.mcp_detail = self._build_mcp_detail()
        self.mcp_detail.setVisible(False)
        layout.addWidget(self.mcp_detail)
        return page

    def _build_mcp_detail(self) -> QWidget:
        card = Card(padding=14, spacing=8)
        card.setMaximumHeight(300)
        top = QHBoxLayout()
        top.setSpacing(8)
        self.mcp_name = label("", "SectionTitle")
        self.mcp_name.setMaximumWidth(420)
        top.addWidget(self.mcp_name)
        self.mcp_state = Badge("—", "muted")
        top.addWidget(self.mcp_state)
        self.mcp_meta = label("", "SectionHint")
        self.mcp_meta.setSizePolicy(QSizePolicy.Policy.Ignored,
                                    QSizePolicy.Policy.Preferred)
        top.addWidget(self.mcp_meta, 1)
        top.addWidget(self._action("立即同步", "ghost", "refresh", self.sync_mcp))
        top.addWidget(self._action("编辑", "ghost", "settings", self.edit_mcp))
        card.body.addLayout(top)

        self.mcp_json = QPlainTextEdit()
        self.mcp_json.setObjectName("JsonEditor")
        self.mcp_json.setReadOnly(True)
        self.mcp_json.setMaximumHeight(132)
        card.body.addWidget(self.mcp_json)

        self.mcp_chips = QHBoxLayout()
        self.mcp_chips.setSpacing(6)
        card.body.addLayout(self.mcp_chips)
        return card

    # ------------------------------------------------------------------ 数据
    def on_show(self) -> None:
        if time.monotonic() - self._loaded_at > 5:
            self.reload()

    def reload(self) -> None:
        if self.busy:
            return
        self._set_busy(True, "正在扫描技能与 MCP…")

        def done(state: dict):
            self._set_busy(False)
            self._loaded_at = time.monotonic()
            self.state = state
            self._fill()

        def failed(message: str):
            self._set_busy(False)
            self.log(f"读取技能 / MCP 失败：{message}", "error")
            self.error("读取失败", message.splitlines()[0])

        tasks.submit(lambda _emit: service.snapshot(), on_done=done, on_error=failed)

    def _fill(self) -> None:
        state = self.state
        hub = state.get("hub") or store.defaults()
        self.auto_check.blockSignals(True)
        self.auto_check.setChecked(bool(hub.get("auto_sync", True)))
        self.auto_check.blockSignals(False)
        self.skill_path.setText(state.get("library") or "")
        self.skill_path.setToolTip(state.get("library") or "")
        # 平台技能跟着快照一起到，不用用户手动点刷新
        self._platform_items = list(state.get("platform_skills") or [])
        self._fill_skills()
        self._fill_mcp()
        self._fill_platform_skills()

    def _fill_skills(self) -> None:
        rows = []
        for item in self.state.get("skills") or []:
            tone = STATE_STYLE.get(item["state"], ("—", "muted"))
            description = item.get("description") or item.get("reason") or "—"
            rows.append({
                "id": item["name"],
                "name": item["name"],
                "description": description,
                "platforms": _platform_text(item["targets"]),
                "state": tone[0],
                "updated": _short_ts(item.get("updated")),
                "_fg_map": {"state": PALETTE[tone[1]]},
                "_tip_map": {
                    "description": description,
                    "platforms": _platform_tip(item["targets"]),
                },
            })
        current = self.skill_table.current_row()
        keep = current.get("id") if current else None
        self.skill_table.set_rows(rows)
        self.skill_empty.setVisible(not rows)
        self.skill_table.setVisible(bool(rows))
        if keep:
            self.skill_table.select_key(keep)
        self.btn_skills.setText(f"技能 · {len(rows)}")
        untracked = self.state.get("skill_untracked") or []
        if rows:
            synced = sum(1 for item in self.state["skills"]
                         if item["state"] == "synced")
            text = f"{len(rows)} 个技能，{synced} 个已全部同步"
            if untracked:
                text += f"；平台另有 {len(untracked)} 个未纳入"
            self.skill_summary.setText(text)
        elif untracked:
            self.skill_summary.setText(
                f"平台已有 {len(untracked)} 个技能，点「从平台导入」纳入管理")
        else:
            self.skill_summary.setText("技能库为空")
        self.skill_summary.setToolTip(self.skill_summary.text())
        self._refresh_skill_detail()

    def _fill_mcp(self) -> None:
        rows = []
        for item in self.state.get("mcp") or []:
            tone = STATE_STYLE.get(item["state"], ("—", "muted"))
            rows.append({
                "id": item["name"],
                "name": item["name"],
                "kind": item["kind"],
                "where": item["summary"],
                "platforms": _platform_text(item["targets"]),
                "state": tone[0],
                "_fg_map": {"state": PALETTE[tone[1]]},
                "_tip_map": {
                    "where": item["summary"],
                    "platforms": _platform_tip(item["targets"]),
                },
            })
        current = self.mcp_table.current_row()
        keep = current.get("id") if current else None
        self.mcp_table.set_rows(rows)
        self.mcp_empty.setVisible(not rows)
        self.mcp_table.setVisible(bool(rows))
        if keep:
            self.mcp_table.select_key(keep)
        self.btn_mcp.setText(f"MCP · {len(rows)}")
        errors = self.state.get("mcp_errors") or {}
        untracked = self.state.get("mcp_untracked") or []
        tip = ""
        if errors:
            self.mcp_summary.setText(f"{len(rows)} 个 MCP，{len(errors)} 个平台读取失败")
        elif rows:
            synced = sum(1 for item in self.state["mcp"] if item["state"] == "synced")
            text = f"{len(rows)} 个已登记，{synced} 个已同步"
            if untracked:
                text += f"；另有 {len(untracked)} 条未纳入"
                tip = "点「从平台导入」把平台现有 MCP 纳入统一管理"
            self.mcp_summary.setText(text)
        elif untracked:
            self.mcp_summary.setText(f"平台已有 {len(untracked)} 条未纳入")
            tip = "点「从平台导入」把平台现有 MCP 纳入统一管理"
        else:
            self.mcp_summary.setText("还没有登记 MCP")
        self.mcp_summary.setToolTip(tip or self.mcp_summary.text())
        self._refresh_mcp_detail()

    # ------------------------------------------------------------------ 详情
    def _refresh_skill_detail(self) -> None:
        row = self.skill_table.current_row()
        item = self._skill_item(row.get("id")) if row else None
        if item is None:
            self.skill_detail.setVisible(False)
            return
        self.skill_detail.setVisible(True)
        self.skill_name.setText(item["name"])
        self.skill_name.setToolTip(item["name"])
        tone = STATE_STYLE.get(item["state"], ("—", "muted"))
        self.skill_state.set(tone[0], tone[1])
        self.skill_meta.setText(
            f"{item['files']} 个文件 · {item['size'] / 1024:.0f} KB · {item['path']}")
        self.skill_meta.setToolTip(item["path"])
        description = item.get("description") or item.get("reason") or "—"
        self.skill_desc.setText(description)
        self.skill_desc.setToolTip(description)
        _clear_layout(self.skill_chips)
        if not item["targets"]:
            self.skill_chips.addWidget(label("还没有分配平台", "KeyLabel"))
        for target in targets.TARGETS:
            if target.key not in item["targets"]:
                continue
            state = (item.get("states") or {}).get(target.key, "missing")
            text, tone_name = {
                "synced": ("已同步", "ok"),
                "stale": ("待更新", "warn"),
                "missing": ("待同步", "warn"),
                "untracked": ("平台已有", "warn"),
            }.get(state, ("待同步", "warn"))
            chip = Badge(f"{target.label} · {text}", tone_name)
            chip.setMaximumWidth(250)
            chip.setToolTip(str(target.skill_path(item["name"])))
            self.skill_chips.addWidget(chip)
        self.skill_chips.addStretch(1)

    def _refresh_mcp_detail(self) -> None:
        row = self.mcp_table.current_row()
        item = self._mcp_item(row.get("id")) if row else None
        if item is None:
            self.mcp_detail.setVisible(False)
            return
        self.mcp_detail.setVisible(True)
        self.mcp_name.setText(item["name"])
        self.mcp_name.setToolTip(item["name"])
        tone = STATE_STYLE.get(item["state"], ("—", "muted"))
        self.mcp_state.set(tone[0], tone[1])
        meta = f"{item['kind']} · {item['summary']}"
        if item.get("preserve"):
            meta += " · 保留各平台原配置"
        self.mcp_meta.setText(meta)
        self.mcp_meta.setToolTip(meta)
        self.mcp_json.setPlainText(
            json.dumps(mcp.redact(item.get("config") or {}),
                       ensure_ascii=False, indent=2))
        _clear_layout(self.mcp_chips)
        if not item["targets"]:
            self.mcp_chips.addWidget(label("还没有分配平台", "KeyLabel"))
        for target in targets.TARGETS:
            if target.key not in item["targets"]:
                continue
            state = (item.get("states") or {}).get(target.key, "missing")
            text, tone_name = {
                "synced": ("已同步", "ok"),
                "stale": ("待更新", "warn"),
                "missing": ("待同步", "warn"),
                "untracked": ("平台已有", "warn"),
            }.get(state, ("待同步", "warn"))
            chip = Badge(f"{target.label} · {text}", tone_name)
            chip.setMaximumWidth(250)
            chip.setToolTip(str(target.mcp_file))
            self.mcp_chips.addWidget(chip)
        self.mcp_chips.addStretch(1)

    def _skill_item(self, name) -> dict | None:
        return next((item for item in self.state.get("skills") or []
                     if item["name"] == name), None)

    def _mcp_item(self, name) -> dict | None:
        return next((item for item in self.state.get("mcp") or []
                     if item["name"] == name), None)

    # ------------------------------------------------------------------ 动作
    def _run(self, work, busy_text: str, on_done=None) -> None:
        if self.busy:
            self.notify("正在处理上一个操作，请稍候")
            return
        self._set_busy(True, busy_text)

        def done(result):
            self._set_busy(False)
            if on_done is not None:
                on_done(result)
            else:
                self.reload()

        def failed(message: str):
            self._set_busy(False)
            self.log(message, "error")
            self.error("操作失败", message.splitlines()[0])

        tasks.submit(lambda _emit: work(), on_done=done, on_error=failed, serial=True )

    def _default_targets(self) -> list[str]:
        hub = self.state.get("hub") or {}
        saved = [key for key in hub.get("default_targets") or []
                 if key in targets.TARGET_MAP]
        if saved:
            return saved
        return [key for key in targets.TARGET_KEYS
                if targets.TARGET_MAP[key].installed]

    def _report(self, batch, title: str) -> None:
        if batch.conflicts:
            lines = [f"· {targets.TARGET_MAP[step.target].label}：{step.message}"
                     for step in batch.conflicts if step.target in targets.TARGET_MAP]
            self.warn("部分平台未同步",
                      f"{title}\n\n" + "\n".join(lines)
                      + "\n\n这些平台本来就有同名条目。确认要覆盖时，"
                        "在详情里点「立即同步」。")
        if batch.failed:
            lines = [f"· {targets.TARGET_MAP[step.target].label}：{step.message}"
                     for step in batch.failed if step.target in targets.TARGET_MAP]
            self.error("同步失败", "\n".join(lines[:8]))
        self.log(f"{title}：{batch.summary()}",
                 "warn" if batch.failed or batch.conflicts else "ok")

    # -------------------------------------------------------------- 技能动作
    def add_skill(self) -> None:
        folder = QFileDialog.getExistingDirectory(
            self, "选择技能目录（需包含 SKILL.md）", str(Path.home()))
        if not folder:
            return
        src = Path(folder)
        if store.skill_md(src) is None:
            self.error("不是有效技能", f"{src} 里没有 SKILL.md")
            return
        name = src.name
        duplicate = any(item["name"].lower() == name.lower()
                        for item in self.state.get("skills") or [])
        if duplicate and not self.confirm(
                "覆盖同名技能", f"技能库里已经有「{name}」，用这次选的目录覆盖它？"):
            return
        picker = TargetPickerDialog(
            self, self._default_targets(), title=f"把「{name}」同步到哪些平台")
        if not picker.exec():
            return
        keys = picker.selected_keys()

        def work():
            result = service.add_skill(src, keys, overwrite=duplicate)
            service.set_default_targets(keys)
            return result

        def done(result):
            self._report(result["batch"], f"添加技能「{result['name']}」")
            self.reload()

        self._run(work, "正在添加技能…", on_done=done)

    def import_skills(self) -> None:
        def scanned(items):
            if not items:
                self.info("没有可导入的技能",
                          "各平台技能目录里没有找到带 SKILL.md 的技能。")
                return
            dialog = ImportDialog(
                self, "从平台导入技能",
                "扫描到以下技能；勾选后导入会复制进技能库，并保留它原来的平台归属。",
                items)
            if not dialog.exec():
                return
            chosen = dialog.selected()
            if not chosen:
                return

            def done(result):
                text = f"导入 {len(result['imported'])} 个"
                if result["skipped"]:
                    text += f"，跳过 {len(result['skipped'])} 个"
                if result["errors"]:
                    text += f"，失败 {len(result['errors'])} 个"
                self.log(text, "ok" if not result["errors"] else "warn")
                if result["errors"]:
                    self.warn("部分技能导入失败", "\n".join(result["errors"][:8]))
                self.reload()

            self._run(lambda: service.import_skills(chosen),
                      "正在导入技能…", on_done=done)

        self._run(service.scan_skill_sources, "正在扫描各平台技能…",
                  on_done=scanned)

    def adjust_skill_targets(self) -> None:
        row = self.skill_table.current_row()
        item = self._skill_item(row.get("id")) if row else None
        if item is None:
            self.info("先选一个技能", "在表格里选中要调整的技能。")
            return
        picker = TargetPickerDialog(
            self, item["targets"], title=f"「{item['name']}」同步到哪些平台")
        if not picker.exec():
            return
        keys = picker.selected_keys()
        removed = set(item["targets"]) - set(keys)
        if removed:
            names = "、".join(targets.labels(sorted(removed)))
            if not self.confirm("取消平台",
                                f"将从 {names} 删除本工具部署的副本，继续？"):
                return
        self._run(lambda: service.set_skill_targets(item["name"], keys),
                  "正在调整同步平台…",
                  on_done=lambda result: (self._report(result["batch"],
                                                       f"调整「{item['name']}」"),
                                          self.reload()))

    def sync_skill(self) -> None:
        row = self.skill_table.current_row()
        item = self._skill_item(row.get("id")) if row else None
        if item is None:
            return
        untracked = [targets.TARGET_MAP[key].label
                     for key, value in (item.get("states") or {}).items()
                     if value == "untracked" and key in targets.TARGET_MAP]
        if untracked and not self.confirm(
                "覆盖平台同名技能",
                "以下平台里已有同名技能，且不是本工具部署的：\n\n"
                + "、".join(untracked)
                + "\n\n继续会用技能库里的版本覆盖它们。"):
            return
        name = item["name"]
        self._run(lambda: service.sync_now(name, "skill"), "正在同步技能…",
                  on_done=lambda result: (self._report(result["batch"],
                                                       f"同步「{name}」"),
                                          self.reload()))

    def remove_skill(self) -> None:
        row = self.skill_table.current_row()
        item = self._skill_item(row.get("id")) if row else None
        if item is None:
            self.info("先选一个技能", "在表格里选中要移除的技能。")
            return
        lines = [f"· {targets.TARGET_MAP[key].label}："
                 f"{targets.TARGET_MAP[key].skill_path(item['name'])}"
                 for key in item["targets"] if key in targets.TARGET_MAP]
        lines.append(f"· 技能库：{item['path'] or '（源目录已丢失）'}")
        if not self.confirm("移除技能",
                            f"将删除「{item['name']}」的以下内容：\n\n"
                            + "\n".join(lines)
                            + "\n\n平台副本与技能库源目录都会移到 data/trash，不会直接抹掉。"):
            return
        name = item["name"]

        def done(result):
            self._report(result["batch"], f"移除「{name}」")
            self.reload()

        self._run(lambda: service.remove_skill(name), "正在移除技能…", on_done=done)

    # -------------------------------------------------------------- MCP 动作
    def quick_import_mcp(self) -> None:
        """粘贴 / 选文件，一次识别所有 MCP，再选平台即可写完。"""
        dialog = McpQuickImportDialog(self)
        if not dialog.exec():
            return
        entries = dialog.values()
        if not entries:
            return
        picker = TargetPickerDialog(
            self, self._default_targets(),
            title=f"把这 {len(entries)} 个 MCP 写入哪些平台")
        if not picker.exec():
            return
        keys = picker.selected_keys()

        def work():
            result = service.import_payload(entries, keys)
            service.set_default_targets(keys)
            return result

        def done(result):
            self._report(result["batch"], f"导入 {len(result['imported'])} 个 MCP")
            if result["skipped"]:
                self.info("部分条目已存在",
                          "以下名称已在统一管理中，已跳过：\n\n"
                          + "、".join(result["skipped"][:8]))
            self.reload()

        self._run(work, "正在导入 MCP…", on_done=done)

    def add_mcp(self) -> None:
        dialog = McpEditorDialog(self)
        if not dialog.exec():
            return
        name, config = dialog.values()
        if name in (self.state.get("hub") or {}).get("mcp", {}):
            self.error("名称已存在", f"统一管理里已经有「{name}」，请换一个名称或直接编辑。")
            return
        picker = TargetPickerDialog(
            self, self._default_targets(), title=f"把「{name}」写入哪些平台")
        if not picker.exec():
            return
        keys = picker.selected_keys()

        def work():
            result = service.add_mcp(name, config, keys)
            service.set_default_targets(keys)
            return result

        def done(result):
            self._report(result["batch"], f"添加 MCP「{name}」")
            self.reload()

        self._run(work, "正在写入 MCP…", on_done=done)

    def edit_mcp(self) -> None:
        row = self.mcp_table.current_row()
        item = self._mcp_item(row.get("id")) if row else None
        if item is None:
            self.info("先选一条 MCP", "在表格里选中要编辑的 MCP。")
            return
        dialog = McpEditorDialog(self, item["name"], item.get("config") or {})
        if not dialog.exec():
            return
        _name, config = dialog.values()
        name = item["name"]

        def done(result):
            self._report(result["batch"], f"更新 MCP「{name}」")
            self.reload()

        self._run(lambda: service.update_mcp(name, config), "正在更新 MCP…",
                  on_done=done)

    def adjust_mcp_targets(self) -> None:
        row = self.mcp_table.current_row()
        item = self._mcp_item(row.get("id")) if row else None
        if item is None:
            self.info("先选一条 MCP", "在表格里选中要调整的 MCP。")
            return
        picker = TargetPickerDialog(
            self, item["targets"], title=f"「{item['name']}」写入哪些平台")
        if not picker.exec():
            return
        keys = picker.selected_keys()
        removed = set(item["targets"]) - set(keys)
        if removed:
            names = "、".join(targets.labels(sorted(removed)))
            if not self.confirm("取消平台",
                                f"将从 {names} 的配置里移除「{item['name']}」，继续？"):
                return
        self._run(lambda: service.set_mcp_targets(item["name"], keys),
                  "正在调整写入平台…",
                  on_done=lambda result: (self._report(result["batch"],
                                                       f"调整「{item['name']}」"),
                                          self.reload()))

    def sync_mcp(self) -> None:
        row = self.mcp_table.current_row()
        item = self._mcp_item(row.get("id")) if row else None
        if item is None:
            return
        untracked = [targets.TARGET_MAP[key].label
                     for key, value in (item.get("states") or {}).items()
                     if value == "untracked" and key in targets.TARGET_MAP]
        if untracked and not self.confirm(
                "覆盖平台同名 MCP",
                "以下平台里已有同名 MCP，且不是本工具写入的：\n\n"
                + "、".join(untracked)
                + "\n\n继续会用统一管理里的配置覆盖它们。"):
            return
        if item.get("preserve") and not self.confirm(
                "统一各平台配置",
                "这条 MCP 导入时保留了各平台原配置。继续会把统一配置写入所有已选平台，"
                "覆盖各平台原来的写法。"):
            return
        name = item["name"]
        self._run(lambda: service.sync_now(name, "mcp"), "正在同步 MCP…",
                  on_done=lambda result: (self._report(result["batch"],
                                                       f"同步「{name}」"),
                                          self.reload()))

    def remove_mcp(self) -> None:
        row = self.mcp_table.current_row()
        item = self._mcp_item(row.get("id")) if row else None
        if item is None:
            self.info("先选一条 MCP", "在表格里选中要移除的 MCP。")
            return
        lines = [f"· {targets.TARGET_MAP[key].label}："
                 f"{targets.TARGET_MAP[key].mcp_file}"
                 for key in item["targets"] if key in targets.TARGET_MAP]
        if not self.confirm("移除 MCP",
                            f"将从以下平台配置里删除「{item['name']}」：\n\n"
                            + ("\n".join(lines) if lines else "（还没有写入任何平台）")):
            return
        name = item["name"]

        def done(result):
            self._report(result["batch"], f"移除 MCP「{name}」")
            self.reload()

        self._run(lambda: service.remove_mcp(name), "正在移除 MCP…", on_done=done)

    def import_mcp(self) -> None:
        def scanned(items):
            if not items:
                self.info("没有可导入的 MCP",
                          "各平台配置里没有读到 mcpServers 条目。")
                return
            dialog = ImportDialog(
                self, "从平台导入 MCP",
                "同名配置会合并成一条；各平台保留自己原来的写法，只有以后新增的平台"
                "使用合并后的配置，不会把现有平台改坏。",
                items, detail_key="summary", checked_by_default=True)
            if not dialog.exec():
                return
            chosen = dialog.selected()
            if not chosen:
                return

            def done(result):
                text = f"导入 {len(result['imported'])} 条 MCP"
                if result["skipped"]:
                    text += f"，跳过 {len(result['skipped'])} 条"
                self.log(text + "（各平台保留原配置）", "ok")
                if result["skipped"]:
                    self.info("部分条目已存在", "\n".join(result["skipped"][:8]))
                self.reload()

            self._run(lambda: service.import_mcp(chosen), "正在导入 MCP…",
                      on_done=done)

        self._run(service.scan_mcp_sources, "正在读取各平台 MCP…", on_done=scanned)

    # ------------------------------------------------------------------ 全局
    def sync_all(self) -> None:
        if not self.confirm(
                "立即同步",
                "将把已登记的技能与 MCP 重新推送到各自平台。\n"
                "平台里同名但不是本工具管理的条目会被覆盖。继续？"):
            return

        def done(result):
            self._report(result["batch"], "全部同步")
            self.reload()

        self._run(service.sync_all, "正在同步全部…", on_done=done)

    def open_library(self) -> None:
        def done(path):
            if not winproc.open_path(path):
                self.warn("打不开技能库", f"目录不存在或系统拒绝打开：{path}")

        self._run(service.ensure_library, "正在打开技能库…", on_done=done)

    def _toggle_auto(self, _state: int) -> None:
        if self.busy:
            self.auto_check.blockSignals(True)
            self.auto_check.setChecked(
                bool((self.state.get("hub") or {}).get("auto_sync", True)))
            self.auto_check.blockSignals(False)
            self.notify("正在处理其他操作，稍后再试")
            return
        enabled = self.auto_check.isChecked()

        def done(_result):
            self.log(f"启动时自动同步：{'开启' if enabled else '关闭'}", "dim")

        def failed(message: str):
            self.log(f"保存自动同步开关失败：{message}", "error")

        tasks.submit(lambda _emit: service.set_auto_sync(enabled),
                     on_done=done, on_error=failed, serial=True )
