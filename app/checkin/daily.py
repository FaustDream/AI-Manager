# -*- coding: utf-8 -*-
"""每日签到轮询：日期换天 → 账号状态校验 → 才调用签到接口。

签到原来挂在界面上的「每日自动签到 + 时刻选择」上：窗口没开就不执行，
同一自然日还可能被反复触发。这里把节奏固定为「启动一次 + 每 6 小时一次」，
用 data/daily.json 记录当日已签成功的账号，做到换天才签、同一天不重复。

本模块不依赖 Qt：网络与文件读写都在调用方的后台线程里完成，
过程事件通过 emit 交给界面层渲染。
"""
from __future__ import annotations

import json
import os
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from datetime import datetime

from ..core import paths
from . import service
from .base import Account, StatusResult

# 界面层按这个间隔触发轮询；改这里就同时改了「每 6 小时一次」的口径
INTERVAL_HOURS = 6
_DAY_FMT = "%Y-%m-%d"
_STAMP_FMT = "%Y-%m-%dT%H:%M:%S"
_WORKERS = 4


# ------------------------------------------------------------------ 当日状态
def _blank() -> dict:
    return {"date": "", "done": [], "last_round": "", "last_result": ""}


def load_state() -> dict:
    """读 data/daily.json；损坏或缺失时按「还没轮询过」处理。"""
    try:
        raw = json.loads(paths.DAILY_FILE.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return _blank()
    if not isinstance(raw, dict):
        return _blank()
    state = _blank()
    state["date"] = str(raw.get("date") or "")
    done = raw.get("done")
    if isinstance(done, list):
        state["done"] = [str(item) for item in done if isinstance(item, str)]
    state["last_round"] = str(raw.get("last_round") or "")
    state["last_result"] = str(raw.get("last_result") or "")
    return state


def _save_state(state: dict, log=None) -> None:
    """先写临时文件再替换：中途崩溃不能留下半个 JSON，否则去重表会失效。

    写盘失败必须上报：去重表丢了，下一轮可能对同一天重复调用签到接口。
    """
    path = paths.DAILY_FILE
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_suffix(".json.tmp")
        tmp.write_text(json.dumps(state, ensure_ascii=False, indent=2), encoding="utf-8")
        os.replace(tmp, path)
    except OSError as exc:
        if log is not None:
            log(f"每日轮询状态写入失败：{exc}", "error")


def describe() -> str:
    """设置页展示的一行状态：上次轮询时间与结果。"""
    state = load_state()
    stamp = state["last_round"]
    if not stamp:
        return "还没有轮询记录，启动后会自动检查一次"
    try:
        when = datetime.strptime(stamp, _STAMP_FMT)
        text = f"上次轮询 {when:%m-%d %H:%M}"
    except ValueError:
        text = f"上次轮询 {stamp}"
    if state["last_result"]:
        text += f" · {state['last_result']}"
    return text


@dataclass
class RoundResult:
    """一轮轮询的统计，界面只用来拼提示文案。"""

    new_day: bool = False
    scanned: int = 0
    due: int = 0
    success: int = 0
    already: int = 0
    failed: int = 0
    skipped: int = 0
    # 接口给了下一轮开放时刻、还没到点的账号数（例如 Qoder 10:00 前）
    waiting: int = 0
    when: datetime = field(default_factory=datetime.now)

    def summary(self) -> str:
        parts = ["新的一天" if self.new_day else "同日轮询", f"{self.scanned} 个账号"]
        if self.due:
            parts.append(f"可签 {self.due}")
        if self.success:
            parts.append(f"成功 {self.success}")
        if self.already:
            parts.append(f"已领 {self.already}")
        if self.waiting:
            parts.append(f"等待窗口 {self.waiting}")
        if self.failed:
            parts.append(f"失败 {self.failed}")
        if not self.due and not self.already and not self.waiting:
            parts.append("无需签到")
        return " · ".join(parts)


# ---------------------------------------------------------------------- 主流程
def run_round(cfg: dict, log, emit=None, now: datetime | None = None) -> RoundResult:
    """跑一轮完整流程，顺序固定：日期检测 → 状态校验 → 签到。

    必须在后台线程调用。emit 收到的事件名与账号中心页面已有的渲染事件一致，
    界面层直接复用同一套处理逻辑。
    """
    moment = now or datetime.now()
    today = moment.strftime(_DAY_FMT)

    # 第一步：日期检测。换天才清空当日记录；同一天沿用，避免重复签到
    state = load_state()
    new_day = state["date"] != today
    done = set() if new_day else set(state["done"])
    result = RoundResult(new_day=new_day, when=moment)
    if new_day:
        log(f"日期轮询：进入新的一天 {today}，当日签到记录清零")
    else:
        log(f"日期轮询：仍是 {today}，按当日记录去重", "dim")

    accounts = service.collect(cfg, log)
    result.scanned = len(accounts)
    if emit is not None:
        emit(("accounts", accounts))

    skip = set(cfg.get("skip") or [])
    targets = [a for a in accounts if a.requires_checkin and a.iid not in skip]

    # 第二步：逐账号校验接口状态（网络请求，并发跑）
    statuses: dict[str, StatusResult] = {}

    def query(account: Account) -> None:
        kind, iid, payload = service.query_one(account, False, log)
        if kind != "status":
            return
        statuses[iid] = payload
        if emit is not None:
            emit(("status", (iid, payload)))

    if targets:
        with ThreadPoolExecutor(max_workers=_WORKERS) as pool:
            list(pool.map(query, targets))

    # 免签到账号（WorkBuddy 国际版、手动设了免签的）不走状态查询，但界面在
    # emit("accounts") 时已经整表重画，不在这里补一次积分，它们的余额就一直是「—」。
    target_ids = {account.iid for account in targets}
    others = [account for account in accounts if account.iid not in target_ids]

    def fetch_credit(account: Account) -> None:
        snapshot = service.query_credit(account, log)
        if emit is not None:
            emit(("credit", (account.iid, snapshot)))

    if others:
        with ThreadPoolExecutor(max_workers=_WORKERS) as pool:
            list(pool.map(fetch_credit, others))

    # 第三步：只对「今天还没签成功 + 接口说现在可签」的账号调用签到接口
    due: list[Account] = []
    for account in targets:
        iid = account.iid
        status = statuses.get(iid)
        checked = status.checked_in if status is not None else None
        if checked is True:
            done.add(iid)             # 接口说今天已签，写进当日去重表
            result.skipped += 1
            continue
        if iid in done:
            result.skipped += 1       # 今天已经由本工具签成功过
            continue
        if service.checked_today(iid):
            done.add(iid)             # 接口没给状态时用本地记录兜底
            result.skipped += 1
            continue
        nxt = status.next_at if status is not None else None
        if nxt is not None and nxt > moment:
            result.skipped += 1
            result.waiting += 1       # 下一轮窗口还没开，等下一次轮询
            continue
        due.append(account)

    result.due = len(due)
    if not due:
        log("账号状态校验：没有可签到的账号，本轮跳过", "dim")

    for account in due:
        log(f"[{account.platform_label}] 正在为 {account.name} 签到…", "dim")
        outcome = service.checkin_one(account, log)
        if outcome.ok and not outcome.already:
            # 「本轮早已领过」没有新发放，不能记成今天签过，留给下一轮再试
            service.mark_checked(account.iid)
            service.record_grant(account, outcome)
            done.add(account.iid)
            result.success += 1
        elif outcome.ok:
            result.already += 1
        else:
            result.failed += 1
        if emit is not None:
            emit(("checkin", (account, outcome)))
        log(f"[{account.platform_label}] {account.name}：{outcome.message}",
            "ok" if outcome.ok else "error")

    _save_state({
        "date": today,
        "done": sorted(done),
        "last_round": moment.strftime(_STAMP_FMT),
        "last_result": result.summary(),
    }, log)
    log(f"每日轮询完成：{result.summary()}", "ok" if result.success else "dim")
    return result
