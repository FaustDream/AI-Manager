# -*- coding: utf-8 -*-
"""各平台 MCP 配置的读写：标准 mcpServers 与 ZCode 的 mcp.servers 两种格式。

只改自己登记过的条目，平台原有条目和未知字段原样保留；每次写盘前把原文件
备份到 data/backups/hub-<时间戳>/<平台>/，写坏了还能拿回来。
"""
from __future__ import annotations

import copy
import hashlib
import json
import os
import re
import shlex
import shutil
import time
from datetime import datetime
from pathlib import Path

from ..core import paths
from . import store
from .base import Batch, HubError, Step
from .targets import TARGET_MAP, Target

_TRAILING_COMMA = re.compile(r",(\s*[}\]])")
_SENSITIVE = re.compile(r"(key|token|secret|password|passwd|auth|cookie)", re.I)


def _now() -> str:
    return datetime.now().isoformat(timespec="seconds")


# ------------------------------------------------------------------ JSON 读写
def strip_comments(text: str) -> str:
    """VS Code 系的 json 允许注释；解析前去掉，字符串里的 // 不受影响。"""
    out: list[str] = []
    index = 0
    in_string = False
    escaped = False
    while index < len(text):
        char = text[index]
        if in_string:
            out.append(char)
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                in_string = False
            index += 1
            continue
        if char == '"':
            in_string = True
            out.append(char)
            index += 1
            continue
        if char == "/" and index + 1 < len(text):
            nxt = text[index + 1]
            if nxt == "/":
                index += 2
                while index < len(text) and text[index] not in "\r\n":
                    index += 1
                continue
            if nxt == "*":
                index += 2
                while index + 1 < len(text) and not (text[index] == "*"
                                                     and text[index + 1] == "/"):
                    index += 1
                index += 2
                continue
        out.append(char)
        index += 1
    return "".join(out)


def read_doc(path: Path) -> dict:
    if not path.exists():
        return {}
    try:
        text = path.read_text(encoding="utf-8-sig")
    except OSError as exc:
        raise HubError(f"读取 {path} 失败：{exc}") from exc
    if not text.strip():
        return {}
    try:
        data = json.loads(text)
    except ValueError:
        try:
            data = json.loads(_TRAILING_COMMA.sub(r"\1", strip_comments(text)))
        except ValueError as exc:
            raise HubError(f"{path} 不是合法 JSON：{exc}") from exc
    if not isinstance(data, dict):
        raise HubError(f"{path} 的顶层不是对象，已跳过")
    return data


def save_doc(path: Path, doc: dict, target: Target, stamp: str) -> None:
    _backup(path, target, stamp)
    tmp = path.with_name(f".{path.name}.hubtmp-{os.getpid()}")
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp.write_text(json.dumps(doc, ensure_ascii=False, indent=2) + "\n",
                       encoding="utf-8")
        os.replace(tmp, path)
    except OSError as exc:
        store.remove_path(tmp)
        raise HubError(f"写入 {path} 失败：{exc}（客户端可能正在占用，关闭后重试）") from exc


def _backup(path: Path, target: Target, stamp: str) -> None:
    if not path.exists():
        return
    folder = paths.HUB_BACKUP_ROOT / f"hub-{stamp}" / target.key
    backup = folder / path.name
    if backup.exists():
        return                     # 同一轮里已经备份过原件
    try:
        folder.mkdir(parents=True, exist_ok=True)
        shutil.copy2(path, backup)
    except OSError as exc:
        raise HubError(f"备份 {path} 失败：{exc}") from exc


# ------------------------------------------------------------------ 结构访问
def container(doc: dict, target: Target, create: bool = True) -> dict:
    node = doc
    for key in target.mcp_path:
        value = node.get(key)
        if value is None:
            if not create:
                return {}
            value = {}
            node[key] = value
        if not isinstance(value, dict):
            raise HubError(f"{target.mcp_file} 的 {'.'.join(target.mcp_path)} 不是对象")
        node = value
    return node


def servers(doc: dict, target: Target) -> dict:
    return container(doc, target, create=False)


def kind(config: dict) -> str:
    if config.get("url"):
        return str(config.get("type") or "http")
    return str(config.get("type") or "stdio")


_TYPE_ALIASES = {
    "stdio": "stdio",
    "http": "http",
    "sse": "sse",
    "streamable-http": "http",
    "streamablehttp": "http",
    "streamable_http": "http",
}


def normalize_config(config: dict) -> dict:
    """把各平台写法收敛成一份统一配置，导入 / 跨平台写入都走这里。

    - `enabled` 与 `disabled` 相反，统一存 `disabled`
    - `streamableHttp` / `streamable-http` 统一成 `http`
    - 去掉 `fromGalleryId`、空的 `url` / `headers` 等平台私有字段
    """
    cfg = copy.deepcopy(config)
    if "enabled" in cfg and "disabled" not in cfg:
        cfg["disabled"] = not bool(cfg["enabled"])
    cfg.pop("enabled", None)

    raw_type = str(cfg.get("type") or "").strip().lower()
    if raw_type in _TYPE_ALIASES:
        cfg["type"] = _TYPE_ALIASES[raw_type]
    elif cfg.get("url"):
        cfg["type"] = "http"
    else:
        cfg["type"] = "stdio"

    if not cfg.get("url") and isinstance(cfg.get("serverUrl"), str):
        cfg["url"] = cfg.pop("serverUrl")
    # 类型和 url 必须一致：有 url 就是 http/sse，没有 url 就按 stdio 处理
    if cfg.get("url"):
        if cfg.get("type") not in ("http", "sse"):
            cfg["type"] = "http"
    elif cfg.get("type") in ("http", "sse"):
        cfg["type"] = "stdio"
    for key in ("url", "headers"):
        if cfg.get(key) is None:
            cfg.pop(key, None)
    for key in ("fromGalleryId", "from_gallery_id", "serverUrl",
                "name", "server_name"):
        cfg.pop(key, None)

    args = cfg.get("args")
    command = cfg.get("command")
    if not args and isinstance(command, str) and " " in command.strip():
        # Trae 的市集条目常把「npx -y xxx」整串塞进 command，其他平台不会拆；
        # 路径本身存在（带空格的安装路径）时保持原样，避免拆坏。
        if not Path(command.strip()).exists():
            try:
                parts = shlex.split(command.strip(), posix=False)
            except ValueError:
                parts = command.split()
            if len(parts) > 1:
                cfg["command"] = parts[0].strip('"')
                cfg["args"] = [part.strip('"') for part in parts[1:]]
                args = cfg["args"]
                command = cfg["command"]
    if isinstance(args, list):
        cfg["args"] = [str(item) for item in args]
    elif args is not None:
        cfg["args"] = [str(args)]

    for key in ("env", "headers"):
        if key in cfg and not isinstance(cfg[key], dict):
            cfg.pop(key, None)
    if cfg.get("command") is not None:
        cfg["command"] = str(cfg["command"])
    if not cfg.get("disabled"):
        cfg.pop("disabled", None)
    if "timeoutMs" in cfg:
        try:
            cfg["timeoutMs"] = int(cfg["timeoutMs"])
        except (TypeError, ValueError):
            cfg.pop("timeoutMs", None)
    return cfg


def _config_score(config: dict) -> int:
    """挑「信息最完整、最可移植」的那份作为合并底稿。"""
    score = 0
    command = str(config.get("command") or "")
    if command:
        score += 10
        if " " not in command.strip():
            score += 8          # 单命令 + args 比「npx -y xxx」整串更通用
        if Path(command).suffix.lower() in (".exe", ".cmd", ".bat", ".py", ".js", ".mjs"):
            score += 2
        try:
            if Path(command).is_file():
                score += 6      # 本机存在的绝对路径优先，避免选到已失效的命令
        except OSError:
            pass
    if config.get("url"):
        score += 10
    args = config.get("args") or []
    if args:
        score += 3 + min(len(args), 5)
    if config.get("env"):
        score += 2 + len(config["env"])
    if config.get("headers"):
        score += 2 + len(config["headers"])
    if config.get("cwd"):
        score += 1
    if config.get("description"):
        score += 1
    return score


def _differences(configs: list[dict]) -> list[str]:
    keys: set[str] = set()
    for config in configs:
        keys.update(config)
    out = []
    for key in sorted(keys):
        values = {json.dumps(config.get(key), ensure_ascii=False, sort_keys=True)
                  for config in configs}
        if len(values) > 1:
            out.append(key)
    return out


def merge_configs(configs: list[dict]) -> tuple[dict, list[str]]:
    """同名 MCP 在多个平台写法不同：取最完整的一份，再合并 env / headers。"""
    normalized = [normalize_config(config) for config in configs
                  if isinstance(config, dict)]
    if not normalized:
        return {}, []
    base = max(normalized, key=_config_score)
    http_like = bool(base.get("url"))
    same_kind = [config for config in normalized
                 if bool(config.get("url")) == http_like]
    merged = copy.deepcopy(base)
    for config in same_kind:
        for key in ("env", "headers"):
            source = config.get(key)
            if not isinstance(source, dict):
                continue
            bucket = merged.setdefault(key, {})
            if not isinstance(bucket, dict):
                bucket = {}
                merged[key] = bucket
            for name, value in source.items():
                if name not in bucket or bucket.get(name) in (None, ""):
                    bucket[name] = value
        if not merged.get("args") and config.get("args"):
            merged["args"] = copy.deepcopy(config["args"])
        if config.get("command") == merged.get("command") \
                and len(config.get("args") or []) > len(merged.get("args") or []):
            merged["args"] = copy.deepcopy(config["args"])
        for key in ("cwd", "description"):
            if not merged.get(key) and config.get(key):
                merged[key] = config[key]
        if not merged.get("timeoutMs") and config.get("timeoutMs"):
            merged["timeoutMs"] = config["timeoutMs"]
        if config.get("disabled"):
            merged["disabled"] = True
    merged["type"] = "http" if merged.get("url") else (merged.get("type") or "stdio")
    return merged, _differences(same_kind)


def _looks_like_server(data: dict) -> bool:
    if any(key in data for key in ("command", "url", "serverUrl")):
        return True
    kind_name = str(data.get("type") or "").strip().lower()
    return kind_name in _TYPE_ALIASES


def suggest_name(config: dict, index: int = 0) -> str:
    if config.get("url"):
        from urllib.parse import urlsplit
        host = urlsplit(str(config["url"])).hostname or ""
        base = re.sub(r"[^A-Za-z0-9._-]+", "-", host).strip("-") or "mcp-server"
    else:
        command = Path(str(config.get("command") or "")).stem
        base = f"{command}-mcp" if command else "mcp-server"
    return f"{base}-{index + 1}" if index else base


def parse_payload(data) -> list[tuple[str, dict]]:
    """识别标准 mcpServers、ZCode mcp.servers、单个服务或服务数组。"""
    if isinstance(data, dict):
        if isinstance(data.get("mcpServers"), dict):
            return [(str(name), config) for name, config in data["mcpServers"].items()
                    if isinstance(config, dict)]
        mcp_box = data.get("mcp")
        if isinstance(mcp_box, dict) and isinstance(mcp_box.get("servers"), dict):
            return [(str(name), config)
                    for name, config in mcp_box["servers"].items()
                    if isinstance(config, dict)]
        if _looks_like_server(data):
            name = str(data.get("name") or data.get("server_name") or "").strip()
            return [(name, data)]
        if data and all(isinstance(value, dict) for value in data.values()):
            return [(str(name), config) for name, config in data.items()
                    if isinstance(config, dict)]
        return []
    if isinstance(data, list):
        return [(str(item.get("name") or item.get("server_name") or "").strip(), item)
                for item in data if isinstance(item, dict)]
    return []


def parse_text(text: str) -> list[tuple[str, dict]]:
    """把粘贴 / 文件里的 JSON 变成可直接导入的条目，自动补名称。"""
    try:
        data = json.loads(text)
    except ValueError as exc:
        raise HubError(f"JSON 解析失败：{exc}") from exc
    raw = parse_payload(data)
    if not raw:
        raise HubError("没有识别到 MCP 配置：需要 mcpServers、mcp.servers，"
                       "或包含 command / url 的单个服务")
    entries: list[tuple[str, dict]] = []
    used: set[str] = set()
    for index, (name, config) in enumerate(raw):
        cfg = normalize_config(config)
        if not cfg.get("command") and not cfg.get("url"):
            continue
        base = name or suggest_name(cfg, index)
        base = re.sub(r"[^A-Za-z0-9._-]+", "-", base).strip("-") or "mcp-server"
        candidate, suffix = base, 2
        while candidate.lower() in used:
            candidate = f"{base}-{suffix}"
            suffix += 1
        used.add(candidate.lower())
        entries.append((candidate, cfg))
    if not entries:
        raise HubError("识别到了条目，但没有一个包含 command 或 url")
    return entries


def summary(config: dict) -> str:
    """表格里显示的一行摘要：只留命令名 / 域名，参数里的密钥一律遮掉。"""
    if config.get("url"):
        from urllib.parse import urlsplit
        parts = urlsplit(str(config["url"]))
        host = parts.hostname or ""
        port = f":{parts.port}" if parts.port else ""
        text = f"{parts.scheme}://{host}{port}{parts.path}"
        return text + ("?…" if parts.query else "")
    command = str(config.get("command") or "")
    args = config.get("args") or []
    parts = [Path(command).name or command]
    if isinstance(args, list):
        parts.extend(_mask_args(args))
    return " ".join(part for part in parts if part) or "（未配置命令）"


def _mask_args(args) -> list[str]:
    out: list[str] = []
    index = 0
    while index < len(args) and len(out) < 4:
        text = str(args[index])
        out.append(_mask_arg(text))
        if re.match(r"(?i)^--?[A-Za-z0-9-]*(key|token|secret|password|auth)(=|$)",
                    text):
            if "=" not in text and index + 1 < len(args):
                out.append("***")
                index += 2
                continue
        index += 1
    return out


def _mask_arg(value) -> str:
    text = str(value)
    if re.search(r"(?i)(key|token|secret|password|auth)=", text):
        return "***"
    if re.match(r"(?i)^(sk-|ghp_|gho_|xoxb-|Bearer\s)", text):
        return "***"
    if len(text) > 80 and re.fullmatch(r"[A-Za-z0-9_.\-]+", text):
        return text[:6] + "…"
    return text


def entry_for(config: dict, target: Target) -> dict:
    entry = copy.deepcopy(config)
    if target.zcode_style:
        z_kind = kind(config)
        if z_kind not in ("stdio", "http", "sse"):
            z_kind = "http" if config.get("url") else "stdio"
        entry["type"] = z_kind
        # 统一配置用 disabled 表示停用，ZCode 用 enabled，写的时候翻一下
        entry["enabled"] = not bool(config.get("disabled", False))
        entry.pop("disabled", None)
        try:
            entry["timeoutMs"] = int(config.get("timeoutMs") or 30000)
        except (TypeError, ValueError):
            entry["timeoutMs"] = 30000
        if z_kind == "stdio":
            entry.setdefault("args", [])
        else:
            entry.pop("command", None)
            entry.pop("args", None)
    else:
        if entry.get("type") == "stdio":
            # 标准 mcpServers 的 stdio 条目不需要 type，去掉以贴近各平台原有格式
            entry.pop("type", None)
        entry.pop("enabled", None)
        entry.pop("timeoutMs", None)
        if target.disabled_flag:
            entry["disabled"] = bool(config.get("disabled", False))
        if target.stdio_null_http and not entry.get("url"):
            # Trae 的 stdio 条目习惯带这两个空字段，补上更贴近它自己写出的格式
            entry.setdefault("url", None)
            entry.setdefault("headers", None)
    return entry


def hash_entry(entry: dict) -> str:
    raw = json.dumps(entry, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    return hashlib.blake2b(raw.encode("utf-8"), digest_size=16).hexdigest()


def redact(config: dict) -> dict:
    """详情里预览配置时遮住 env / headers 中的密钥，避免把 token 摆在界面上。"""
    def scrub(node):
        if isinstance(node, dict):
            out = {}
            for key, value in node.items():
                if isinstance(value, str) and _SENSITIVE.search(str(key)):
                    out[key] = "***"
                else:
                    out[key] = scrub(value)
            return out
        if isinstance(node, list):
            return [scrub(item) for item in node]
        return node

    return scrub(copy.deepcopy(config))


def entry(hub: dict, name: str, config: dict | None = None) -> dict:
    item = hub["mcp"].get(name)
    if item is None:
        item = {"config": copy.deepcopy(config or {}), "targets": [],
                "deployed": {}, "preserve": False}
        hub["mcp"][name] = item
    item.setdefault("config", {})
    item.setdefault("targets", [])
    item.setdefault("deployed", {})
    item.setdefault("preserve", False)
    return item


# ------------------------------------------------------------------ 部署
def deploy(name: str, config: dict, keys, hub: dict,
           overwrite: set[str] | None = None,
           batch: Batch | None = None, stamp: str | None = None) -> Batch:
    batch = batch or Batch()
    overwrite = overwrite or set()
    config = normalize_config(config)
    stamp = stamp or time.strftime("%Y%m%d-%H%M%S")
    item = entry(hub, name, config)
    item["config"] = copy.deepcopy(config)
    for key in keys:
        target = TARGET_MAP.get(key)
        if target is None:
            batch.add(Step(False, key, "未知平台"))
            continue
        try:
            doc = read_doc(target.mcp_file)
            box = container(doc, target)
            current = box.get(name)
            wanted = entry_for(config, target)
            record = (item.get("deployed") or {}).get(key)
            if current is not None and not record and key not in overwrite \
                    and current != wanted:
                batch.add(Step(False, key, "平台已有同名 MCP，未覆盖",
                               str(target.mcp_file), conflict=True))
                continue
            if current == wanted:
                item["deployed"][key] = {"at": _now(), "hash": hash_entry(wanted)}
                batch.add(Step(True, key, "已是最新", str(target.mcp_file)))
                continue
            box[name] = wanted
            save_doc(target.mcp_file, doc, target, stamp)
        except HubError as exc:
            batch.add(Step(False, key, str(exc), str(target.mcp_file)))
            continue
        item["deployed"][key] = {"at": _now(), "hash": hash_entry(wanted)}
        batch.add(Step(True, key, "已写入", str(target.mcp_file)))
    return batch


def remove(name: str, keys, hub: dict, batch: Batch | None = None,
           stamp: str | None = None) -> Batch:
    batch = batch or Batch()
    stamp = stamp or time.strftime("%Y%m%d-%H%M%S")
    item = hub["mcp"].get(name)
    if item is None:
        return batch
    deployed = item.get("deployed") or {}
    for key in keys:
        target = TARGET_MAP.get(key)
        if target is None:
            continue
        if key not in deployed:
            batch.add(Step(False, key, "不是本工具写入的，已跳过",
                           str(target.mcp_file), skipped=True))
            continue
        try:
            doc = read_doc(target.mcp_file)
            box = container(doc, target)
            if name in box:
                box.pop(name)
                save_doc(target.mcp_file, doc, target, stamp)
        except HubError as exc:
            batch.add(Step(False, key, str(exc), str(target.mcp_file)))
            continue
        deployed.pop(key, None)
        item["targets"] = [value for value in item.get("targets") or [] if value != key]
        batch.add(Step(True, key, "已移除", str(target.mcp_file)))
    return batch


def set_targets(name: str, keys, hub: dict, batch: Batch | None = None) -> Batch:
    batch = batch or Batch()
    item = entry(hub, name)
    old = set(item.get("targets") or [])
    new = set(keys)
    removed = old - new
    added = new - old
    if removed:
        remove(name, removed, hub, batch)
    item["targets"] = sorted(new)
    if added:
        # 只写新加的平台：已导入的平台保留各自原配置，不因为调整平台被覆盖
        deploy(name, item.get("config") or {}, sorted(added), hub, batch=batch)
    return batch


# ------------------------------------------------------------------ 状态 / 导入
def read_all() -> tuple[dict[str, dict], dict[str, str]]:
    """一次读出所有平台的服务器列表：{平台: {名称: 配置}}，附带读取错误。"""
    result: dict[str, dict] = {}
    errors: dict[str, str] = {}
    for key, target in TARGET_MAP.items():
        try:
            result[key] = servers(read_doc(target.mcp_file), target)
        except HubError as exc:
            result[key] = {}
            errors[key] = str(exc)
    return result, errors


def state(item: dict, name: str, key: str, current: dict | None) -> str:
    if current is None:
        return "missing"
    record = (item.get("deployed") or {}).get(key) or {}
    if not record:
        target = TARGET_MAP.get(key)
        if target is not None and current == entry_for(item.get("config") or {}, target):
            return "synced"
        return "untracked"
    return "synced" if record.get("hash") == hash_entry(current) else "stale"


def overall(item: dict, current_all: dict[str, dict], name: str) -> str:
    targets = item.get("targets") or []
    if not targets:
        return "unassigned"
    states = [state(item, name, key, (current_all.get(key) or {}).get(name))
              for key in targets]
    if any(value in ("missing", "stale") for value in states):
        return "stale"
    if any(value == "untracked" for value in states):
        return "untracked"
    return "synced"


def scan_sources() -> list[dict]:
    """扫描各平台已有 MCP，按名称合并成一条，并把平台差异自动抹平。

    同名但写法不同（Trae 的 url:null、ZCode 的 enabled/timeoutMs、
    CodeBuddy 的 disabled）会挑最完整的一份做底稿，再合并 env / headers。
    """
    groups: dict[str, dict] = {}
    for key, target in TARGET_MAP.items():
        try:
            found = servers(read_doc(target.mcp_file), target)
        except HubError:
            continue
        for name, config in found.items():
            if not isinstance(config, dict):
                continue
            group = groups.get(name)
            if group is None:
                group = {"id": name, "name": name, "configs": [],
                         "sources": [], "source_configs": {}}
                groups[name] = group
            group["configs"].append(config)
            group["source_configs"][key] = config
            if key not in group["sources"]:
                group["sources"].append(key)
    items = []
    for group in groups.values():
        merged, differences = merge_configs(group["configs"])
        if not merged or (not merged.get("command") and not merged.get("url")):
            continue
        item = {
            "id": group["name"],
            "name": group["name"],
            "config": merged,
            "sources": group["sources"],
            "source_configs": group["source_configs"],
            "variants": max(0, len(group["configs"]) - 1),
            "differences": differences,
        }
        item["source_label"] = "、".join(TARGET_MAP[key].label
                                        for key in item["sources"])
        item["summary"] = summary(merged)
        if differences:
            item["summary"] += f"（{len(differences)} 处差异，保留原配置）"
        items.append(item)
    items.sort(key=lambda item: item["name"].lower())
    return items


def import_items(items: list[dict], hub: dict) -> dict:
    imported: list[str] = []
    skipped: list[str] = []
    for item in items:
        name = str(item.get("name") or "").strip()
        config = item.get("config")
        if not name or not isinstance(config, dict):
            continue
        if name in hub["mcp"]:
            skipped.append(f"{name}（已在统一管理中）")
            continue
        config = normalize_config(config)
        if not config.get("command") and not config.get("url"):
            skipped.append(f"{name}（没有 command / url）")
            continue
        entry_item = entry(hub, name, config)
        entry_item["config"] = copy.deepcopy(config)
        entry_item["preserve"] = True
        entry_item["targets"] = [key for key in item.get("sources") or []
                                 if key in TARGET_MAP]
        # 来源平台保留自己原来的写法：记录原配置的哈希，避免自动同步把它们覆盖掉。
        # 只有新增平台才使用合并后的统一配置。
        source_configs = item.get("source_configs") or {}
        deployed = {}
        for key in entry_item["targets"]:
            actual = source_configs.get(key)
            if isinstance(actual, dict):
                deployed[key] = {"at": _now(), "hash": hash_entry(actual)}
            else:
                deployed[key] = {
                    "at": _now(),
                    "hash": hash_entry(entry_for(config, TARGET_MAP[key])),
                }
        entry_item["deployed"] = deployed
        imported.append(name)
    return {"imported": imported, "skipped": skipped}


def sync_all(hub: dict) -> dict:
    current_all, read_errors = read_all()
    changed = 0
    errors = [f"{TARGET_MAP[key].label}：{message}"
              for key, message in read_errors.items()]
    conflicts: list[str] = []
    for name, item in list(hub["mcp"].items()):
        if item.get("preserve"):
            # 导入时保留了各平台原配置，自动同步不要擅自统一
            continue
        keys = item.get("targets") or []
        if not keys:
            continue
        pending = []
        for key in keys:
            current = (current_all.get(key) or {}).get(name)
            value = state(item, name, key, current)
            if value in ("missing", "stale"):
                pending.append(key)
        if not pending:
            continue
        batch = deploy(name, item.get("config") or {}, pending, hub)
        changed += batch.ok_count
        conflicts.extend(f"{name} → {TARGET_MAP[step.target].label}"
                         for step in batch.conflicts if step.target in TARGET_MAP)
        errors.extend(f"{name} → {TARGET_MAP[step.target].label}：{step.message}"
                      for step in batch.failed if step.target in TARGET_MAP)
    return {"changed": changed, "errors": errors, "conflicts": conflicts}
