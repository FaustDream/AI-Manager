# -*- coding: utf-8 -*-
"""签到状态按天留档：一天只问一次接口，之后切页面直接读记录。

平台只提供「查询签到状态」这一个入口，没有更便宜的判断方式；总览和账号中心
每次刷新都要为每个账号打一遍，既慢又容易被平台当成异常调用。当天第一次查到
的结果存进 data/checkin_status.json，同一自然日内不再重复查询，只刷新积分。

活动制平台（Qoder）的「下次可签」时刻只由接口下发，本地推算不成立，
所以 next_at 也一并留档：重启后详情面板的倒计时不必等下一轮轮询。
"""
from __future__ import annotations

import json
import os
import threading
from datetime import date, datetime

from ..core import paths

_STAMP = "%Y-%m-%dT%H:%M:%S"

# 各账号的查询在后台线程并发执行，写同一份文件必须串行
_LOCK = threading.Lock()


def _parse(value) -> datetime | None:
    try:
        return datetime.strptime(str(value), _STAMP)
    except (TypeError, ValueError):
        return None


def _entry(checked=None, next_at=None) -> dict | None:
    """规整成一条记录；两项都空就当没查过，返回 None。"""
    item = {"checked": checked if isinstance(checked, bool) else None,
            "next_at": next_at if isinstance(next_at, datetime) else None}
    return item if item["checked"] is not None or item["next_at"] else None


def _load() -> dict:
    try:
        raw = json.loads(paths.CHECKIN_STATUS_FILE.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return raw if isinstance(raw, dict) else {}


def _save(data: dict) -> None:
    """先写临时文件再替换：崩在半路也不能留下半个 JSON。"""
    payload = {
        "date": data["date"],
        "accounts": {
            iid: {"checked": item["checked"],
                  "next_at": item["next_at"].strftime(_STAMP) if item["next_at"] else None}
            for iid, item in data["accounts"].items()
        },
    }
    path = paths.CHECKIN_STATUS_FILE
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_suffix(".json.tmp")
        tmp.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")
        os.replace(tmp, path)
    except OSError:
        pass


def _today_data(day: date) -> dict:
    """换天即作废：记录里不是今天的日期就当没查过，不需要额外清理。"""
    raw = _load()
    stamp = day.isoformat()
    if str(raw.get("date") or "") != stamp or not isinstance(raw.get("accounts"), dict):
        return {"date": stamp, "accounts": {}}
    accounts = {}
    for iid, item in raw["accounts"].items():
        if not isinstance(item, dict):
            continue
        entry = _entry(item.get("checked"), _parse(item.get("next_at")))
        if entry is not None:
            accounts[iid] = entry
    return {"date": stamp, "accounts": accounts}


def today(day: date | None = None) -> dict[str, dict]:
    """今天已确认过的记录：{iid: {"checked": bool | None, "next_at": datetime | None}}。"""
    return dict(_today_data(day or date.today())["accounts"])


def note_many(answers: dict[str, dict], day: date | None = None) -> None:
    """整批写入今天查到的结果，一次落盘。

    只覆盖 answers 里出现过的字段：签到成功时只想记 checked，不能顺手把
    next_at 抹掉；显式传 None 才表示「这次确实没有」。
    """
    clean: dict[str, dict] = {}
    for iid, item in answers.items():
        if not iid or not isinstance(item, dict):
            continue
        entry: dict = {}
        if "checked" in item:
            entry["checked"] = item["checked"] if isinstance(item["checked"], bool) else None
        if "next_at" in item:
            moment = item["next_at"]
            entry["next_at"] = moment if isinstance(moment, datetime) else None
        if entry:
            clean[iid] = entry
    if not clean:
        return
    with _LOCK:
        data = _today_data(day or date.today())
        for iid, entry in clean.items():
            merged = dict(data["accounts"].get(iid) or {})
            merged.update(entry)
            merged.setdefault("checked", None)
            merged.setdefault("next_at", None)
            if merged["checked"] is None and merged["next_at"] is None:
                data["accounts"].pop(iid, None)
            else:
                data["accounts"][iid] = merged
        _save(data)
