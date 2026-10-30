# -*- coding: utf-8 -*-
"""各 AI 编程平台的技能目录与 MCP 配置文件位置。

平台路径是这套功能的唯一事实来源：技能部署、MCP 写入、导入扫描都从这里取。
国内版客户端普遍带 `-cn` 后缀（Qoder CN / Trae CN），WorkBuddy 是例外：
国内版用 `~/.workbuddy`，国际版用 `~/.workbuddy-ai`。
"""
from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from ..core import paths

HOME = Path.home()


def _home(name: str) -> Path:
    return HOME / name


def _appdata(*parts: str) -> Path:
    return paths.user_appdata_dir().joinpath(*parts)


@dataclass(frozen=True)
class Target:
    key: str
    label: str
    group: str
    skill_dir: Path
    mcp_file: Path
    mcp_path: tuple[str, ...] = ("mcpServers",)
    home: Path | None = None
    note: str = ""
    # ZCode 的条目要补 type / enabled / timeoutMs，标准 mcpServers 不用
    zcode_style: bool = False
    # Trae 的 stdio 条目习惯带 url / headers 空值
    stdio_null_http: bool = False
    # CodeBuddy 用 disabled 表示启用状态
    disabled_flag: bool = False

    @property
    def installed(self) -> bool:
        return bool((self.home and self.home.exists())
                    or self.skill_dir.exists() or self.mcp_file.exists())

    def skill_path(self, name: str) -> Path:
        return self.skill_dir / name


TARGETS: tuple[Target, ...] = (
    Target(
        key="qoder_cn",
        label="Qoder CN",
        group="Qoder",
        skill_dir=_home(".qoder-cn") / "skills",
        mcp_file=_home(".qoder-cn") / "settings.json",
        home=_home(".qoder-cn"),
    ),
    Target(
        key="qoder_intl",
        label="Qoder 国际版",
        group="Qoder",
        skill_dir=_home(".qoder") / "skills",
        mcp_file=_home(".qoder") / "settings.json",
        home=_home(".qoder"),
    ),
    Target(
        key="trae_cn",
        label="Trae CN",
        group="Trae",
        skill_dir=_home(".trae-cn") / "skills",
        mcp_file=_appdata("Trae CN", "User", "mcp.json"),
        home=_home(".trae-cn"),
        stdio_null_http=True,
    ),
    Target(
        key="workbuddy_intl",
        label="WorkBuddy 国际版",
        group="WorkBuddy",
        skill_dir=_home(".workbuddy-ai") / "skills",
        mcp_file=_home(".workbuddy-ai") / "mcp.json",
        home=_home(".workbuddy-ai"),
    ),
    Target(
        key="workbuddy_cn",
        label="WorkBuddy 国内版",
        group="WorkBuddy",
        skill_dir=_home(".workbuddy") / "skills",
        mcp_file=_home(".workbuddy") / "mcp.json",
        home=_home(".workbuddy"),
    ),
    Target(
        key="codebuddy",
        label="CodeBuddy（IDE / 插件版）",
        group="CodeBuddy",
        skill_dir=_home(".codebuddy") / "skills",
        mcp_file=_home(".codebuddy") / "mcp.json",
        home=_home(".codebuddy"),
        note="IDE 与 VS Code / JetBrains 插件共用此目录",
        disabled_flag=True,
    ),
    Target(
        key="zcode",
        label="ZCode",
        group="ZCode",
        skill_dir=_home(".zcode") / "skills",
        mcp_file=_home(".zcode") / "cli" / "config.json",
        mcp_path=("mcp", "servers"),
        home=_home(".zcode"),
        zcode_style=True,
    ),
)

TARGET_MAP = {target.key: target for target in TARGETS}
TARGET_KEYS = tuple(target.key for target in TARGETS)
GROUPS = tuple(dict.fromkeys(target.group for target in TARGETS))


def get(key: str) -> Target | None:
    return TARGET_MAP.get(key)


def labels(keys) -> list[str]:
    return [TARGET_MAP[key].label for key in keys if key in TARGET_MAP]


def short_path(path: Path) -> str:
    """界面展示用的紧凑路径：家目录换成 ~，反斜杠换成正斜杠。"""
    text = str(path)
    home = str(HOME)
    if text.lower().startswith(home.lower()):
        text = "~" + text[len(home):]
    return text.replace("\\", "/")
