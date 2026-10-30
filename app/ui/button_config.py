# -*- coding: utf-8 -*-
"""账号中心按钮注册表：位置、权限、启用规则与文案。

BUTTONS 是代码级元数据：放在哪一行、点击前要满足什么、什么时候可点、写什么字。
按钮的显示与顺序由注册表固定，不提供运行期配置——配置对话框已移除，
避免按钮被隐藏后没人知道该去哪找回来。
"""
from __future__ import annotations

from PySide6.QtWidgets import QPushButton

# ---------------------------------------------------------------------------
# 前置权限（requires）：动作需要满足的条件，点击不满足时页面给提示
#   selection        需要选中表格行
#   stored_account   仅浏览器登录账号（可移除）
#   checkin_account  仅可签到账号
#   rows             需要存在数据行
# 启用规则（enable_when）：决定按钮什么时候可点（灰/亮）
#   always           始终可点
#   selection        选中行后才可点
#   has_rows         有数据行才可点
#   has_pending      有账号今天还能签才可点（全签过就置灰）
# ---------------------------------------------------------------------------
BUTTONS: dict[str, dict] = {
    # ---------------------------------------------------------- 账号列表面板
    "accounts.add": {
        "panel": "accounts", "position": "header", "order": 10,
        "text": "添加账号", "kind": "primary", "icon": "key",
        "requires": [], "enable_when": "always",
        "note": "浏览器授权或手动录入新账号",
    },
    "accounts.open_client": {
        "panel": "accounts", "position": "toolbar", "order": 20,
        "text": "打开客户端", "kind": "ghost", "icon": "external",
        "requires": ["selected"], "enable_when": "selection",
        "note": "启动选中账号对应的官方客户端",
    },
    "accounts.switch_login": {
        "panel": "accounts", "position": "toolbar", "order": 30,
        "text": "切换登录", "kind": "ghost", "icon": "sync",
        "requires": ["selected"], "enable_when": "selection",
        "note": "把 token 写回官方客户端登录文件（自动关闭并重启客户端）",
    },
    "accounts.remove": {
        "panel": "accounts", "position": "toolbar", "order": 40,
        "text": "移除", "kind": "danger-ghost", "icon": None,
        "requires": ["selected", "stored_account"], "enable_when": "selection",
        "note": "从本工具移除浏览器登录账号，不影响官方客户端",
    },
    # -------------------------------------------------------------- 签到面板
    "checkin.checkin_all": {
        "panel": "checkin", "position": "toolbar", "order": 10,
        "text": "全部签到", "kind": "primary", "icon": "bolt",
        "requires": ["rows"], "enable_when": "has_pending",
        "note": "为所有参与签到的账号执行签到（跳过免签到与今日已签）",
    },
    "checkin.refresh_all": {
        "panel": "checkin", "position": "toolbar", "order": 30,
        "text": "刷新", "kind": "ghost", "icon": "refresh",
        "requires": [], "enable_when": "always",
        "note": "重新扫描本地登录态，并刷新签到状态、积分与 ZCode 额度",
    },
    "checkin.toggle_skip": {
        "panel": "checkin", "position": "toolbar", "order": 70,
        "text": "免签到", "kind": "ghost", "icon": None,
        "requires": ["selected", "checkin_account"], "enable_when": "selection",
        "note": "切换选中账号的免签到标记",
    },
    # -------------------------------------------------------------- 账号动作
    # 添加账号 / 打开客户端只在账号动作行出现一次，签到行不再重复注册
    "profiles.logout": {
        "panel": "profiles", "position": "header", "order": 10,
        "text": "退出登录", "kind": "ghost", "icon": None,
        "requires": [], "enable_when": "always",
        "note": "保存当前账号后清空登录态，客户端回到登录页",
    },
}

PANEL_LABEL = {"accounts": "账号列表", "checkin": "签到", "profiles": "账号动作"}
POSITION_LABEL = {"header": "页头", "toolbar": "工具行", "footer": "底栏"}
ENABLE_LABEL = {"always": "始终可用", "selection": "选中后可用",
                "has_rows": "有数据可用", "has_pending": "有待签账号"}
REQUIRES_LABEL = {
    "selected": "需选中行", "stored_account": "仅浏览器账号",
    "checkin_account": "仅可签到账号", "rows": "需有数据行",
}


def sorted_ids(panel: str, position: str) -> list[str]:
    """某面板某位置的全部按钮 ID，按注册表里的 order 排好。"""
    ids = [bid for bid, meta in BUTTONS.items()
           if meta["panel"] == panel and meta["position"] == position]
    return sorted(ids, key=lambda bid: (BUTTONS[bid]["order"], bid))


# ------------------------------------------------------------------ 规则求值
def rule_ok(enable_when: str, state: dict) -> bool:
    if enable_when == "selection":
        return bool(state.get("selected"))
    if enable_when == "has_rows":
        return bool(state.get("rows"))
    if enable_when == "has_pending":
        return bool(state.get("checkin_pending"))
    return True


class ButtonBar:
    """一组按钮的构建与状态同步：统一负责排序、启用规则。

    用法：
        bar = ButtonBar()
        buttons = bar.build(layout, "checkin", "toolbar", on_click)
        bar.sync(state)        # state: {"busy": bool, "rows": bool, "selected": bool}
    """

    def __init__(self, cfg: dict | None = None):
        # cfg 已不参与按钮构建，保留参数是为了不惊动调用方
        self.cfg = cfg or {}
        self._widgets: dict[str, QPushButton] = {}

    def build(self, layout, panel: str, position: str,
              on_click: dict, ctx=None,
              compact: bool = False) -> dict[str, QPushButton]:
        """构建按钮并接线；ctx 返回 {selected/rows/checkin_account/stored_account}。

        compact 用于按钮多、需要挤在一行的场景：去掉图标并收紧内边距。
        """
        buttons: dict[str, QPushButton] = {}
        from .widgets import make_button
        for bid in sorted_ids(panel, position):
            meta = BUTTONS[bid]
            handler = on_click.get(bid)
            button = make_button(meta["text"], meta["kind"],
                                 None if compact else meta["icon"])
            if compact:
                button.setProperty("compact", True)
            if handler:
                button.clicked.connect(
                    self._guard(bid, handler, ctx))
            layout.addWidget(button)
            buttons[bid] = button
            self._widgets[bid] = button
        return buttons

    def _guard(self, bid: str, handler, ctx):
        """点击前置权限检查：requires 不满足时不执行动作，给统一提示。"""
        def run():
            meta = BUTTONS[bid]
            state = ctx() if callable(ctx) else (ctx or {})
            missing = [r for r in (meta.get("requires") or [])
                       if not state.get(r)]
            if missing:
                tips = "、".join(REQUIRES_LABEL.get(r, r) for r in missing)
                self._denied(f"「{meta['text']}」需要：{tips}")
                return
            handler()
        return run

    def _denied(self, message: str) -> None:
        # 延迟导入避免环；页面级提示走全局 toast
        try:
            from ..core.bus import toast
            toast(message, "warn")
        except Exception:
            pass

    def register(self, button_id: str, widget: QPushButton) -> None:
        self._widgets[button_id] = widget

    def widget(self, button_id: str):
        return self._widgets.get(button_id)

    def sync(self, state: dict) -> None:
        """按 busy / 规则统一刷新启用状态。"""
        busy = bool(state.get("busy"))
        for bid, widget in self._widgets.items():
            meta = BUTTONS[bid]
            widget.setEnabled(not busy and rule_ok(meta["enable_when"], state))
