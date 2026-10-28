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
        "auto": False,
        "auto_time": "08:30",
        "last_auto": "",
        "skip": [],
        "extra_sources": {"trae": [], "qoder": [], "qoder_intl": [], "workbuddy": []},
        "window": {},
        "last_page": "overview",
        # 会话页的持久化选项：方向 / 含已删除 / 恢复可见 / 完成后重启
        "session_opts": {
            "direction": "both",
            "include_deleted": True,
            "restore": True,
            "restart": True,
            "only_body": False,
        },
    }


def _coerce(raw: Any) -> dict:
    cfg = defaults()
    if not isinstance(raw, dict):
        return cfg
    cfg["auto"] = bool(raw.get("auto", False))
    cfg["auto_time"] = str(raw.get("auto_time") or "08:30")
    cfg["last_auto"] = str(raw.get("last_auto") or "")
    cfg["skip"] = [str(x) for x in raw.get("skip") or [] if isinstance(x, str)]
    cfg["last_page"] = str(raw.get("last_page") or "overview")

    extra = raw.get("extra_sources")
    if isinstance(extra, dict):
        for key in cfg["extra_sources"]:
            values = extra.get(key)
            if isinstance(values, list):
                cfg["extra_sources"][key] = [str(x) for x in values if isinstance(x, str)]

    for key in ("window", "session_opts"):
        value = raw.get(key)
        if isinstance(value, dict):
            cfg[key] = {**cfg[key], **value}
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
