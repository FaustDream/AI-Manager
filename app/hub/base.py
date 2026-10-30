# -*- coding: utf-8 -*-
"""技能 / MCP 同步的公共异常与结果类型。"""
from __future__ import annotations

from dataclasses import dataclass, field


class HubError(RuntimeError):
    """可展示给用户的同步失败原因。"""


@dataclass
class Step:
    """一次平台写入的结果，供界面汇总成「成功 N / 跳过 M / 失败 K」。"""

    ok: bool
    target: str
    message: str = ""
    path: str = ""
    conflict: bool = False
    skipped: bool = False


@dataclass
class Batch:
    steps: list[Step] = field(default_factory=list)

    def add(self, step: Step) -> None:
        self.steps.append(step)

    @property
    def ok_count(self) -> int:
        return sum(1 for step in self.steps if step.ok)

    @property
    def skipped(self) -> list[Step]:
        return [step for step in self.steps if step.skipped]

    @property
    def failed(self) -> list[Step]:
        return [step for step in self.steps
                if not step.ok and not step.conflict and not step.skipped]

    @property
    def conflicts(self) -> list[Step]:
        return [step for step in self.steps if step.conflict]

    def summary(self) -> str:
        if not self.steps:
            return "没有需要同步的平台"
        text = f"成功 {self.ok_count}"
        if self.skipped:
            text += f"，跳过 {len(self.skipped)}"
        if self.conflicts:
            text += f"，冲突 {len(self.conflicts)}"
        if self.failed:
            text += f"，失败 {len(self.failed)}"
        return text
