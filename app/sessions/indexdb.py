# -*- coding: utf-8 -*-
"""轻量会话索引：只存清单与指纹，不存正文。

列表页要的是标题、时间、条数、体积——这些属于索引信息，存本地小库（data/catalog.db），
刷新时先照着它上屏；正文留在各客户端原存储里，选中会话才去读。指纹是增量的核心：
源指纹（root_fp）没变的来源整体跳过，消息计数按「文件 mtime:size」缓存，变了才重读内容。
"""
from __future__ import annotations

import json
import sqlite3
from datetime import datetime

from ..core import paths

DB = paths.DATA_DIR / "catalog.db"
SCHEMA_VERSION = "1"
_TABLES = ("sources", "rows", "cache")


def _connect() -> sqlite3.Connection:
    DB.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(str(DB))
    conn.execute("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT)")
    conn.execute("CREATE TABLE IF NOT EXISTS sources ("
                 "source_key TEXT PRIMARY KEY, label TEXT, root_fp TEXT, "
                 "scanned_at INTEGER, row_count INTEGER)")
    conn.execute("CREATE TABLE IF NOT EXISTS rows ("
                 "row_key TEXT PRIMARY KEY, source_key TEXT, payload TEXT)")
    conn.execute("CREATE TABLE IF NOT EXISTS cache (key TEXT PRIMARY KEY, fp TEXT, value TEXT)")
    row = conn.execute("SELECT value FROM meta WHERE key='schema_version'").fetchone()
    if row is None:
        conn.execute("INSERT INTO meta VALUES ('schema_version', ?)", (SCHEMA_VERSION,))
    elif row[0] != SCHEMA_VERSION:
        # 结构变了就地清空重建：索引是缓存性质，丢了只影响一次刷新速度
        for table in _TABLES:
            conn.execute(f"DELETE FROM {table}")
        conn.execute("UPDATE meta SET value=? WHERE key='schema_version'", (SCHEMA_VERSION,))
    conn.commit()
    return conn


def row_key(row: dict) -> str:
    return f'{row["kind"]}|{row["source_key"]}|{row.get("account") or ""}|{row["id"]}'


class Index:
    """一次扫描周期的索引视图：读进内存改，结束时 flush 落盘。"""

    def __init__(self) -> None:
        self.fps: dict[str, str] = {}
        self.labels: dict[str, str] = {}
        self.rows: dict[str, list[dict]] = {}
        self.counts: dict[str, tuple[str, str]] = {}
        self._store: dict[str, tuple[str, str, list[dict]]] = {}
        self._drops: set[str] = set()
        self._counts_new: dict[str, tuple[str, str]] = {}

    # ---------------------------------------------------------------- 读
    def hit(self, source_key: str, root_fp: str) -> bool:
        return self.fps.get(source_key) == root_fp

    def source_rows(self, source_key: str) -> list[dict]:
        return [dict(row) for row in self.rows.get(source_key, [])]

    def count(self, key: str, fp: str) -> str | None:
        item = self.counts.get(key)
        return item[1] if item and item[0] == fp else None

    # ---------------------------------------------------------------- 写
    def put_count(self, key: str, fp: str, value: str) -> None:
        self._counts_new[key] = (fp, value)

    def store(self, source_key: str, label: str, root_fp: str, rows: list[dict]) -> None:
        self._store[source_key] = (label, root_fp, rows)
        self._drops.discard(source_key)

    def drop_source(self, source_key: str) -> None:
        """写入动作之后让该来源失效，下次刷新重建。"""
        self._drops.add(source_key)
        self._store.pop(source_key, None)

    def flush(self) -> None:
        conn = _connect()
        try:
            now = int(datetime.now().timestamp())
            for key in self._drops:
                conn.execute("DELETE FROM rows WHERE source_key=?", (key,))
                conn.execute("DELETE FROM sources WHERE source_key=?", (key,))
            for key, (label, root_fp, rows) in self._store.items():
                conn.execute("DELETE FROM rows WHERE source_key=?", (key,))
                conn.execute(
                    "INSERT OR REPLACE INTO sources VALUES (?,?,?,?,?)",
                    (key, label, root_fp, now, len(rows)))
                conn.executemany(
                    "INSERT OR REPLACE INTO rows VALUES (?,?,?)",
                    [(row_key(row), key, json.dumps(row, ensure_ascii=False))
                     for row in rows])
            if self._counts_new:
                conn.executemany(
                    "INSERT OR REPLACE INTO cache VALUES (?,?,?)",
                    [(key, fp, value) for key, (fp, value) in self._counts_new.items()])
            conn.commit()
        finally:
            conn.close()


def load() -> Index:
    index = Index()
    conn = _connect()
    try:
        for key, fp, label in conn.execute("SELECT source_key, root_fp, label FROM sources"):
            index.fps[key] = fp or ""
            index.labels[key] = label or ""
        for key, payload in conn.execute("SELECT source_key, payload FROM rows"):
            try:
                row = json.loads(payload)
            except ValueError:
                continue
            # 损坏 / 旧结构的行直接丢：索引是缓存，少一行只是刷新慢一次
            if not (isinstance(row, dict) and row.get("kind")
                    and row.get("id") and row.get("source_label")):
                continue
            index.rows.setdefault(key, []).append(row)
        for key, fp, value in conn.execute("SELECT key, fp, value FROM cache"):
            index.counts[key] = (fp or "", value or "0")
    finally:
        conn.close()
    return index


def drop_sources(keys) -> None:
    """写入后直接落盘失效，不等下一次扫描周期。"""
    index = Index()
    for key in keys:
        index.drop_source(key)
    index.flush()


def clear() -> None:
    """重建索引：清空缓存内容，下次刷新全量重扫。"""
    conn = _connect()
    try:
        for table in _TABLES:
            conn.execute(f"DELETE FROM {table}")
        conn.commit()
    finally:
        conn.close()
