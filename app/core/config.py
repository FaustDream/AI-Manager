# -*- coding: utf-8 -*-
"""配置读写：单文件 JSON，原子写入，损坏时回退默认值。"""
from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any

from . import paths


def defaults() -> dict:
    return {
        "skip": [],
        "extra_sources": {"trae": [], "qoder": [], "qoder_intl": [], "workbuddy": []},
        "window": {},
        # 账号表：拖出来的顺序与用户调过的列宽
        "account_order": [],
        "account_columns": [],
        # 会话页的视图状态：只看有正文 / 排序
        "session_opts": {
            "only_body": False,
            "sort": "last",
            "sort_desc": True,
        },
    }


def _coerce(raw: Any) -> dict:
    cfg = defaults()
    if not isinstance(raw, dict):
        return cfg
    cfg["skip"] = [str(x) for x in raw.get("skip") or [] if isinstance(x, str)]

    # 拖动排序的顺序必须原样读回来，否则每次重启都会退回默认顺序
    cfg["account_order"] = [str(x) for x in raw.get("account_order") or []
                            if isinstance(x, str)]
    columns = raw.get("account_columns")
    if isinstance(columns, list):
        widths = []
        for value in columns:
            try:
                widths.append(int(value))
            except (TypeError, ValueError):
                widths = []
                break
        cfg["account_columns"] = widths

    extra = raw.get("extra_sources")
    if isinstance(extra, dict):
        for key in cfg["extra_sources"]:
            values = extra.get(key)
            if isinstance(values, list):
                cfg["extra_sources"][key] = [str(x) for x in values if isinstance(x, str)]

    for key in ("window", "session_opts"):
        value = raw.get(key)
        if isinstance(value, dict):
            if key == "window":
                cfg[key] = {**cfg[key], **value}
            else:
                # 会话页的选项按已知键逐个取值：旧配置里的方向/重启等字段已经废弃，
                # 原样合并会让它们一直留在 config.json 里
                cfg[key] = {name: value.get(name, default)
                            for name, default in cfg[key].items()}
    return cfg


def load() -> dict:
    try:
        data = json.loads(paths.CONFIG_FILE.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return defaults()
    return _coerce(data)


def save(cfg: dict) -> None:
    """先写临时文件再替换，避免断电/崩溃留下半个 JSON。"""
    path = paths.CONFIG_FILE
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_suffix(".json.tmp")
        tmp.write_text(json.dumps(cfg, ensure_ascii=False, indent=2), encoding="utf-8")
        os.replace(tmp, path)
    except OSError:
        pass
