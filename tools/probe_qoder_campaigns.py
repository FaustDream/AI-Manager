# -*- coding: utf-8 -*-
"""一次性探针：打印 Qoder 两版 campaigns 接口的真实返回，用来确定签到窗口口径。"""
from __future__ import annotations

import sys
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from app.checkin.qoder import QoderIntlProvider, QoderProvider  # noqa: E402


def local(ts) -> str:
    if not isinstance(ts, (int, float)):
        return str(ts)
    seconds = ts / 1000 if ts > 1e11 else ts
    return datetime.fromtimestamp(seconds, tz=timezone.utc).astimezone().strftime(
        "%Y-%m-%d %H:%M:%S %Z")


for provider in (QoderProvider(), QoderIntlProvider()):
    print(f"\n=== {provider.label} / {provider.base} ===")
    accounts = provider.scan(lambda message: print("  scan:", message))
    print(f"账号数 {len(accounts)}")
    for account in accounts:
        print(f"\n-- {account.name} ({account.account_id}) --")
        try:
            session = provider._session(account.token)
            doc = provider._get(session, "/sash/api/v1/me/campaigns")
        except Exception as exc:
            print("  请求失败：", exc)
            continue
        print(f"  showCampaign={doc.get('showCampaign')} claimable={doc.get('claimable')}")
        for index, item in enumerate(doc.get("campaigns") or []):
            if not isinstance(item, dict):
                continue
            benefits = item.get("benefit") or {}
            print(f"  [{index}] {item.get('campaignKey')} {item.get('actionType')} "
                  f"{item.get('claimStatus')}")
            print(f"      start {local(item.get('startAt'))}  "
                  f"end {local(item.get('endAt'))}")
            if benefits:
                print(f"      benefit {benefits.get('kind')} {benefits.get('amount')} "
                      f"{benefits.get('validity')}")
        print(f"  本地 now = {datetime.now():%Y-%m-%d %H:%M:%S} "
              f"/ UTC = {datetime.now(timezone.utc):%Y-%m-%d %H:%M:%S}")
