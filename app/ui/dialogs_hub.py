# -*- coding: utf-8 -*-
"""技能 / MCP 页面的三个对话框：平台多选、MCP 编辑、从平台导入。"""
from __future__ import annotations

import json
import shutil
from pathlib import Path

from PySide6.QtCore import Qt, QTimer
from PySide6.QtGui import QGuiApplication
from PySide6.QtWidgets import (QCheckBox, QComboBox, QDialog, QFileDialog, QFrame,
                               QHBoxLayout, QLineEdit, QListWidget, QListWidgetItem,
                               QPlainTextEdit, QScrollArea, QSizePolicy,
                               QVBoxLayout, QWidget)

from ..hub import mcp, targets
from .widgets import Badge, hline, label, make_button

STDIO_TEMPLATE = {
    "command": "npx",
    "args": ["-y", "@modelcontextprotocol/server-filesystem", "D:/"],
    "env": {},
}
HTTP_TEMPLATE = {
    "type": "http",
    "url": "https://example.com/mcp",
    "headers": {},
}


def _template(kind: str) -> dict:
    return HTTP_TEMPLATE if kind in ("http", "sse") else STDIO_TEMPLATE


class TargetPickerDialog(QDialog):
    """勾选要同步到的平台；未安装的也能选，目录会在首次同步时创建。"""

    def __init__(self, parent, selected=(), title: str = "选择同步平台",
                 subtitle: str = "已默认勾选本机检测到的平台；未检测到的也可以选，"
                                 "目录会在首次同步时自动创建。"):
        super().__init__(parent)
        self.setWindowTitle(title)
        self.setMinimumSize(720, 560)
        self.resize(780, 720)
        self.setModal(True)
        self.checks: dict[str, QCheckBox] = {}

        root = QVBoxLayout(self)
        root.setContentsMargins(24, 20, 24, 20)
        root.setSpacing(10)
        root.addWidget(label(title, "PageTitle"))
        hint = label(subtitle, "PageSub")
        hint.setWordWrap(True)
        root.addWidget(hint)
        root.addWidget(hline())

        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setFrameShape(QFrame.Shape.NoFrame)
        scroll.setHorizontalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAlwaysOff)
        holder = QWidget()
        column = QVBoxLayout(holder)
        column.setContentsMargins(0, 0, 6, 0)
        column.setSpacing(10)

        chosen = set(selected or ())
        for group in targets.GROUPS:
            group_targets = [target for target in targets.TARGETS
                             if target.group == group]
            if not group_targets:
                continue
            column.addWidget(label(group, "SectionTitle"))
            for target in group_targets:
                column.addWidget(self._row(target, target.key in chosen))
        column.addStretch(1)
        scroll.setWidget(holder)
        root.addWidget(scroll, 1)

        bottom = QHBoxLayout()
        bottom.setSpacing(8)
        all_btn = make_button("全选", "ghost")
        all_btn.clicked.connect(lambda: self._set_all(True))
        none_btn = make_button("清空", "ghost")
        none_btn.clicked.connect(lambda: self._set_all(False))
        bottom.addWidget(all_btn)
        bottom.addWidget(none_btn)
        bottom.addStretch(1)
        cancel = make_button("取消", "ghost")
        cancel.clicked.connect(self.reject)
        bottom.addWidget(cancel)
        ok = make_button("确定", "primary")
        ok.clicked.connect(self.accept)
        bottom.addWidget(ok)
        root.addLayout(bottom)

    def _row(self, target: targets.Target, checked: bool) -> QWidget:
        box = QFrame()
        box.setObjectName("CardFlat")
        layout = QVBoxLayout(box)
        layout.setContentsMargins(12, 8, 12, 8)
        layout.setSpacing(3)

        top = QHBoxLayout()
        top.setSpacing(8)
        check = QCheckBox(target.label)
        check.setChecked(checked)
        self.checks[target.key] = check
        top.addWidget(check)
        if target.installed:
            top.addWidget(Badge("已安装", "ok"))
        else:
            top.addWidget(Badge("未检测到", "warn"))
        if target.note:
            note = label(target.note, "KeyLabel")
            note.setSizePolicy(QSizePolicy.Policy.Ignored,
                               QSizePolicy.Policy.Preferred)
            note.setToolTip(target.note)
            top.addWidget(note)
        top.addStretch(1)
        layout.addLayout(top)

        skill = label(f"技能  {targets.short_path(target.skill_dir)}", "PathHint")
        skill.setSizePolicy(QSizePolicy.Policy.Ignored, QSizePolicy.Policy.Preferred)
        skill.setToolTip(str(target.skill_dir))
        layout.addWidget(skill)
        mcp = label(f"MCP   {targets.short_path(target.mcp_file)}", "PathHint")
        mcp.setSizePolicy(QSizePolicy.Policy.Ignored, QSizePolicy.Policy.Preferred)
        mcp.setToolTip(str(target.mcp_file))
        layout.addWidget(mcp)
        return box

    def _set_all(self, value: bool) -> None:
        for check in self.checks.values():
            check.setChecked(value)

    def selected_keys(self) -> list[str]:
        return [key for key, check in self.checks.items() if check.isChecked()]


class McpEditorDialog(QDialog):
    """新增 / 编辑一条 MCP：名称 + 一段 JSON，避免为每种字段做一套表单。"""

    def __init__(self, parent, name: str = "", config: dict | None = None):
        super().__init__(parent)
        self.editing = bool(name)
        self.setWindowTitle("编辑 MCP" if self.editing else "添加 MCP")
        self.setMinimumSize(720, 600)
        self.setModal(True)

        root = QVBoxLayout(self)
        root.setContentsMargins(24, 20, 24, 20)
        root.setSpacing(10)
        root.addWidget(label("编辑 MCP" if self.editing else "添加 MCP", "PageTitle"))
        hint = label("名称是各平台配置里的键；下面填标准 MCP 配置，"
                     "type 可填 stdio / http / sse。写入时会自动适配各平台格式。",
                     "PageSub")
        hint.setWordWrap(True)
        root.addWidget(hint)

        name_row = QHBoxLayout()
        name_row.setSpacing(8)
        name_row.addWidget(label("名称", "StatLabel"))
        self.name = QLineEdit(name)
        self.name.setPlaceholderText("例如 yunshu-mcp")
        self.name.setReadOnly(self.editing)
        name_row.addWidget(self.name, 1)
        name_row.addWidget(label("类型", "StatLabel"))
        self.kind = QComboBox()
        self.kind.addItems(["stdio", "http", "sse"])
        self.kind.setFixedWidth(96)
        name_row.addWidget(self.kind)
        template = make_button("插入模板", "ghost")
        template.clicked.connect(self._insert_template)
        name_row.addWidget(template)
        paste = make_button("从剪贴板导入", "ghost")
        paste.clicked.connect(self._paste)
        name_row.addWidget(paste)
        root.addLayout(name_row)

        self.editor = QPlainTextEdit()
        self.editor.setObjectName("JsonEditor")
        self.editor.setPlaceholderText('{\n  "command": "npx",\n  "args": ["-y", "some-mcp"]\n}')
        root.addWidget(self.editor, 1)

        self.error = label("", "ErrorText")
        root.addWidget(self.error)

        bottom = QHBoxLayout()
        bottom.addStretch(1)
        cancel = make_button("取消", "ghost")
        cancel.clicked.connect(self.reject)
        bottom.addWidget(cancel)
        save = make_button("保存", "primary")
        save.clicked.connect(self._save)
        bottom.addWidget(save)
        root.addLayout(bottom)

        if config:
            self.editor.setPlainText(json.dumps(config, ensure_ascii=False, indent=2))
            self.kind.setCurrentText(_kind_of(config))
        else:
            self.editor.setPlainText(json.dumps(STDIO_TEMPLATE, ensure_ascii=False, indent=2))
        # 用户没动类型下拉时保留原 type（例如 streamable-http），不要擅自改成 http
        self._initial_kind = self.kind.currentText()

    def _insert_template(self) -> None:
        self.editor.setPlainText(
            json.dumps(_template(self.kind.currentText()), ensure_ascii=False, indent=2))

    def _paste(self) -> None:
        text = QGuiApplication.clipboard().text().strip()
        if not text:
            self.error.setText("剪贴板是空的")
            return
        try:
            data = json.loads(text)
        except ValueError as exc:
            self.error.setText(f"剪贴板不是合法 JSON：{exc}")
            return
        config = data
        if isinstance(data, dict) and isinstance(data.get("mcpServers"), dict) \
                and data["mcpServers"]:
            first_name, first_config = next(iter(data["mcpServers"].items()))
            config = first_config
            if not self.name.text().strip():
                self.name.setText(str(first_name))
        if not isinstance(config, dict):
            self.error.setText("剪贴板里的 MCP 配置不是对象")
            return
        self.editor.setPlainText(json.dumps(config, ensure_ascii=False, indent=2))
        self.kind.setCurrentText(_kind_of(config))
        self.error.setText("")

    def _save(self) -> None:
        name = self.name.text().strip()
        if not name:
            self.error.setText("名称不能为空")
            return
        try:
            config = json.loads(self.editor.toPlainText())
        except ValueError as exc:
            self.error.setText(f"JSON 解析失败：{exc}")
            return
        if not isinstance(config, dict) or not config:
            self.error.setText("配置必须是一个非空对象")
            return
        kind = self.kind.currentText()
        if kind == "stdio" and not config.get("command"):
            self.error.setText("stdio 类型必须填写 command")
            return
        if kind in ("http", "sse") and not config.get("url"):
            self.error.setText(f"{kind} 类型必须填写 url")
            return
        if not config.get("command") and not config.get("url"):
            self.error.setText("必须填写 command（stdio）或 url（http / sse）")
            return
        if kind != self._initial_kind:
            config["type"] = kind
        else:
            config.setdefault("type", kind)
        self.editor.setPlainText(json.dumps(config, ensure_ascii=False, indent=2))
        self.error.setText("")
        self.accept()

    def values(self) -> tuple[str, dict]:
        config = json.loads(self.editor.toPlainText())
        return self.name.text().strip(), config


class McpQuickImportDialog(QDialog):
    """直接粘贴 / 选择一份 JSON，自动识别所有 MCP 并转成各平台格式。"""

    def __init__(self, parent):
        super().__init__(parent)
        self.setWindowTitle("从 JSON 导入 MCP")
        self.setMinimumSize(760, 600)
        self.setModal(True)
        self.entries: list[tuple[str, dict]] = []

        root = QVBoxLayout(self)
        root.setContentsMargins(24, 20, 24, 20)
        root.setSpacing(10)
        root.addWidget(label("从 JSON 导入 MCP", "PageTitle"))
        hint = label("支持标准 mcpServers、ZCode 的 mcp.servers、单个服务或服务数组；"
                     "识别后会自动转换成各平台格式，不需要你逐个平台改配置。", "PageSub")
        hint.setWordWrap(True)
        root.addWidget(hint)

        row = QHBoxLayout()
        row.setSpacing(8)
        paste = make_button("从剪贴板填充", "ghost")
        paste.clicked.connect(self._from_clipboard)
        row.addWidget(paste)
        choose = make_button("选择 JSON 文件", "ghost", "folder")
        choose.clicked.connect(self._from_file)
        row.addWidget(choose)
        row.addStretch(1)
        root.addLayout(row)

        self.editor = QPlainTextEdit()
        self.editor.setObjectName("JsonEditor")
        self.editor.setPlaceholderText(
            '{\n  "mcpServers": {\n    "example": {\n      "command": "npx",\n'
            '      "args": ["-y", "some-mcp"]\n    }\n  }\n}')
        root.addWidget(self.editor, 1)

        self.preview = label("把 MCP 的 JSON 粘贴到这里，或点「从剪贴板填充」。", "SectionHint")
        self.preview.setWordWrap(True)
        self.preview.setMaximumHeight(150)
        root.addWidget(self.preview)

        bottom = QHBoxLayout()
        bottom.addStretch(1)
        cancel = make_button("取消", "ghost")
        cancel.clicked.connect(self.reject)
        bottom.addWidget(cancel)
        self.btn_import = make_button("导入并同步", "primary")
        self.btn_import.setEnabled(False)
        self.btn_import.clicked.connect(self._save)
        bottom.addWidget(self.btn_import)
        root.addLayout(bottom)

        self._timer = QTimer(self)
        self._timer.setSingleShot(True)
        self._timer.setInterval(250)
        self._timer.timeout.connect(self._parse)
        self.editor.textChanged.connect(self._timer.start)

        text = QGuiApplication.clipboard().text().strip()
        if text[:1] in "{[":
            self.editor.setPlainText(text)
        else:
            self._parse()

    def _from_clipboard(self) -> None:
        text = QGuiApplication.clipboard().text().strip()
        if text:
            self.editor.setPlainText(text)
        else:
            self.preview.setText("剪贴板是空的")

    def _from_file(self) -> None:
        path, _ = QFileDialog.getOpenFileName(
            self, "选择 MCP JSON", str(Path.home()),
            "JSON 配置 (*.json);;所有文件 (*)")
        if not path:
            return
        try:
            text = Path(path).read_text(encoding="utf-8-sig")
        except OSError as exc:
            self.preview.setText(f"读取失败：{exc}")
            return
        self.editor.setPlainText(text)

    def _parse(self) -> None:
        text = self.editor.toPlainText().strip()
        self.entries = []
        if not text:
            self.preview.setText("把 MCP 的 JSON 粘贴到这里，或点「从剪贴板填充」。")
            self.btn_import.setEnabled(False)
            return
        try:
            self.entries = mcp.parse_text(text)
        except mcp.HubError as exc:
            self.preview.setText(str(exc))
            self.btn_import.setEnabled(False)
            return
        lines = [f"· {name}：{mcp.summary(config)}"
                 for name, config in self.entries[:6]]
        if len(self.entries) > 6:
            lines.append(f"· …另有 {len(self.entries) - 6} 个")
        warnings = []
        for name, config in self.entries[:10]:
            command = str(config.get("command") or "")
            if not command:
                continue
            if "/" in command or "\\" in command:
                if not Path(command).exists():
                    warnings.append(f"{name}：命令不存在 {command}")
            elif shutil.which(command) is None:
                warnings.append(f"{name}：PATH 中找不到 {command}")
        text = f"识别到 {len(self.entries)} 个 MCP：\n" + "\n".join(lines)
        if warnings:
            text += "\n\n注意（仍可导入，运行时可能失败）：\n" + "\n".join(warnings[:4])
        self.preview.setText(text)
        self.preview.setToolTip(text)
        self.btn_import.setEnabled(True)

    def _save(self) -> None:
        self._parse()
        if not self.entries:
            return
        self.accept()

    def values(self) -> list[tuple[str, dict]]:
        return self.entries


class ImportDialog(QDialog):
    """从平台现有技能 / MCP 里挑要纳入统一管理的条目。"""

    def __init__(self, parent, title: str, subtitle: str, items: list[dict],
                 detail_key: str = "description", checked_by_default: bool = False):
        super().__init__(parent)
        self.setWindowTitle(title)
        self.setMinimumSize(760, 560)
        self.setModal(True)
        self.items = items

        root = QVBoxLayout(self)
        root.setContentsMargins(24, 20, 24, 20)
        root.setSpacing(10)
        root.addWidget(label(title, "PageTitle"))
        hint = label(subtitle, "PageSub")
        hint.setWordWrap(True)
        root.addWidget(hint)
        root.addWidget(hline())

        self.list = QListWidget()
        self.list.setObjectName("PickList")
        self.list.setWordWrap(True)
        self.list.setResizeMode(QListWidget.ResizeMode.Adjust)
        for item in items:
            detail = str(item.get(detail_key) or item.get("summary") or item.get("path") or "")
            text = f"{item.get('name', '')}\n{item.get('source_label', '')}"
            if detail:
                text += f" · {detail}"
            row = QListWidgetItem(text)
            row.setFlags(Qt.ItemFlag.ItemIsEnabled | Qt.ItemFlag.ItemIsSelectable
                         | Qt.ItemFlag.ItemIsUserCheckable)
            row.setCheckState(Qt.CheckState.Checked if checked_by_default
                              else Qt.CheckState.Unchecked)
            row.setData(Qt.ItemDataRole.UserRole, item)
            self.list.addItem(row)
        root.addWidget(self.list, 1)

        bottom = QHBoxLayout()
        all_btn = make_button("全选", "ghost")
        all_btn.clicked.connect(lambda: self._set_all(True))
        none_btn = make_button("清空", "ghost")
        none_btn.clicked.connect(lambda: self._set_all(False))
        bottom.addWidget(all_btn)
        bottom.addWidget(none_btn)
        bottom.addStretch(1)
        cancel = make_button("取消", "ghost")
        cancel.clicked.connect(self.reject)
        bottom.addWidget(cancel)
        ok = make_button("导入选中", "primary")
        ok.clicked.connect(self.accept)
        bottom.addWidget(ok)
        root.addLayout(bottom)

    def _set_all(self, value: bool) -> None:
        state = Qt.CheckState.Checked if value else Qt.CheckState.Unchecked
        for index in range(self.list.count()):
            self.list.item(index).setCheckState(state)

    def selected(self) -> list[dict]:
        result = []
        for index in range(self.list.count()):
            row = self.list.item(index)
            if row.checkState() == Qt.CheckState.Checked:
                result.append(row.data(Qt.ItemDataRole.UserRole))
        return result


def _kind_of(config: dict) -> str:
    kind = str(config.get("type") or "")
    if kind in ("stdio", "http", "sse"):
        return kind
    return "http" if config.get("url") else "stdio"
