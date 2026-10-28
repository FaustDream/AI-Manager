# -*- coding: utf-8 -*-
"""provider 注册表。"""
from __future__ import annotations

from .base import (Account, ActionResult, CreditSnapshot, CreditLot, Provider,
                   StatusResult, UnsupportedAccount)
from .qoder import QoderIntlProvider, QoderProvider
from .trae import TraeProvider
from .workbuddy import WorkBuddyProvider

PROVIDERS = (TraeProvider(), QoderProvider(), QoderIntlProvider(), WorkBuddyProvider())
PROVIDER_MAP = {p.key: p for p in PROVIDERS}

LABELS = {p.key: p.label for p in PROVIDERS}
LABEL_KEYS = {label: key for key, label in LABELS.items()}
ORDER = ("trae", "qoder", "qoder_intl", "workbuddy")

__all__ = ["Account", "ActionResult", "CreditLot", "CreditSnapshot", "Provider",
           "StatusResult", "UnsupportedAccount", "PROVIDERS", "PROVIDER_MAP",
           "LABELS", "LABEL_KEYS", "ORDER"]
