# -*- coding: utf-8 -*-
"""provider 注册表。"""
from __future__ import annotations

from .base import (Account, ActionResult, CreditSnapshot, CreditLot, Provider,
                   StatusResult, UnsupportedAccount)
from .qoder import QoderIntlProvider, QoderProvider
from .trae import TraeProvider
from .workbuddy import WorkBuddyProvider
from .zstore import ZCodeProvider

PROVIDERS = (TraeProvider(), QoderProvider(), QoderIntlProvider(), WorkBuddyProvider(),
             ZCodeProvider())
PROVIDER_MAP = {p.key: p for p in PROVIDERS}

LABELS = {p.key: p.label for p in PROVIDERS}
LABEL_KEYS = {label: key for key, label in LABELS.items()}
ORDER = ("trae", "qoder", "qoder_intl", "workbuddy", "zcode")

__all__ = ["Account", "ActionResult", "CreditLot", "CreditSnapshot", "Provider",
           "StatusResult", "UnsupportedAccount", "PROVIDERS", "PROVIDER_MAP",
           "LABELS", "LABEL_KEYS", "ORDER"]
