# -*- coding: utf-8 -*-
"""运行日志：分级落盘 + 结构化记录 + 性能打点。

双份落盘，同一条记录各写一份：
- app.log   人读文本：``[时间] [级别] 消息``，直接打开就能看
- app.jsonl 机读结构化：一行一条 JSON，供「日志」页查询、筛选与导出

级别（写盘门槛可配，默认全写）：
- debug：过程细节，排查时才需要（界面旧级别 dim 归到这里）
- info ：关键步骤与状态变化（界面旧级别 ok 归到这里）
- warn ：能继续但不理想
- error：失败、异常

记录分三种 kind：
- log  ：普通日志行
- event：状态变化（如「任务开始」「登录态已过期」）
- perf ：性能打点（任务耗时、排队耗时、结束时进程内存），为优化提供数据

超过 5MB 轮转，保留最近 3 份（app.log.1…3 / app.jsonl.1…3）。
写盘沿用「内存缓冲 + 定时 flush」：会话同步一次刷几百行，
逐行 open/close 会拖住 GUI 线程；进程退出前由主窗口 flush() 兜底。
"""
from __future__ import annotations

import csv
import ctypes
import ctypes.wintypes
import json
import re
import threading
import time
from datetime import datetime
from pathlib import Path

from . import paths

# ------------------------------------------------------------------ 级别
LEVELS = {"debug": 10, "info": 20, "warn": 30, "error": 40}
LEVEL_TEXT = {"debug": "调试", "info": "信息", "warn": "警告", "error": "错误"}
# 界面历史级别名 → 标准级别：dim 是过程细节，ok 是成功的信息
_ALIAS = {"dim": "debug", "ok": "info", "warning": "warn", "err": "error", "": "info"}

_MIN_LEVEL = 10      # 写盘门槛，set_level 调整；默认全写（debug 起）
_MAX_PENDING = 200   # 缓冲条数，超过就先落一次盘
_MAX_BYTES = 5 * 1024 * 1024     # 单文件轮转阈值
_KEEP_ROTATED = 3                # 轮转后保留份数

_BUFFER: list[tuple[str, dict]] = []    # (文本行, 结构化记录)
_LOCK = threading.Lock()


# ------------------------------------------------------------------ Windows 内存
class _MemoryCounters(ctypes.Structure):
    _fields_ = [("cb", ctypes.c_uint32), ("PageFaultCount", ctypes.c_uint32),
                ("PeakWorkingSetSize", ctypes.c_size_t), ("WorkingSetSize", ctypes.c_size_t),
                ("QuotaPeakPagedPoolUsage", ctypes.c_size_t), ("QuotaPagedPoolUsage", ctypes.c_size_t),
                ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
                ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
                ("PagefileUsage", ctypes.c_size_t), ("PeakPagefileUsage", ctypes.c_size_t)]


def memory_mb() -> float:
    """当前进程占用的物理内存（MB）；取不到时返回 0，不值得为它抛错。"""
    try:
        counters = _MemoryCounters()
        counters.cb = ctypes.sizeof(_MemoryCounters)
        kernel32 = ctypes.windll.kernel32
        # 不声明签名的话 HANDLE 会被默认转成 c_int，64 位下高位丢失、调用失败
        kernel32.GetCurrentProcess.restype = ctypes.wintypes.HANDLE
        psapi = ctypes.windll.psapi
        psapi.GetProcessMemoryInfo.argtypes = [ctypes.wintypes.HANDLE,
                                               ctypes.POINTER(_MemoryCounters),
                                               ctypes.c_uint32]
        psapi.GetProcessMemoryInfo.restype = ctypes.wintypes.BOOL
        handle = kernel32.GetCurrentProcess()
        if not psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
            return 0.0
        return round(counters.WorkingSetSize / 1024 / 1024, 1)
    except Exception:
        return 0.0


# ------------------------------------------------------------------ 基础
def set_level(name: str) -> None:
    """设置写盘门槛：低于该级别的不落盘。默认 debug（全部记录）。"""
    global _MIN_LEVEL
    _MIN_LEVEL = LEVELS.get(_norm_level(name), 10)


def level_name() -> str:
    for key, value in LEVELS.items():
        if value == _MIN_LEVEL:
            return key
    return "debug"


def _norm_level(level: str) -> str:
    key = (level or "").strip().lower()
    key = _ALIAS.get(key, key)
    return key if key in LEVELS else "info"


def _stamp() -> str:
    return datetime.now().strftime("%Y-%m-%d %H:%M:%S.%f")[:-3]


def _text_line(ts: str, level: str, message: str) -> str:
    head = ts[:19]      # 文本行不带毫秒，和历史格式保持一致
    return f"[{head}] [{level.upper()}] {message}"


# ------------------------------------------------------------------ 写入
def _write(record: dict, text: str) -> None:
    with _LOCK:
        _BUFFER.append((text, record))
        if len(_BUFFER) >= _MAX_PENDING:
            _flush_locked()


def log(level: str, message: str, *, module: str = "", kind: str = "log", **extra) -> None:
    """主入口：按级别记一条日志。module 填调用方页面/模块名，extra 进结构化字段。

    门槛只作用于普通日志（kind=log）；事件与性能打点始终落盘——
    排查问题时这两类是骨架，被门槛滤掉就没有分析依据了。
    """
    lv = _norm_level(level)
    if kind == "log" and LEVELS[lv] < _MIN_LEVEL:
        return
    ts = _stamp()
    record = {"ts": ts, "lv": lv, "kind": kind, "msg": str(message)}
    if module:
        record["mod"] = module
    for key, value in extra.items():
        if value is not None:
            record[key] = value
    _write(record, _text_line(ts, lv, str(message)))


def debug(message: str, *, module: str = "", **extra) -> None:
    log("debug", message, module=module, **extra)


def info(message: str, *, module: str = "", **extra) -> None:
    log("info", message, module=module, **extra)


def warn(message: str, *, module: str = "", **extra) -> None:
    log("warn", message, module=module, **extra)


def error(message: str, *, module: str = "", **extra) -> None:
    log("error", message, module=module, **extra)


def event(name: str, **fields) -> None:
    """状态变化事件：kind=event，name 是事件名，其余字段进记录。"""
    level = fields.pop("level", "info")
    module = fields.pop("module", "")
    log(level, name, module=module, kind="event", **fields)


def perf(name: str, duration_ms: float, **metrics) -> None:
    """性能打点：kind=perf。duration_ms 为执行耗时，metrics 常用 queue_ms/state/task。"""
    module = metrics.pop("module", "任务")
    record = {"ts": _stamp(), "lv": "info", "kind": "perf",
              "msg": name, "mod": module,
              "dur": round(float(duration_ms), 1), "mem": memory_mb()}
    for key, value in metrics.items():
        if value is not None:
            record[key] = value
    text = (f"[{record['ts'][:19]}] [PERF] {name} 耗时 {_ms_text(duration_ms)}"
            f" · 内存 {record['mem']}MB")
    _write(record, text)


def append(text: str, level: str = "info", **extra) -> str:
    """兼容入口：老调用点 self.log(msg, level) 都走这里，返回带时间戳的整行。"""
    log(level, text, **extra)
    lv = _norm_level(level)
    return _text_line(datetime.now().strftime("%Y-%m-%d %H:%M:%S.%f")[:-3], lv, text)


def _ms_text(ms: float) -> str:
    if ms < 1000:
        return f"{ms:.0f} 毫秒"
    if ms < 60_000:
        return f"{ms / 1000:.1f} 秒"
    return f"{int(ms // 60_000)} 分 {int((ms % 60_000) / 1000)} 秒"


# ------------------------------------------------------------------ 落盘与轮转
def _rotated(path: Path, index: int) -> Path:
    return path.with_name(f"{path.name}.{index}")


def _rotate_locked(path: Path) -> None:
    try:
        if not path.exists() or path.stat().st_size < _MAX_BYTES:
            return
        oldest = _rotated(path, _KEEP_ROTATED)
        oldest.unlink(missing_ok=True)
        for index in range(_KEEP_ROTATED - 1, 0, -1):
            src = _rotated(path, index)
            if src.exists():
                src.replace(_rotated(path, index + 1))
        path.replace(_rotated(path, 1))
    except OSError:
        pass


def _flush_locked() -> None:
    if not _BUFFER:
        return
    texts, records = [], []
    for text, record in _BUFFER:
        texts.append(text)
        records.append(json.dumps(record, ensure_ascii=False))
    _BUFFER.clear()
    try:
        paths.LOG_DIR.mkdir(parents=True, exist_ok=True)
        jsonl = paths.LOG_DIR / "app.jsonl"
        # 先轮转再写：写完再转会让主文件消失到下一次写入为止，
        # 这段时间「打开日志目录」的人会以为日志丢了
        _rotate_locked(paths.LOG_FILE)
        _rotate_locked(jsonl)
        with open(paths.LOG_FILE, "a", encoding="utf-8") as fh:
            fh.write("\n".join(texts) + "\n")
        with open(jsonl, "a", encoding="utf-8") as fh:
            fh.write("\n".join(records) + "\n")
    except OSError:
        pass


def flush() -> None:
    """把缓冲写入磁盘；界面定时调用，退出时也会调用。"""
    with _LOCK:
        _flush_locked()


def clear_rotated() -> int:
    """清掉轮转出来的历史文件（不碰正在写的两个主文件），返回释放的字节数。"""
    freed = 0
    for base in (paths.LOG_FILE, paths.LOG_DIR / "app.jsonl"):
        for index in range(1, _KEEP_ROTATED + 1):
            path = _rotated(base, index)
            try:
                freed += path.stat().st_size
                path.unlink()
            except OSError:
                continue
    return freed


# ------------------------------------------------------------------ 查询与导出
_TEXT_RE = re.compile(r"^\[(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2})\] "
                      r"(?:\[(DEBUG|INFO|WARN|ERROR|PERF)\] )?(.*)$")


def _jsonl_paths() -> list[Path]:
    root = paths.LOG_DIR
    names = [f"app.jsonl.{index}" for index in range(_KEEP_ROTATED, 0, -1)]
    names.append("app.jsonl")
    return [root / name for name in names if (root / name).exists()]


def _parse_text_fallback() -> list[dict]:
    """只有老格式 app.log 时（还没有 jsonl），解析文本行充当记录，历史日志也能查。"""
    out: list[dict] = []
    try:
        lines = paths.LOG_FILE.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return out
    for line in lines:
        match = _TEXT_RE.match(line)
        if not match:
            continue
        ts, tag, message = match.groups()
        out.append({"ts": ts, "lv": "perf" if tag == "PERF" else "info",
                    "kind": "perf" if tag == "PERF" else "log", "msg": message})
    return out


def query(level: str = "", kind: str = "", keyword: str = "", limit: int = 800) -> list[dict]:
    """查日志：按级别 / 类型 / 关键词过滤，返回最新的 limit 条（新的在前）。

    读的是 app.jsonl 及其轮转文件；若完全没有结构化记录，
    回退解析老 app.log 文本，早期历史也能查到。
    """
    flush()      # 先把手上的缓冲落盘，用户点「查询」要看到最新
    records: list[dict] = []
    for path in _jsonl_paths():
        try:
            for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
                line = line.strip()
                if not line:
                    continue
                try:
                    records.append(json.loads(line))
                except ValueError:
                    continue
        except OSError:
            continue
    if not records:
        records = _parse_text_fallback()

    want_level = _norm_level(level) if level else ""
    want_kind = (kind or "").strip()
    needle = (keyword or "").strip().lower()
    filtered = []
    for record in records:
        if want_level and record.get("lv") != want_level:
            continue
        if want_kind and record.get("kind", "log") != want_kind:
            continue
        if needle:
            haystack = json.dumps(record, ensure_ascii=False).lower()
            if needle not in haystack:
                continue
        filtered.append(record)
    filtered.sort(key=lambda item: str(item.get("ts") or ""), reverse=True)
    return filtered[:max(1, min(int(limit), 5000))]


def stats(records: list[dict]) -> dict:
    """按级别数一遍，给汇总行用。"""
    counts = {key: 0 for key in LEVELS}
    perf_count = event_count = 0
    for record in records:
        lv = _norm_level(str(record.get("lv") or "info"))
        counts[lv] = counts.get(lv, 0) + 1
        kind = record.get("kind", "log")
        if kind == "perf":
            perf_count += 1
        elif kind == "event":
            event_count += 1
    return {"debug": counts["debug"], "info": counts["info"], "warn": counts["warn"],
            "error": counts["error"], "perf": perf_count, "event": event_count,
            "total": len(records)}


def export(records: list[dict], dest: Path, fmt: str = "txt") -> int:
    """导出查询结果：txt（可读）/ jsonl（机读）/ csv（表格）。返回写出的条数。"""
    dest = Path(dest)
    dest.parent.mkdir(parents=True, exist_ok=True)
    fmt = (fmt or "txt").lower()
    if fmt == "jsonl":
        with open(dest, "w", encoding="utf-8") as fh:
            for record in records:
                fh.write(json.dumps(record, ensure_ascii=False) + "\n")
        return len(records)
    if fmt == "csv":
        columns = ["ts", "lv", "kind", "mod", "msg", "dur", "mem"]
        with open(dest, "w", encoding="utf-8-sig", newline="") as fh:
            writer = csv.writer(fh)
            writer.writerow(["时间", "级别", "类型", "模块", "内容",
                             "耗时ms", "内存MB"])
            for record in records:
                row = [record.get(column, "") for column in columns]
                extras = {k: v for k, v in record.items() if k not in columns}
                if extras:
                    row.append(json.dumps(extras, ensure_ascii=False))
                else:
                    row.append("")
                writer.writerow(row)
        return len(records)
    with open(dest, "w", encoding="utf-8") as fh:
        for record in records:
            lv = _norm_level(str(record.get("lv") or "info")).upper()
            kind = record.get("kind", "log")
            tag = "PERF" if kind == "perf" else "EVENT" if kind == "event" else lv
            module = record.get("mod") or ""
            head = f"[{record.get('ts', '')}] [{tag}]"
            if module:
                head += f" [{module}]"
            parts = [f"{head} {record.get('msg', '')}"]
            if record.get("dur") is not None:
                parts.append(f"耗时 {_ms_text(float(record['dur']))}")
            if record.get("mem") is not None:
                parts.append(f"内存 {record['mem']}MB")
            extras = {k: v for k, v in record.items()
                      if k not in ("ts", "lv", "kind", "mod", "msg", "dur", "mem")}
            if extras:
                parts.append(json.dumps(extras, ensure_ascii=False))
            fh.write(" · ".join(parts) + "\n")
    return len(records)
