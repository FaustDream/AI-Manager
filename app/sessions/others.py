# -*- coding: utf-8 -*-
"""WorkBuddy / CodeBuddy 之外的软件：Qoder 两版、ZCode、Trae 占位。

这几个客户端的会话正文都在各自的 SQLite 里，账号维度各不相同：Qoder 与 ZCode
的库没有账号字段（同一客户端换号共用一份库），Trae 只有云端。所以这里只把会话
摊平成统一行交给会话页，并负责删除 / 回收站恢复的分派；跨软件格式转换不做。
"""
from __future__ import annotations

from pathlib import Path

from ..core import paths
from . import qoder, trae, zcode


def scan() -> list[dict]:
    return [*qoder.scan(), *zcode.scan(), *trae.placeholders()]


def labels() -> list[str]:
    return [*(cfg["label"] for cfg in qoder.EDITIONS.values()),
            zcode.LABEL, *(label for _folder, label in trae.APPS)]


def root_fps() -> dict[str, str]:
    """各来源的指纹：{source_key: fp}，供索引层判断要不要重扫。"""
    out = {key: qoder.root_fp(key) for key in qoder.EDITIONS}
    out["zcode"] = zcode.root_fp()
    for folder, _label in trae.APPS:
        root = paths.user_appdata_dir() / folder
        out[folder] = "present" if root.exists() else "absent"
    return out


def delete(rows: list[dict], log) -> dict:
    out = {"deleted": 0, "bytes": 0}
    for module, kind in ((qoder, "qoder"), (zcode, "zcode")):
        group = [row for row in rows if row["kind"] == kind]
        if not group:
            continue
        stats = module.delete(group, log)
        out["deleted"] += stats["deleted"]
        out["bytes"] += stats["bytes"]
    return out


def restore_item(item: dict, tag_dir: Path) -> bool:
    kind = item.get("kind")
    if kind == "qoder":
        return qoder.restore_item(item, tag_dir)
    if kind == "zcode":
        return zcode.restore_item(item, tag_dir)
    return False
