# -*- coding: utf-8 -*-
"""WorkBuddy 国内版 / 国际版的数据目录与通用文件操作。

两个版本各自用独立目录（~/.workbuddy 与 ~/.workbuddy-ai），会话列表的查询条件是
`transport='local' AND deleted_at IS NULL AND user_id=<当前账号>`。
"""
from __future__ import annotations

import json
import os
import re
import shutil
import sqlite3
import stat
from pathlib import Path

HOME = Path.home()

EDITIONS = {
    "cn": {
        "dir": ".workbuddy",
        "label": "WorkBuddy 国内版",
        "short": "国内版",
        "exe": r"C:\App\WB\CN\WorkBuddy.exe",
        "fallbacks": (
            r"C:\Program Files\WorkBuddy\WorkBuddy.exe",
            "%LOCALAPPDATA%/Programs/WorkBuddy/WorkBuddy.exe",
            "%LOCALAPPDATA%/Programs/CodeBuddy CN/CodeBuddy CN.exe",
        ),
        "proc": "WorkBuddy.exe",
    },
    "intl": {
        "dir": ".workbuddy-ai",
        "label": "WorkBuddy 国际版",
        "short": "国际版",
        "exe": r"C:\App\WB\AI\WorkBuddyAI.exe",
        "fallbacks": (
            "%LOCALAPPDATA%/Programs/WorkBuddyAI/WorkBuddyAI.exe",
            r"C:\Program Files\WorkBuddyAI\WorkBuddyAI.exe",
        ),
        "proc": "WorkBuddyAI.exe",
    },
}

LOCAL_TABLES = ("sessions", "session_usage")


def _expand(text: str) -> Path:
    return Path(os.path.expandvars(text))


class Edition:
    def __init__(self, key: str):
        cfg = EDITIONS[key]
        self.key = key
        self.label = cfg["label"]
        self.short = cfg["short"]
        self.proc = cfg["proc"]
        self.root = HOME / cfg["dir"]
        self.db = self.root / "workbuddy.db"
        self.projects = self.root / "projects"
        self.tasks = self.root / "tasks"
        self.todos = self.root / "todos"
        self.snapshot = self.root / "storage" / "skeleton" / "account-snapshot.json"
        self._exe = _expand(cfg["exe"])
        self._fallbacks = [_expand(p) for p in cfg.get("fallbacks", ())]

    @property
    def exe(self) -> Path:
        """优先官方安装路径，找不到再试常见位置。"""
        if self._exe.exists():
            return self._exe
        for candidate in self._fallbacks:
            if candidate.exists():
                return candidate
        return self._exe

    @property
    def exists(self) -> bool:
        return self.db.exists()

    def current_uid(self) -> tuple[str, str]:
        """返回 (uid, 来源说明)。优先读客户端真实登录态，读不到再从库里反推。"""
        try:
            if self.snapshot.exists():
                data = json.loads(self.snapshot.read_text(encoding="utf-8"))
                uid = (data.get("primary") or {}).get("uid", "")
                if uid:
                    return uid, "account-snapshot.json"
        except (OSError, ValueError):
            pass
        conn = connect_ro(self.db)
        if conn is None:
            return "", "无"
        try:
            row = conn.execute(
                "SELECT user_id, COUNT(*) AS n FROM sessions "
                "WHERE transport='local' GROUP BY user_id ORDER BY n DESC LIMIT 1"
            ).fetchone()
        finally:
            conn.close()
        if row and row[0]:
            return row[0], "从数据库反推（可能落到旧账号，建议先登录一次）"
        return "", "无"


def connect_ro(path: Path):
    if not path or not path.exists():
        return None
    try:
        return sqlite3.connect(f"file:{path.as_posix()}?mode=ro", uri=True)
    except sqlite3.Error:
        return None


def connect_rw(path: Path):
    if not path.exists():
        raise RuntimeError(f"数据库不存在：{path}")
    return sqlite3.connect(str(path))


def table_columns(conn, table: str) -> list[str]:
    if table not in LOCAL_TABLES:
        raise ValueError(table)
    return [r[1] for r in conn.execute(f"PRAGMA table_info({table})").fetchall()]


def all_columns(conn, table: str) -> list[str]:
    return [r[1] for r in conn.execute(f"PRAGMA table_info({table})").fetchall()]


# ------------------------------------------------------------------ 文件操作
def path_size(path: Path) -> int:
    try:
        if path.is_dir():
            return sum(f.stat().st_size for f in path.rglob("*") if f.is_file())
        return path.stat().st_size
    except OSError:
        return 0


def ensure_writable(path: Path) -> None:
    try:
        if path.is_dir():
            for inner in path.rglob("*"):
                try:
                    inner.chmod(inner.stat().st_mode | stat.S_IWRITE)
                except OSError:
                    pass
        path.chmod(path.stat().st_mode | stat.S_IWRITE)
    except OSError:
        pass


def remove_path(path: Path) -> bool:
    if path.is_dir():
        def _onexc(func, target, _exc):
            try:
                os.chmod(target, stat.S_IWRITE)
                func(target)
            except OSError:
                pass

        try:
            shutil.rmtree(path, onexc=_onexc)
            return True
        except OSError:
            return False
    try:
        path.unlink()
        return True
    except OSError:
        try:
            os.chmod(path, stat.S_IWRITE)
            path.unlink()
            return True
        except OSError:
            return False


def copy_any(src: Path, dst: Path) -> None:
    """复制文件或目录，目标已存在则整体覆盖。"""
    if src.is_dir():
        if dst.exists():
            remove_path(dst)
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(src, dst)
    else:
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dst)
    ensure_writable(dst)


def cwd_to_slug(cwd: str) -> str:
    """工作目录 → projects 子目录名（与客户端规则一致，仅作兜底）。"""
    if not cwd:
        return ""
    text = re.sub(r"^([A-Za-z]):", lambda m: m.group(1).lower(), str(cwd))
    text = text.replace("\\", "-").replace("/", "-")
    return re.sub(r"-{2,}", "-", text).strip("-")
