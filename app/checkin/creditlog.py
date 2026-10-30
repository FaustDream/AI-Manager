# -*- coding: utf-8 -*-
"""积分余额按天快照：平台只给当前余额、不给流水，增减只能自己记。

每个账号每天只保留最后一次看到的余额，历史留 60 天。有了这份记录，
「今天比上次多了/少了多少」和近两周走势都能算出来，不需要任何第三方图表库。
"""
from __future__ import annotations

import json
import os
import threading
from datetime import date, timedelta

from ..core import paths

KEEP_DAYS = 60
TREND_DAYS = 14

# 各账号的查询在后台线程并发执行，读写同一份文件必须串行，否则会互相覆盖
_LOCK = threading.Lock()


def _num(value) -> float | None:
    try:
        number = float(value)
    except (TypeError, ValueError):
        return None
    # 接口返回值带浮点噪声（719.0999999999），存盘前收一下，文件才看得懂
    return round(number, 3) if number >= 0 else None


def _load() -> dict:
    try:
        data = json.loads(paths.CREDIT_HISTORY_FILE.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return data if isinstance(data, dict) else {}


def _save(data: dict) -> None:
    """先写临时文件再替换：崩在半路也不能留下半个 JSON。"""
    path = paths.CREDIT_HISTORY_FILE
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_suffix(".json.tmp")
        tmp.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        os.replace(tmp, path)
    except OSError:
        pass


def _prune(data: dict, today: date) -> dict:
    limit = (today - timedelta(days=KEEP_DAYS)).isoformat()
    out = {}
    for iid, series in data.items():
        if not isinstance(series, dict):
            continue
        kept = {}
        for day, value in series.items():
            if not isinstance(day, str) or day < limit:
                continue
            number = _num(value)
            if number is not None:
                kept[day] = number
        if kept:
            out[iid] = kept
    return out


def note(iid: str, balance, today: date | None = None) -> bool:
    """记下某账号今天的余额；同一天多次写入只保留最后一次。"""
    value = _num(balance)
    if not iid or value is None:
        return False
    day = (today or date.today()).isoformat()
    with _LOCK:
        data = _load()
        series = data.get(iid)
        if not isinstance(series, dict):
            series = {}
        if series.get(day) == value:
            return False
        series[day] = value
        data[iid] = series
        _save(_prune(data, date.today()))
    return True


def _latest(series: dict, day: str) -> float | None:
    """某天（含）之前最后一次记录的值：没记录的日子用前值顺延，趋势才连续。"""
    candidates = [stamp for stamp in series if stamp <= day]
    return series[max(candidates)] if candidates else None


def summary(trend_days: int = TREND_DAYS, today: date | None = None) -> dict:
    """余额合计、相对上次记录的增量、近 N 天走势，一次读盘算完。

    只算「增加」：余额下降既可能是消耗也可能是到期扣减，平台不给流水就分不开。
    """
    now = today or date.today()
    stamp = now.isoformat()
    data = _prune(_load(), now)

    total = gained = 0.0
    counted = 0
    baseline = ""
    for series in data.values():
        days = sorted(series)
        if not days:
            continue
        total += series[days[-1]]
        counted += 1
        if stamp not in series:
            continue
        before = [day for day in days if day < stamp]
        if not before:
            continue                 # 今天第一次记录：没有基准，不算增减
        prev = before[-1]
        delta = series[stamp] - series[prev]
        if delta > 0:
            gained += delta
        baseline = max(baseline, prev) if baseline else prev

    # 没有记录的日子不画：从第一条记录开始，避免开头一串 0 把走势压平
    stamps = [day for series in data.values() for day in series]
    trend = []
    if stamps:
        begin = max(now - timedelta(days=trend_days - 1),
                    date.fromisoformat(min(stamps)))
        begin = min(begin, now)      # 时钟回拨留下的未来记录不能把走势挤空
        cursor = begin
        while cursor <= now:
            day = cursor.isoformat()
            trend.append((day, sum(value for value in
                                   (_latest(series, day) for series in data.values())
                                   if value is not None)))
            cursor += timedelta(days=1)

    return {
        "total": total,
        "accounts": counted,
        "gained": gained,
        "baseline": baseline,
        "today": stamp,
        "has_history": bool(baseline),
        "trend": trend,
    }


def clear() -> None:
    """清空历史（维护页用）。"""
    with _LOCK:
        _save({})
