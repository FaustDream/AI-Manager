# -*- coding: utf-8 -*-
"""Trae 的会话占位：正文在云端，本地只留输入历史。

Trae CN / Trae 的对话存在服务端，本地 workspaceStorage 里只有终端缓冲和
icube-ai-agent-storage-input-history，读不到任何消息正文。为了不让「软件筛选里
有 Trae、列表却是空的」看起来像读取失败，这里给已安装的版本各放一行占位，
状态标「云端」——删除动作会像对待云端会话一样跳过它。
"""
from __future__ import annotations

from ..core import paths

APPS = (("Trae CN", "Trae CN"), ("Trae", "Trae 国际版"))


def placeholders() -> list[dict]:
    rows = []
    for folder, label in APPS:
        if not (paths.user_appdata_dir() / folder).exists():
            continue
        rows.append({
            "kind": "trae",
            "source_key": folder,
            "source_label": label,
            "account": "",
            "id": f"cloud:{folder}",
            "title": "对话记录存在云端，本地没有可管理的正文",
            "last": 0,
            "msgs": 0,
            "size": 0,
            "state": "云端",
            "has_body": False,
            "files": [],
            "path": "",
            "cwd": "",
            "client_status": "",
            "raw": None,
        })
    return rows
