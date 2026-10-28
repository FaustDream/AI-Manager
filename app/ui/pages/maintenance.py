# -*- coding: utf-8 -*-
"""备份与清理：回滚、回收站、黑名单、CodeBuddy 空壳会话清理。"""
from __future__ import annotations

import os
import time

from PySide6.QtCore import Qt
from PySide6.QtWidgets import (QFrame, QHBoxLayout, QScrollArea, QVBoxLayout, QWidget)

from ...core import paths, tasks, winproc
from ...core.format import fmt_size
from ...sessions import backup, codebuddy as cb, service, trash
from ...sessions.editions import Edition
from ..models import Column
from ..theme import PALETTE
from ..widgets import Badge, Card, DataTable, hline, label, make_button
from .base import BasePage

BACKUP_COLS = [
    Column("created", "创建时间", 150, "center"),
    Column("count", "会话数", 80, "right"),
    Column("tag", "备份编号", 170),
    Column("size", "占用", 90, "right"),
    Column("path", "位置", 300, stretch=True),
]

TRASH_COLS = [
    Column("created", "删除时间", 150, "center"),
    Column("count", "会话数", 80, "right"),
    Column("tag", "记录编号", 170),
    Column("size", "占用", 90, "right"),
    Column("path", "回收站位置", 300, stretch=True),
]


class MaintenancePage(BasePage):
    key = "maintenance"
    title = "备份与清理"
    subtitle = ""

    def __init__(self, win):
        super().__init__(win)
        self._refreshing = False
        self._stamp = 0.0
        self._build()
        self.refresh()

    # ------------------------------------------------------------------ 界面
    def _build(self) -> None:
        self.btn_refresh = make_button("刷新", "ghost", "refresh")
        self.btn_refresh.clicked.connect(self.refresh)
        self.header.add_action(self.btn_refresh)

        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setFrameShape(QFrame.Shape.NoFrame)
        holder = QWidget()
        column = QVBoxLayout(holder)
        column.setContentsMargins(0, 0, 6, 0)
        column.setSpacing(12)

        # ---------------------------------------------------------- 备份
        backup_card = Card(padding=0, spacing=0)
        head = QHBoxLayout()
        head.setContentsMargins(16, 12, 16, 12)
        head.setSpacing(8)
        head.addWidget(label("备份与回滚", "SectionTitle"))
        head.addStretch(1)
        open_backup = make_button("打开备份目录", "ghost", "folder")
        open_backup.clicked.connect(lambda: os.startfile(str(paths.BACKUP_ROOT)))
        head.addWidget(open_backup)
        self.btn_rollback = make_button("回滚选中备份", "ghost")
        self.btn_rollback.clicked.connect(self.rollback)
        head.addWidget(self.btn_rollback)
        backup_card.body.addLayout(head)
        backup_card.body.addWidget(hline())
        self.backup_table = DataTable(BACKUP_COLS, key="tag", multi=False, row_height=34)
        self.backup_table.setMinimumHeight(180)
        backup_card.body.addWidget(self.backup_table)
        column.addWidget(backup_card)

        # ---------------------------------------------------------- 回收站
        trash_card = Card(padding=0, spacing=0)
        head2 = QHBoxLayout()
        head2.setContentsMargins(16, 12, 16, 12)
        head2.setSpacing(8)
        head2.addWidget(label("回收站", "SectionTitle"))
        self.trash_hint = label("删除的会话先移入回收站，可在这里恢复；彻底清除才释放磁盘",
                                "SectionHint")
        head2.addWidget(self.trash_hint)
        head2.addStretch(1)
        open_trash = make_button("打开目录", "ghost", "folder")
        open_trash.clicked.connect(lambda: os.startfile(str(paths.TRASH_ROOT)))
        head2.addWidget(open_trash)
        self.btn_purge = make_button("彻底删除选中", "danger-ghost")
        self.btn_purge.clicked.connect(self.purge_selected)
        head2.addWidget(self.btn_purge)
        self.btn_restore = make_button("恢复选中", "ghost")
        self.btn_restore.clicked.connect(self.restore_selected)
        head2.addWidget(self.btn_restore)
        trash_card.body.addLayout(head2)
        trash_card.body.addWidget(hline())
        self.trash_table = DataTable(TRASH_COLS, key="tag", multi=False, row_height=34)
        self.trash_table.setMinimumHeight(170)
        trash_card.body.addWidget(self.trash_table)
        column.addWidget(trash_card)

        # ---------------------------------------------------------- 清理
        clean_card = Card(padding=16, spacing=10)
        clean_card.add_title("清理")

        row1 = QHBoxLayout()
        row1.setSpacing(8)
        self.tomb_badge = Badge("黑名单 —", "muted")
        row1.addWidget(label("会话黑名单", "StatLabel"))
        row1.addWidget(self.tomb_badge)
        self.tomb_hint = label("", "SectionHint")
        row1.addWidget(self.tomb_hint)
        row1.addStretch(1)
        self.btn_clear_tomb = make_button("清空黑名单", "ghost")
        self.btn_clear_tomb.clicked.connect(self.clear_blacklist)
        row1.addWidget(self.btn_clear_tomb)
        clean_card.body.addLayout(row1)
        clean_card.body.addWidget(hline())

        row2 = QHBoxLayout()
        row2.setSpacing(8)
        row2.addWidget(label("CodeBuddy 空壳会话", "StatLabel"))
        self.clean_hint = label("清理没有任何消息的会话，以及清空后残留的空工作区", "SectionHint")
        row2.addWidget(self.clean_hint)
        row2.addStretch(1)
        for text, mode in (("删除无正文", "nobody"), ("删除未命名", "unnamed"),
                           ("删除全部空壳", "junk")):
            button = make_button(text, "danger-ghost")
            button.clicked.connect(lambda _c=False, m=mode: self.cleanup_codebuddy(m))
            row2.addWidget(button)
        clean_card.body.addLayout(row2)
        column.addWidget(clean_card)
        column.addStretch(1)

        scroll.setWidget(holder)
        self.body.addWidget(scroll, 1)

    # ------------------------------------------------------------------ 数据
    def on_show(self) -> None:
        # 统计结果 30 秒内复用，避免每次点菜单都重扫一遍磁盘
        if time.monotonic() - self._stamp > 30:
            self.refresh()

    def refresh(self, *_args) -> None:
        """目录统计与 CodeBuddy 扫描都放到后台，页面切换不再等磁盘。"""
        if self._refreshing:
            return
        self._refreshing = True
        self.btn_refresh.setEnabled(False)
        self.trash_hint.setText("正在统计备份、回收站与会话…")
        # 会话页已经扫过 CodeBuddy 的话直接复用，省掉一次 1.6 秒的全量遍历
        cached = None
        sessions_page = self.win.page("sessions")
        snap = getattr(sessions_page, "snap", None) if sessions_page is not None else None
        if snap is not None and snap.cb_sessions:
            cached = list(snap.cb_sessions)
        tasks.submit(lambda emit: self._collect(emit, cached),
                     on_done=self._apply, on_error=self._load_failed)

    @staticmethod
    def _collect(emit, cached_sessions=None) -> dict:
        data = {
            "backups": backup.list_backups(),
            "trash": trash.list_trash(),
            "tombstones": len(trash.load_tombstones()),
            "sessions": cached_sessions or [],
        }
        if cached_sessions is None:
            try:
                data["sessions"] = cb.scan()
            except Exception:      # 客户端正在写索引时可能读失败，不影响其他统计
                data["sessions"] = []
        return data

    def _apply(self, data: dict) -> None:
        self._refreshing = False
        self._stamp = time.monotonic()
        self.btn_refresh.setEnabled(True)

        backups = data.get("backups") or []
        rows = [{
            "tag": item["tag"],
            "created": str(item["created"]).replace("T", " "),
            "count": item["count"],
            "size": fmt_size(item["size"]),
            "path": item["path"],
        } for item in backups]
        self.backup_table.set_rows(rows)
        self.btn_rollback.setEnabled(bool(rows))

        trash_items = data.get("trash") or []
        self.trash_table.set_rows([{
            "tag": item["tag"],
            "created": str(item["created"]).replace("T", " "),
            "count": item["count"],
            "size": fmt_size(item.get("size")),
            "path": item["path"],
        } for item in trash_items])
        has_trash = bool(trash_items)
        self.btn_restore.setEnabled(has_trash)
        self.btn_purge.setEnabled(has_trash)
        self.trash_hint.setText(f"{len(trash_items)} 次删除记录 · 可恢复；彻底清除才释放磁盘"
                               if has_trash else "回收站是空的")

        tombstones = int(data.get("tombstones") or 0)
        self.tomb_badge.setText(f"{tombstones} 条")
        self.tomb_badge.set_tone("warn" if tombstones else "muted")
        self.btn_clear_tomb.setEnabled(bool(tombstones))
        self.tomb_hint.setText("被删除且不再参与同步的会话 ID" if tombstones
                               else "当前没有被拦截的会话")

        sessions = data.get("sessions") or []
        no_body = sum(1 for s in sessions if not s["has_body"])
        unnamed = sum(1 for s in sessions if not (s["name"] or "").strip())
        self.clean_hint.setText(
            f"共 {len(sessions)} 条 CodeBuddy 会话，其中无正文 {no_body} 条、"
            f"未命名 {unnamed} 条")

        self.win.set_summary("maintenance", f"{len(backups)} 份备份 · {len(trash_items)} 次删除记录")

    def _load_failed(self, message: str) -> None:
        self._refreshing = False
        self.btn_refresh.setEnabled(True)
        self.trash_hint.setText("统计失败，可点右上角「刷新」重试")
        self.log(f"读取备份与回收站失败：{message.splitlines()[0]}", "error")

    # ------------------------------------------------------------------ 动作
    def rollback(self) -> None:
        row = self.backup_table.current_row()
        if not row:
            self.info("选择备份", "请先在列表里选中一条备份。")
            return
        tag = row["tag"]
        if not self.confirm("确认回滚",
                            f"将把两个版本的数据库和正文恢复到备份 {tag} 的状态。\n"
                            "此操作会覆盖当前数据，确定继续吗？"):
            return
        self._set_busy(True, "正在回滚…")

        def worker(emit):
            closed = service.close_clients(lambda m, lv="info": self.log(m, lv))
            service.rollback(tag)
            for ep in closed:
                winproc.start_exe(ep.exe)
            return tag

        def done(tag_value):
            self._set_busy(False)
            self.log(f"回滚完成：{tag_value}", "ok")
            self.notify(f"已回滚到 {tag_value}")
            self.refresh()

        tasks.submit(worker, on_done=done, on_error=self._failed)

    def restore_selected(self) -> None:
        row = self.trash_table.current_row()
        if not row:
            self.info("选择记录", "请先选中一条回收站记录。")
            return
        tag = row["tag"]
        if not self.confirm("恢复会话",
                            f"将把记录 {tag} 里的会话放回原位置，并从黑名单移除。\n继续吗？"):
            return
        self._set_busy(True, "正在恢复…")

        def worker(emit):
            return service.restore_trash(tag, lambda m, lv="info": self.log(m, lv))

        def done(count):
            self._set_busy(False)
            self.log(f"已从 {tag} 恢复 {count} 条会话", "ok")
            self.notify(f"已恢复 {count} 条会话")
            self.refresh()

        tasks.submit(worker, on_done=done, on_error=self._failed)

    def purge_selected(self) -> None:
        row = self.trash_table.current_row()
        if not row:
            self.info("选择记录", "请先选中要彻底删除的记录。")
            return
        tag = row["tag"]
        if not self.confirm("彻底删除",
                            f"记录 {tag} 中的文件将被永久删除，无法恢复。继续吗？"):
            return
        if service.purge_trash(tag):
            self.log(f"已彻底删除回收站记录 {tag}", "dim")
            self.notify("已彻底删除")
            self.refresh()
        else:
            self.error("删除失败", "回收站目录可能被占用，请关闭资源管理器后重试。")

    def clear_blacklist(self) -> None:
        count = len(trash.load_tombstones())
        if not count:
            self.info("黑名单为空", "当前没有被拦截的会话。")
            return
        if not self.confirm("清空黑名单",
                            f"将移除 {count} 条拦截记录。\n"
                            "注意：如果旧数据还留在另一个版本里，下次同步会把它们重新带回来。\n"
                            "继续吗？"):
            return
        removed = service.clear_blacklist()
        self.log(f"已清空黑名单（{removed} 条）", "dim")
        self.notify("已清空黑名单")
        self.refresh()

    def cleanup_codebuddy(self, mode: str) -> None:
        label_text = {"nobody": "删除无正文会话", "unnamed": "删除未命名会话",
                      "junk": "删除全部空壳会话"}[mode]
        if not self.confirm(label_text,
                            "将永久删除命中的 CodeBuddy 会话（不进回收站），"
                            "并删除清空后残留的空工作区。\n"
                            "只保留「既有标题又有正文」的会话。\n"
                            "建议先关闭 CodeBuddy IDE / IDEA，避免索引被写回去。\n\n继续吗？"):
            return
        self._set_busy(True, f"正在{label_text}…")

        def worker(emit):
            return service.cleanup_codebuddy(mode)

        def done(stats):
            self._set_busy(False)
            text = (f"{label_text}：会话 {stats['sessions']} 条，"
                    f"空工作区 {stats['workspaces']} 个，释放 {fmt_size(stats['bytes'])}")
            if stats.get("failed"):
                text += f"，占用跳过 {stats['failed']} 个"
            self.log(text, "ok")
            self.notify(f"{label_text}完成：{stats['sessions']} 条")
            self.refresh()

        tasks.submit(worker, on_done=done, on_error=self._failed)

    def _failed(self, message: str) -> None:
        self._set_busy(False)
        self.log(message, "error")
        self.error("操作失败", message.splitlines()[0])
