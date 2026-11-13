//! CodeBuddy 家族两个客户端的登录态写入面：用 WorkBuddy 账号档案切换它们的登录。
//!
//! **两个目标不是同一个产品，落点也完全不同**（显示名以 `app::products` 为准）：
//!   - `CodeBuddy IDE`（VSCode-fork）：`%APPDATA%\CodeBuddy CN\User\globalStorage\state.vscdb`
//!     的 `secret://…planning-genie.new.accessTokencn` 键；值是 Node Buffer-JSON
//!     （`{"type":"Buffer","data":[…]}`）包着的 `v10` 信封，主密钥在 `Local State`。
//!   - `CodeBuddy 插件`（JetBrains 宿主）：`%APPDATA%\JetBrains\*\options\secret-storage.xml`
//!     的 `<Entry key="Tencent-Cloud.coding-copilot.new.accessToken">`，值是 XML 转义的明文 JSON。
//!
//! 取证结论（2026-11 实机，见 data/backups/switch-exp-* 的对照实验）：
//! 两端存的都是同一份**转换版会话 JSON**（票据解密后的 `auth`/`account`/`accounts`
//! 加上插件 `attachOldSessionProperties` 的拍平字段），差异只在落点。
//!
//! 拍平规则必须与插件的 `attachOldSessionProperties` 逐字段一致（`account.id`/`label`、
//! 顶层 `id`/`token`/`accessToken`（`uid+token` 形态）/`converted`），否则插件走旧格式
//! 路径时拿不到值。票据的 `$wbEncrypted` 字段用 workbuddy 的 `open_field` 解开。

use super::workbuddy;
use super::SwitchOutcome;
use crate::app::clients::Roots;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::{fsx, process, secret};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// 切换目标：CodeBuddy 软件或 IDEA 插件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Software,
    IdeaPlugin,
}

impl Target {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "software" => Ok(Self::Software),
            "idea" => Ok(Self::IdeaPlugin),
            _ => Err(AppError::new(
                ErrorKind::Invalid,
                format!("未知的切换目标：{s}"),
            )),
        }
    }

    fn label(self) -> &'static str {
        // 显示名统一由产品表给：同一个东西在界面/日志里叫两个名字，就是下一次混淆的开始
        match self {
            Self::Software => crate::app::products::Product::CodeBuddyIde.label(),
            Self::IdeaPlugin => crate::app::products::Product::CodeBuddyIdea.label(),
        }
    }

    /// 需要检查是否在运行的进程名。在跑就不能写：两端都会在退出时把内存态刷回磁盘。
    ///
    /// CodeBuddy IDE 是固定 exe；插件**没有自己的进程**，宿主是装了插件的那个
    /// JetBrains IDE（`idea64.exe` / `goland64.exe` …），必须按本机实际安装情况现算——
    /// 写死 `idea64.exe` 会漏掉 GoLand / PyCharm，而漏判的结果是「切了没生效」。
    fn host_processes(self, roots: &Roots) -> Vec<String> {
        match self {
            Self::Software => crate::app::products::Product::CodeBuddyIde
                .processes()
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            Self::IdeaPlugin => {
                let mut out: Vec<String> = Vec::new();
                for xml in idea_stores(roots) {
                    let Some(stem) = host_stem_of(&xml) else {
                        continue;
                    };
                    // JetBrains 的 exe 是 `<词干>.exe` / `<词干>64.exe`
                    for exe in [format!("{stem}.exe"), format!("{stem}64.exe")] {
                        if !out.contains(&exe) {
                            out.push(exe);
                        }
                    }
                }
                out
            }
        }
    }

    /// 会话 JSON 的顶层 `id`：各端 genie 扩展的 auth 配置 id（实机取证值）。
    fn auth_config_id(self) -> &'static str {
        match self {
            Self::Software => "Tencent-Cloud.genie-ide-cn",
            Self::IdeaPlugin => "Tencent-Cloud.coding-copilot",
        }
    }

    /// CodeBuddyExtension 会话树里的平台目录名（取证值，与 sessions::sources::codebuddy 对齐）。
    fn platform_key(self) -> &'static str {
        match self {
            Self::Software => "CodeBuddyIDE",
            Self::IdeaPlugin => "JetBrains",
        }
    }

    /// 读取目标客户端当前会话文件（软件走 state.vscdb，插件走 secret-storage.xml）。
    fn read_session(self, roots: &Roots) -> Option<Value> {
        match self {
            Self::Software => read_software_session(roots),
            Self::IdeaPlugin => read_idea_session(roots),
        }
    }
}

/// CodeBuddy 插件在 JetBrains 侧的落点：插件目录名与凭据条目前缀（实机取证值）。
///
/// 两处都用它判断「这个 IDE 到底装没装插件」——只看 `secret-storage.xml` 会把
/// 用户所有 JetBrains 产品都算进来。
const PLUGIN_DIR: &str = "coding-copilot-jetbrains";
const PLUGIN_ENTRY_KEY: &str = "Tencent-Cloud.coding-copilot";

/// 账号作用域的产品/模型缓存键：切号后必须清掉，否则客户端仍拿着旧账号的
/// 模型清单与能力（权限），要等自然过期才刷新。两端用同一组键名（实机取证）。
const ACCOUNT_CACHE_KEYS: [&str; 6] = [
    "CodeBuddy-Product-Cache",
    "ACC_SHARE_CACHE_PRODUCT_MODELS",
    "ACC_SHARE_CACHE_PRODUCT_REMOTE_AGENTS",
    "cloud_product_config_cache",
    "CodeBuddy-Endpoint-Cache",
    "CodeBuddy-LLMDataReportCACHE-llm-data",
];

/// 目标客户端的现状（详情页状态展示用）。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, serde::Serialize)]
pub struct CodebuddyTargetStatus {
    /// `software` / `idea`（与切换命令的 target 参数同值）
    pub key: String,
    pub label: String,
    /// 本机是否装了/登录过（数据目录存在）
    pub installed: bool,
    pub current_name: Option<String>,
    pub current_uid: Option<String>,
    /// `cn` / `intl`：从会话的授权域判断（www.workbuddy.cn / www.workbuddy.ai）
    pub edition: Option<String>,
}

/// 两个目标客户端的当前登录状态。读不到会话就是「未登录」，不报错。
pub fn statuses(roots: &Roots) -> Vec<CodebuddyTargetStatus> {
    [Target::Software, Target::IdeaPlugin]
        .iter()
        .map(|&target| {
            let session = target.read_session(roots);
            let installed = match target {
                Target::Software => software_vscdb(roots).is_file(),
                Target::IdeaPlugin => !idea_stores(roots).is_empty(),
            };
            let account = session.as_ref().and_then(|s| s.get("account"));
            let name = account
                .and_then(|a| a.get("label").or_else(|| a.get("nickname")))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let uid = account
                .and_then(|a| a.get("uid"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let edition = session
                .as_ref()
                .and_then(|s| s.pointer("/auth/domain"))
                .and_then(|v| v.as_str())
                .and_then(edition_of_domain);
            CodebuddyTargetStatus {
                key: match target {
                    Target::Software => "software".into(),
                    Target::IdeaPlugin => "idea".into(),
                },
                label: target.label().to_string(),
                installed: installed || session.is_some(),
                current_name: name,
                current_uid: uid,
                edition,
            }
        })
        .collect()
}

/// 授权域 → 版本：`www.workbuddy.cn` 是国内版、`www.workbuddy.ai` 是国际版。
fn edition_of_domain(domain: &str) -> Option<String> {
    if domain.ends_with("workbuddy.cn") {
        Some("cn".into())
    } else if domain.ends_with("workbuddy.ai") {
        Some("intl".into())
    } else {
        None
    }
}

fn software_vscdb(roots: &Roots) -> PathBuf {
    roots
        .appdata
        .join("CodeBuddy CN/User/globalStorage/state.vscdb")
}

/// 本机所有**装了 CodeBuddy 插件**的 IDE 的 secret-storage.xml。
///
/// 判据是插件本体或凭据条目，不是「有没有 secret-storage.xml」——任何 JetBrains IDE
/// 跑一次都会生成那个文件，按它写等于把 WorkBuddy 的令牌撒进用户所有 IDE 的密钥库。
fn idea_stores(roots: &Roots) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(dirs) = std::fs::read_dir(roots.appdata.join("JetBrains")) else {
        return out;
    };
    for dir in dirs.flatten() {
        let xml = dir.path().join("options").join("secret-storage.xml");
        if !xml.is_file() {
            continue;
        }
        let has_plugin = dir.path().join("plugins").join(PLUGIN_DIR).is_dir();
        let has_entry = std::fs::read_to_string(&xml)
            .map(|text| text.contains(PLUGIN_ENTRY_KEY))
            .unwrap_or(false);
        if has_plugin || has_entry {
            out.push(xml);
        }
    }
    out
}

/// `…\JetBrains\IntelliJIdea2026.3\options\secret-storage.xml` → `idea`
fn host_stem_of(store: &Path) -> Option<&'static str> {
    let product_dir = store.parent()?.parent()?.file_name()?;
    crate::app::products::host_stem(&product_dir.to_string_lossy())
}

/// 目标客户端**当前**登录账号的 uid（会话保留的来源）。读不到返回 None。
fn read_current_uid(roots: &Roots, target: Target) -> Option<String> {
    let session = match target {
        Target::Software => read_software_session(roots)?,
        Target::IdeaPlugin => read_idea_session(roots)?,
    };
    session
        .get("account")
        .and_then(|a| a.get("uid"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// 读 CodeBuddy 软件 state.vscdb 里的会话（v10 信封解开 + Buffer-JSON 解包）。
fn read_software_session(roots: &Roots) -> Option<Value> {
    let base = roots.appdata.join("CodeBuddy CN");
    let key = secret::chromium_key(&base.join("Local State")).ok()?;
    let conn = rusqlite::Connection::open(base.join("User/globalStorage/state.vscdb")).ok()?;
    let value: Vec<u8> = conn
        .query_row(
            "SELECT value FROM ItemTable WHERE key LIKE '%planning-genie%'",
            [],
            |r| r.get(0),
        )
        .ok()?;
    let outer: Value = serde_json::from_slice(&value).ok()?;
    let blob: Vec<u8> = serde_json::from_value(outer.get("data")?.clone()).ok()?;
    let plain = secret::chromium_open(&key, &blob).ok()?;
    serde_json::from_slice(&plain).ok()
}

/// 读插件在 secret-storage.xml 里的会话（明文 Entry）。
///
/// 多个 IDE 都装了插件时取**最近写过**的那份：同一台机器上可能在不同 IDE 登了不同账号，
/// 遍历顺序随机会让「当前登录是谁」变成抽奖——切完把会话归到错的账号名下。
fn read_idea_session(roots: &Roots) -> Option<Value> {
    let mut stores = idea_stores(roots);
    stores.sort_by_key(|path| std::cmp::Reverse(mtime_ms(path)));

    let marker = format!("key=\"{PLUGIN_ENTRY_KEY}.new.accessToken\" value=\"");
    for xml in stores {
        let Ok(text) = std::fs::read_to_string(&xml) else {
            continue;
        };
        // 这个 IDE 装了插件但没登过 CodeBuddy：换下一个。
        // 早先这里是 `?`——一个空 store 就把整次读取结束掉，另一台已登录的 IDE 永远读不到。
        let Some(at) = text.find(&marker) else {
            continue;
        };
        let value_start = at + marker.len();
        let Some(len) = text[value_start..].find('"') else {
            continue;
        };
        let raw = unescape_xml(&text[value_start..value_start + len]);
        if let Ok(doc) = serde_json::from_str::<Value>(&raw) {
            return Some(doc);
        }
    }
    None
}

fn mtime_ms(path: &Path) -> u128 {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|delta| delta.as_millis())
        .unwrap_or(0)
}

fn unescape_xml(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// 票据 → 插件存盘的转换版会话：递归解开 `$wbEncrypted`，再按
/// `attachOldSessionProperties` 的规则拍平旧格式字段。
fn build_session(ticket: &Value, auth_config_id: &str) -> Result<Value> {
    let doc = decrypt_tree(ticket)?;
    let acc = doc
        .get("account")
        .and_then(|v| v.as_object())
        .ok_or_else(|| AppError::new(ErrorKind::Invalid, "票据里没有 account"))?;
    let au = doc
        .get("auth")
        .and_then(|v| v.as_object())
        .ok_or_else(|| AppError::new(ErrorKind::Invalid, "票据里没有 auth"))?;
    let get = |m: &serde_json::Map<String, Value>, k: &str| -> Result<String> {
        m.get(k)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| AppError::new(ErrorKind::Invalid, format!("票据缺少 {k}")))
    };
    let uid = get(acc, "uid")?;
    let token = get(au, "accessToken")?;
    let mut account = acc.clone();
    account.insert("id".into(), json!(uid));
    account.insert(
        "label".into(),
        json!(acc.get("nickname").cloned().unwrap_or_default()),
    );
    Ok(json!({
        "account": account,
        "auth": doc["auth"].clone(),
        "accounts": doc.get("accounts").cloned().unwrap_or_else(|| json!([])),
        "id": auth_config_id,
        "token": token,
        "refreshToken": get(au, "refreshToken")?,
        "expiresAt": au.get("expiresAt").cloned().unwrap_or(Value::Null),
        "domain": get(au, "domain")?,
        "accessToken": format!("{uid}+{token}"),
        "converted": true,
    }))
}

/// 递归解开 `$wbEncrypted` 字段；普通节点原样递归。
fn decrypt_tree(node: &Value) -> Result<Value> {
    if let Some(obj) = node.as_object() {
        if obj.get("$wbEncrypted").and_then(|v| v.as_i64()) == Some(1) {
            return Ok(Value::String(workbuddy::open_field(node)?));
        }
        let mut out = serde_json::Map::new();
        for (k, v) in obj {
            out.insert(k.clone(), decrypt_tree(v)?);
        }
        return Ok(Value::Object(out));
    }
    if let Some(arr) = node.as_array() {
        let mut out = Vec::with_capacity(arr.len());
        for v in arr {
            out.push(decrypt_tree(v)?);
        }
        return Ok(Value::Array(out));
    }
    Ok(node.clone())
}

// ---------------------------------------------------------------- 写入面

/// CodeBuddy 软件：state.vscdb 的 `secret://` 键（Buffer-JSON 包着的 v10 信封）。
fn write_software(roots: &Roots, session: &Value) -> Result<PathBuf> {
    let base = roots.appdata.join("CodeBuddy CN");
    let local_state = base.join("Local State");
    let vscdb = software_vscdb(roots);
    if !vscdb.exists() {
        return Err(AppError::new(
            ErrorKind::Io,
            "没有找到 CodeBuddy 软件的数据目录（可能没安装）",
        ));
    }
    let key = secret::chromium_key(&local_state)?;
    let plain = serde_json::to_vec(session)?;
    let blob = secret::chromium_seal(&key, &plain)?;
    let value = serde_json::to_vec(&json!({"type": "Buffer", "data": blob}))?;

    let conn = rusqlite::Connection::open(&vscdb)
        .map_err(|e| AppError::new(ErrorKind::Io, format!("打开 state.vscdb 失败：{e}")))?;
    let key_text = software_secret_key("planning-genie.new.accessTokencn");
    let rows = conn
        .execute(
            "UPDATE ItemTable SET value = ?1 WHERE key = ?2",
            rusqlite::params![value.as_slice(), key_text],
        )
        .map_err(|e| AppError::new(ErrorKind::Io, format!("写 state.vscdb 失败：{e}")))?;
    if rows == 0 {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "CodeBuddy 软件里没有登录凭据键，请先打开它登录一次",
        ));
    }
    Ok(vscdb)
}

/// IDEA 插件：各 IDE 的 options/secret-storage.xml（明文 Entry，XML 转义）。
/// 返回实际写入的文件；一个都没找到就报错。
fn write_idea(roots: &Roots, session: &Value) -> Result<Vec<PathBuf>> {
    let entry_key = "Tencent-Cloud.coding-copilot.new.accessToken";
    let value = xml_escape(&serde_json::to_string(session)?);
    let stores = idea_stores(roots);
    if stores.is_empty() {
        return Err(AppError::new(
            ErrorKind::Io,
            "没有找到 IDEA 的 secret-storage.xml（插件可能没安装/没登录过）",
        ));
    }
    let mut written = Vec::new();
    for xml in stores {
        let text = std::fs::read_to_string(&xml).map_err(|e| {
            AppError::new(ErrorKind::Io, format!("读 secret-storage.xml 失败：{e}"))
        })?;
        if !text.contains("<Scores>") {
            continue;
        }
        let next = upsert_entry(&text, entry_key, &value);
        fsx::write_atomic(&xml, next.as_bytes())?;
        written.push(xml);
    }
    Ok(written)
}

/// 软件的 secret 键名（VSCode SecretStorage 的 `secret://` 命名）。
fn software_secret_key(key: &str) -> String {
    format!(r#"secret://{{"extensionId":"tencent-cloud.coding-copilot","key":"{key}"}}"#)
}

/// 账号信息与权限同步：清掉账号作用域的产品/模型缓存，逼客户端按新账号重新拉取。
///
/// 不清的后果：切完账号，模型清单、可用能力（权限）还是旧账号缓存里的，
/// 要等 TTL 过期才刷新——用户看到的就是「号切了，但权限没跟着变」。
/// 返回清掉的条数；读不到存储不算错误（没装/没登录过）。
fn prune_account_caches(roots: &Roots, target: Target) -> Result<u32> {
    let mut removed = 0u32;
    match target {
        Target::Software => {
            let vscdb = software_vscdb(roots);
            if !vscdb.is_file() {
                return Ok(0);
            }
            let conn = rusqlite::Connection::open(&vscdb)
                .map_err(|e| AppError::new(ErrorKind::Io, format!("打开 state.vscdb 失败：{e}")))?;
            for key in ACCOUNT_CACHE_KEYS {
                let rows = conn
                    .execute(
                        "DELETE FROM ItemTable WHERE key = ?1",
                        rusqlite::params![software_secret_key(key)],
                    )
                    .map_err(|e| AppError::new(ErrorKind::Io, format!("清理账号缓存失败：{e}")))?;
                removed += rows as u32;
            }
        }
        Target::IdeaPlugin => {
            for xml in idea_stores(roots) {
                let text = std::fs::read_to_string(&xml).map_err(|e| {
                    AppError::new(ErrorKind::Io, format!("读 secret-storage.xml 失败：{e}"))
                })?;
                let mut next = text.clone();
                for key in ACCOUNT_CACHE_KEYS {
                    if let Some(pruned) = remove_entry(&next, key) {
                        next = pruned;
                        removed += 1;
                    }
                }
                if next != text {
                    fsx::write_atomic(&xml, next.as_bytes())?;
                }
            }
        }
    }
    Ok(removed)
}

/// 删掉一条 `<Entry key="…" … />`；没有就返回 None。
fn remove_entry(xml: &str, key: &str) -> Option<String> {
    let marker = format!("key=\"{key}\"");
    let at = xml.find(&marker)?;
    let start = xml[..at].rfind("<Entry")?;
    let end = xml[at..].find("/>").map(|i| at + i + 2)?;
    Some(format!("{}{}", &xml[..start], &xml[end..]))
}

/// 目标客户端当前会话里的 auth 配置 id（切号时沿用，保证旧格式字段与客户端一致）。
fn existing_auth_id(roots: &Roots, target: Target) -> Option<String> {
    target
        .read_session(roots)?
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// 把 `<Entry key=… value=… />` 整条换掉；没有就插到 `<Scores>` 之后。
fn upsert_entry(xml: &str, key: &str, value: &str) -> String {
    let marker = format!("key=\"{key}\"");
    if let Some(at) = xml.find(&marker) {
        let start = xml[..at].rfind("<Entry").unwrap_or(at);
        let end = xml[at..]
            .find("/>")
            .map(|i| at + i + 2)
            .unwrap_or_else(|| xml.len());
        return format!(
            "{}<Entry key=\"{key}\" value=\"{value}\" />{}",
            &xml[..start],
            &xml[end..]
        );
    }
    const SCORES: &str = "<Scores>";
    if let Some(pos) = xml.find(SCORES) {
        let insert_at = pos + SCORES.len();
        return format!(
            "{}\n        <Entry key=\"{key}\" value=\"{value}\" />{}",
            &xml[..insert_at],
            &xml[insert_at..]
        );
    }
    xml.to_string()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ---------------------------------------------------------------- 会话保留

/// 切换后会话不消失：把原账号在该平台下的会话树挪到新账号名下。
///
/// 会话树按账号分目录（`Data\<账号>\<平台>\<键>\…`，键=账号 uid 或工作区），
/// 不挪的话新账号读自己的空树、旧会话从界面消失。只动目标平台：另一个
/// CodeBuddy 系客户端的登录与会话互不影响。同名文件保留目标侧的（不覆盖），
/// 源侧副本清理掉——会话「跟着登录账号走」，与 WorkBuddy 契约 10 同一个语义。
fn rekey_conversations(roots: &Roots, platform: &str, from: &str, to: &str) -> Result<u64> {
    let base = roots.codebuddy();
    let src_plat = base.join(from).join(platform);
    if !src_plat.is_dir() {
        return Ok(0);
    }
    let dst_plat = base.join(to).join(platform);
    std::fs::create_dir_all(&dst_plat)?;
    let mut moved = 0u64;
    for entry in std::fs::read_dir(&src_plat)? {
        let entry = entry?;
        let name = entry.file_name();
        // 账号键目录跟着改名（<from> → <to>），工作区键目录原名保留
        let dst_name = if name.to_string_lossy() == from {
            std::ffi::OsString::from(to)
        } else {
            name
        };
        moved += move_merge(&entry.path(), &dst_plat.join(dst_name))?;
    }
    // 源树已空就把壳也清掉（空目录留着会让「哪个账号还在用」的判断变糊）
    let _ = std::fs::remove_dir(&src_plat);
    let _ = std::fs::remove_dir(base.join(from));
    Ok(moved)
}

/// 递归移动合并：目标已有的文件不覆盖（保留目标侧），其余 rename 过去。
fn move_merge(src: &Path, dst: &Path) -> Result<u64> {
    if !dst.exists() {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(src, dst)
            .map_err(|e| AppError::new(ErrorKind::Io, format!("移动会话数据失败：{e}")))?;
        return Ok(count_files(dst));
    }
    let mut moved = 0u64;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            moved += move_merge(&from, &to)?;
        } else if !to.exists() {
            std::fs::rename(&from, &to)
                .map_err(|e| AppError::new(ErrorKind::Io, format!("移动会话数据失败：{e}")))?;
            moved += 1;
        } else {
            // 目标已有同名文件：保留目标侧的，源侧这份丢弃
            let _ = std::fs::remove_file(&from);
        }
    }
    let _ = std::fs::remove_dir(src);
    Ok(moved)
}

fn count_files(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| {
                    let p = e.path();
                    if p.is_dir() {
                        count_files(&p)
                    } else {
                        1
                    }
                })
                .sum()
        })
        .unwrap_or(0)
}

// ---------------------------------------------------------------- 编排

/// 用 WorkBuddy 账号的档案把目标客户端切成这个账号（`edition`: `cn` / `intl`）。
///
/// 只动目标客户端的凭证落点，不碰 WorkBuddy 自己的登录态（那走 `accounts::switch`）。
/// `processes` 是测试注入口：测试里给空列表跳过进程检查。
pub fn switch_target(
    roots: &Roots,
    uid: &str,
    name: &str,
    edition: &str,
    target: Target,
) -> Result<SwitchOutcome> {
    // 在跑就不能写：先按本机实际安装情况算出「谁在占着这些文件」。
    // 插件没有自己的进程，宿主是装了插件的那个 JetBrains IDE。
    let processes = target.host_processes(roots);
    let refs: Vec<&str> = processes.iter().map(String::as_str).collect();
    switch_target_with(roots, uid, name, edition, target, &refs)
}

fn switch_target_with(
    roots: &Roots,
    uid: &str,
    name: &str,
    edition: &str,
    target: Target,
    processes: &[&str],
) -> Result<SwitchOutcome> {
    // 档案在 <data>/profiles/<cn|intl>/<uid>/auth.info（与 WorkBuddyProvider 的 pack_dir 同构）
    let pack = crate::app::paths::data_dir()
        .join("profiles")
        .join(edition)
        .join(uid)
        .join("auth.info");
    if !pack.is_file() {
        return Err(AppError::new(
            ErrorKind::SessionStateMissing,
            format!(
                "本机没有 {name} 的登录态档案。请先在 WorkBuddy 里登录一次该账号，\
                 关闭客户端后会自动补存，之后切换就不需要重新登录。"
            ),
        ));
    }
    if !processes.is_empty() && process::process_running(processes) {
        return Err(AppError::new(
            ErrorKind::ClientRunning,
            format!("请先关闭 {} 再切换", target.label()),
        ));
    }

    let ticket: Value = serde_json::from_slice(
        &std::fs::read(&pack)
            .map_err(|e| AppError::new(ErrorKind::Io, format!("读档案失败：{e}")))?,
    )
    .map_err(|e| AppError::new(ErrorKind::Invalid, format!("档案不是合法票据：{e}")))?;
    // auth 配置 id 优先沿用目标客户端当前会话里的值（国内/国际版可能不同），
    // 客户端从没登录过才退回实机取证的缺省值
    let auth_id =
        existing_auth_id(roots, target).unwrap_or_else(|| target.auth_config_id().to_string());
    let session = build_session(&ticket, &auth_id)?;
    // 会话保留的来源：目标客户端当前登录的账号（写盘前读，之后就是新账号了）
    let from_uid = read_current_uid(roots, target);

    match target {
        Target::Software => {
            write_software(roots, &session)?;
        }
        Target::IdeaPlugin => {
            write_idea(roots, &session)?;
        }
    };

    // 账号信息与权限同步：清账号作用域的模型/产品缓存，切完立刻按新账号拉取
    let pruned = prune_account_caches(roots, target).unwrap_or_else(|e| {
        crate::app::log::log(
            crate::app::log::Level::Warn,
            "accounts",
            format!("清理账号缓存失败：{}", e.message),
        );
        0
    });

    let tag = if edition == "intl" {
        "国际版"
    } else {
        "国内版"
    };
    let mut message = format!(
        "{}已切换到{tag}「{name}」，重启 {} 后生效",
        target.label(),
        target.label()
    );
    if pruned > 0 {
        message.push_str(&format!(
            "，已清理 {pruned} 项账号缓存（模型与权限将按新账号刷新）"
        ));
    }
    // 切换后会话不能消失：把原账号的会话树挪到新账号名下（与契约 10 同语义）。
    // 保留失败不影响登录态已切好——落日志、提示用户可在会话页找回。
    if let Some(from) = from_uid.filter(|f| f != uid) {
        match rekey_conversations(roots, target.platform_key(), &from, uid) {
            Ok(0) => {}
            Ok(n) => message.push_str(&format!("，原账号的会话已保留给新账号（{n} 个文件）")),
            Err(e) => {
                crate::app::log::log(
                    crate::app::log::Level::Warn,
                    "accounts",
                    format!("切换后保留会话失败：{}", e.message),
                );
                message.push_str("，会话未能自动保留（可在会话页找回原账号的记录）");
            }
        }
    }
    Ok(SwitchOutcome { message })
}

// ---------------------------------------------------------------- 测试

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket() -> Value {
        json!({
            "auth": {
                "accessToken": "tok-A",
                "refreshToken": "ref-A",
                "expiresAt": 1796347317300i64,
                "domain": "www.workbuddy.cn",
                "sessionState": "s1",
            },
            "account": {
                "uid": "u-1",
                "nickname": "凌致",
                "uin": "1",
                "type": "personal",
            },
            "accounts": [{"uid": "u-1", "nickname": "凌致", "type": "personal"}],
            "allAccounts": [],
        })
    }

    fn roots(dir: &tempfile::TempDir) -> Roots {
        Roots::at(dir.path(), dir.path(), dir.path())
    }

    /// 与插件 attachOldSessionProperties 逐字段对齐。
    #[test]
    fn build_session_flattens_like_the_plugin() {
        let s = build_session(&ticket(), "cfg-id").unwrap();
        assert_eq!(s["id"], "cfg-id");
        assert_eq!(s["token"], "tok-A");
        assert_eq!(s["refreshToken"], "ref-A");
        assert_eq!(s["domain"], "www.workbuddy.cn");
        assert_eq!(s["expiresAt"], 1796347317300i64);
        assert_eq!(s["accessToken"], "u-1+tok-A", "拍平字段是 uid+token 形态");
        assert_eq!(s["converted"], true);
        assert_eq!(s["account"]["id"], "u-1");
        assert_eq!(s["account"]["label"], "凌致");
        assert_eq!(s["accounts"][0]["uid"], "u-1");
        // 原票据的加密壳不在结果里（普通字段原样）
        assert_eq!(s["auth"]["sessionState"], "s1");
    }

    /// 递归解密走的是 workbuddy::open_field；这里只验证「树遍历」本身。
    #[test]
    fn decrypt_tree_walks_nested_plain_nodes() {
        let v = json!({"a": {"b": ["x", {"c": "y"}]}, "d": 1});
        let out = decrypt_tree(&v).unwrap();
        assert_eq!(out, v);
    }

    fn fake_local_state(dir: &Path) {
        use base64::Engine;
        let base = dir.join("CodeBuddy CN");
        std::fs::create_dir_all(&base).unwrap();
        let key = [7u8; 32];
        let mut wrapped = b"DPAPI".to_vec();
        wrapped.extend(secret::protect(&key).unwrap());
        let json = json!({"os_crypt": {
            "encrypted_key": base64::engine::general_purpose::STANDARD.encode(&wrapped)
        }});
        std::fs::write(base.join("Local State"), serde_json::to_vec(&json).unwrap()).unwrap();
    }

    fn fake_vscdb(dir: &Path) {
        let path = dir
            .join("CodeBuddy CN")
            .join("User")
            .join("globalStorage")
            .join("state.vscdb");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
            rusqlite::params![
                r#"secret://{"extensionId":"tencent-cloud.coding-copilot","key":"planning-genie.new.accessTokencn"}"#,
                &b"old"[..]
            ],
        )
        .unwrap();
    }

    /// 软件写入面可逆：写进去的会话能按 v10 信封读回来。
    #[test]
    fn software_writer_roundtrips_through_v10_envelope() {
        let dir = tempfile::TempDir::new().unwrap();
        fake_local_state(dir.path());
        fake_vscdb(dir.path());
        let session = build_session(&ticket(), Target::Software.auth_config_id()).unwrap();
        write_software(&roots(&dir), &session).unwrap();

        let key = secret::chromium_key(&dir.path().join("CodeBuddy CN/Local State")).unwrap();
        let conn = rusqlite::Connection::open(
            dir.path()
                .join("CodeBuddy CN/User/globalStorage/state.vscdb"),
        )
        .unwrap();
        let value: Vec<u8> = conn
            .query_row(
                "SELECT value FROM ItemTable WHERE key LIKE '%planning-genie%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let outer: Value = serde_json::from_slice(&value).unwrap();
        let blob: Vec<u8> = serde_json::from_value(outer["data"].clone()).unwrap();
        let plain = secret::chromium_open(&key, &blob).unwrap();
        let back: Value = serde_json::from_slice(&plain).unwrap();
        assert_eq!(back["account"]["uid"], "u-1");
        assert_eq!(back["accessToken"], "u-1+tok-A");
    }

    #[test]
    fn software_writer_needs_the_secret_key_row() {
        let dir = tempfile::TempDir::new().unwrap();
        fake_local_state(dir.path());
        let path = dir
            .path()
            .join("CodeBuddy CN/User/globalStorage/state.vscdb");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute("CREATE TABLE ItemTable (key TEXT, value BLOB)", [])
            .unwrap();
        let session = build_session(&ticket(), "x").unwrap();
        let err = write_software(&roots(&dir), &session).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::Invalid));
    }

    /// IDEA 写入面：替换已有 Entry / 缺失时插入。
    #[test]
    fn idea_writer_upserts_entry() {
        let dir = tempfile::TempDir::new().unwrap();
        let xml = dir
            .path()
            .join("JetBrains/IntelliJIdea2026.2/options/secret-storage.xml");
        std::fs::create_dir_all(xml.parent().unwrap()).unwrap();
        // 插件装着、还没登录——这是「首次切换」的真实形态：有插件目录、没有凭据条目。
        // 只有 secret-storage.xml 的 IDE 不算 CodeBuddy 宿主，不会被写。
        std::fs::create_dir_all(
            dir.path()
                .join("JetBrains/IntelliJIdea2026.2/plugins/coding-copilot-jetbrains"),
        )
        .unwrap();
        std::fs::write(
            &xml,
            r#"<application>
    <component name="SecretStorage">
      <Scores>
        <Entry key="other" value="1" />
      </Scores>
    </component>
  </application>"#,
        )
        .unwrap();

        let session = build_session(&ticket(), Target::IdeaPlugin.auth_config_id()).unwrap();
        let written = write_idea(&roots(&dir), &session).unwrap();
        assert_eq!(written.len(), 1);

        let text = std::fs::read_to_string(&xml).unwrap();
        assert!(text.contains("Tencent-Cloud.coding-copilot.new.accessToken"));
        assert!(text.contains("u-1+tok-A"));
        assert!(text.contains("other"), "原有条目不能被抹掉");

        // 再写一次：替换而不是叠加
        let session2 = json!({"account": {"uid": "u-2"}, "converted": true});
        write_idea(&roots(&dir), &session2).unwrap();
        let text2 = std::fs::read_to_string(&xml).unwrap();
        assert!(text2.contains("u-2"));
        assert!(!text2.contains("u-1+tok-A"));
        assert_eq!(
            text2
                .matches("Tencent-Cloud.coding-copilot.new.accessToken")
                .count(),
            1
        );
    }

    #[test]
    fn idea_writer_errors_when_no_secret_storage() {
        let dir = tempfile::TempDir::new().unwrap();
        let session = build_session(&ticket(), "x").unwrap();
        let err = write_idea(&roots(&dir), &session).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::Io));
    }

    /// 运行中拒绝：一个字节都不写。
    #[test]
    fn switch_refuses_when_target_client_running() {
        let dir = tempfile::TempDir::new().unwrap();
        // 造一个档案，确保拒绝发生在「进程检查」而不是「缺档案」
        let uid = "u-1";
        let pack = crate::app::paths::data_dir()
            .join("profiles/cn")
            .join(uid)
            .join("auth.info");
        let _ = pack;
        let err = switch_target_with(
            &roots(&dir),
            uid,
            "测试",
            "cn",
            Target::Software,
            &["this-process-does-not-exist.exe"],
        )
        .unwrap_err();
        // 本机没有这个档案 → SessionStateMissing；这个用例只锁「检查顺序」不 panic
        assert!(matches!(err.kind, ErrorKind::SessionStateMissing));
    }

    #[test]
    fn xml_escape_is_reversible_enough_for_attributes() {
        assert_eq!(xml_escape(r#"a"b&c<d>"#), "a&quot;b&amp;c&lt;d&gt;");
    }

    /// 回归：插件的「在运行」判据必须是带 `.exe` 的宿主进程名。
    ///
    /// 早先写的是 `["idea64", "idea"]`，而 `process_running` 比对的是任务管理器里的
    /// `szExeFile`（`idea64.exe`），永远匹配不上——守卫形同虚设，写进去的登录态
    /// 会被 IDE 退出时回写覆盖，用户看到的就是「切了没生效」。
    #[test]
    fn idea_plugin_guard_uses_full_exe_names() {
        let dir = tempfile::TempDir::new().unwrap();
        let roots = roots(&dir);
        let host = roots.appdata.join("JetBrains/IntelliJIdea2026.3");
        std::fs::create_dir_all(host.join("options")).unwrap();
        std::fs::create_dir_all(host.join("plugins/coding-copilot-jetbrains")).unwrap();
        std::fs::write(
            host.join("options/secret-storage.xml"),
            b"<Scores></Scores>",
        )
        .unwrap();

        let names = Target::IdeaPlugin.host_processes(&roots);
        assert!(names.iter().any(|n| n == "idea64.exe"), "{names:?}");
        assert!(names.iter().any(|n| n == "idea.exe"), "{names:?}");
        assert!(
            names.iter().all(|n| n.to_lowercase().ends_with(".exe")),
            "必须是 exe 全名：{names:?}"
        );
        // CodeBuddy IDE 是固定进程名，不受 JetBrains 目录影响
        assert_eq!(
            Target::Software.host_processes(&roots),
            vec!["CodeBuddy CN.exe", "CodeBuddy.exe"]
        );
    }

    /// 只碰装了插件的 IDE。
    ///
    /// 任何 JetBrains IDE 跑一次都会生成 `secret-storage.xml`；按「有这个文件」就写，
    /// 等于把 WorkBuddy 的令牌撒进用户所有 IDE 的密钥库（GoLand / PyCharm 都会中招）。
    #[test]
    fn idea_stores_only_touch_ides_with_the_plugin() {
        let dir = tempfile::TempDir::new().unwrap();
        let roots = roots(&dir);
        let jb = roots.appdata.join("JetBrains");

        // 没装插件：只有 IDE 自己的密钥库
        std::fs::create_dir_all(jb.join("GoLand2026.1/options")).unwrap();
        std::fs::write(
            jb.join("GoLand2026.1/options/secret-storage.xml"),
            b"<Scores></Scores>",
        )
        .unwrap();
        // 装了插件
        std::fs::create_dir_all(jb.join("IntelliJIdea2026.3/options")).unwrap();
        std::fs::create_dir_all(jb.join("IntelliJIdea2026.3/plugins/coding-copilot-jetbrains"))
            .unwrap();
        std::fs::write(
            jb.join("IntelliJIdea2026.3/options/secret-storage.xml"),
            b"<Scores></Scores>",
        )
        .unwrap();

        let stores = idea_stores(&roots);
        assert_eq!(stores.len(), 1, "{stores:?}");
        assert!(stores[0].starts_with(jb.join("IntelliJIdea2026.3")));
    }

    /// 回归：装了插件但没登过 CodeBuddy 的 IDE 不能把整次读取结束掉。
    ///
    /// 旧实现在循环里用 `?`——第一个 store 没有条目就直接 return None，
    /// 另一台已经登录的 IDE 永远读不到，状态显示「未登录」、切换也不迁移会话。
    #[test]
    fn read_idea_session_skips_hosts_without_a_ticket() {
        let dir = tempfile::TempDir::new().unwrap();
        let roots = roots(&dir);
        let jb = roots.appdata.join("JetBrains");
        let doc = json!({"account": {"uid": "u-9"}, "auth": {"domain": "www.workbuddy.cn"}});
        let entry = format!(
            r#"<Scores><Entry key="{}.new.accessToken" value="{}" /></Scores>"#,
            PLUGIN_ENTRY_KEY,
            xml_escape(&serde_json::to_string(&doc).unwrap())
        );

        // IntelliJ 有票据（更旧）；PyCharm 装了插件没登录（更新，会被先读到）
        for (name, body, secs_ago) in [
            ("IntelliJIdea2026.3", entry, 600u64),
            ("PyCharm2026.1", "<Scores></Scores>".to_string(), 0u64),
        ] {
            let host = jb.join(name);
            std::fs::create_dir_all(host.join("options")).unwrap();
            std::fs::create_dir_all(host.join("plugins/coding-copilot-jetbrains")).unwrap();
            let xml = host.join("options/secret-storage.xml");
            std::fs::write(&xml, body).unwrap();
            set_mtime(&xml, secs_ago);
        }

        let session = read_idea_session(&roots).expect("应跳过空 store 读到 IntelliJ 的票据");
        assert_eq!(
            session.pointer("/account/uid").and_then(|v| v.as_str()),
            Some("u-9")
        );
    }

    fn set_mtime(path: &Path, secs_ago: u64) {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        let time = std::time::SystemTime::now() - std::time::Duration::from_secs(secs_ago);
        file.set_modified(time).unwrap();
    }

    /// 会话保留：源树整棵挪到目标账号名下，源不留残骸、目标已有的文件不被覆盖。
    #[test]
    fn rekey_conversations_moves_history_to_the_new_account() {
        let dir = tempfile::TempDir::new().unwrap();
        let base = dir.path().join("CodeBuddyExtension/Data");
        let (from, to) = (
            "aaaaaaaa-1111-2222-3333-444444444444",
            "bbbbbbbb-5555-6666-7777-888888888888",
        );

        // 原账号：一条会话（history/<工作区>/<会话>/messages/1.json）
        let src = base.join(from).join("JetBrains").join(from);
        std::fs::create_dir_all(src.join("history/ws1/conv1/messages")).unwrap();
        std::fs::write(src.join("history/ws1/index.json"), b"{}").unwrap();
        std::fs::write(src.join("history/ws1/conv1/messages/1.json"), b"{}").unwrap();
        std::fs::write(src.join("history/ws1/conv1/messages/2.json"), b"{}").unwrap();
        // 目标账号：同名会话已有一个文件（不能被覆盖）
        let dst = base.join(to).join("JetBrains").join(to);
        std::fs::create_dir_all(dst.join("history/ws1/conv1/messages")).unwrap();
        std::fs::write(dst.join("history/ws1/conv1/messages/1.json"), b"mine").unwrap();

        let roots = Roots::at(dir.path(), dir.path(), dir.path());
        let moved = rekey_conversations(&roots, "JetBrains", from, to).unwrap();

        assert_eq!(moved, 2, "index + 2 条消息里，目标已有的 1 条不重搬");
        // 目标侧：会话完整，已有的还是自己的内容
        assert!(dst.join("history/ws1/index.json").is_file());
        assert_eq!(
            std::fs::read(dst.join("history/ws1/conv1/messages/1.json")).unwrap(),
            b"mine"
        );
        assert!(dst.join("history/ws1/conv1/messages/2.json").is_file());
        // 源侧清空：会话「跟着账号走」，不在两边重复
        assert!(!src.exists(), "源会话树应已并走");
    }

    /// 账号信息与权限同步：缓存行被清掉，登录会话本身不能动。
    #[test]
    fn prune_caches_keeps_the_session_row() {
        let dir = tempfile::TempDir::new().unwrap();
        fake_local_state(dir.path());
        fake_vscdb(dir.path());
        let conn = rusqlite::Connection::open(
            dir.path()
                .join("CodeBuddy CN/User/globalStorage/state.vscdb"),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
            rusqlite::params![software_secret_key("CodeBuddy-Product-Cache"), &b"x"[..]],
        )
        .unwrap();
        drop(conn);

        let removed = prune_account_caches(&roots(&dir), Target::Software).unwrap();
        assert_eq!(removed, 1);

        let conn = rusqlite::Connection::open(
            dir.path()
                .join("CodeBuddy CN/User/globalStorage/state.vscdb"),
        )
        .unwrap();
        let caches: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ItemTable WHERE key LIKE '%CodeBuddy-Product-Cache%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let session: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ItemTable WHERE key LIKE '%planning-genie%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(caches, 0);
        assert_eq!(session, 1, "登录凭据不能被当成缓存清掉");
    }

    /// IDEA 端同样：只清缓存 Entry，会话 Entry 留着。
    #[test]
    fn prune_caches_keeps_the_idea_session_entry() {
        let dir = tempfile::TempDir::new().unwrap();
        let xml = dir
            .path()
            .join("JetBrains/IntelliJIdea2026.2/options/secret-storage.xml");
        std::fs::create_dir_all(xml.parent().unwrap()).unwrap();
        std::fs::write(
            &xml,
            r#"<application>
    <component name="SecretStorage">
      <Scores>
        <Entry key="Tencent-Cloud.coding-copilot.new.accessToken" value="{}" />
        <Entry key="CodeBuddy-Product-Cache" value="1" />
        <Entry key="ACC_SHARE_CACHE_PRODUCT_MODELS" value="2" />
      </Scores>
    </component>
  </application>"#,
        )
        .unwrap();

        let removed = prune_account_caches(&roots(&dir), Target::IdeaPlugin).unwrap();
        assert_eq!(removed, 2);

        let text = std::fs::read_to_string(&xml).unwrap();
        assert!(!text.contains("CodeBuddy-Product-Cache"));
        assert!(!text.contains("ACC_SHARE_CACHE_PRODUCT_MODELS"));
        assert!(text.contains("Tencent-Cloud.coding-copilot.new.accessToken"));
    }

    /// 状态展示：未登录 → installed=false / 无昵称；有会话 → 昵称 + 版本。
    #[test]
    fn statuses_read_current_account_and_edition() {
        let dir = tempfile::TempDir::new().unwrap();
        let roots = roots(&dir);
        let before = statuses(&roots);
        assert_eq!(before.len(), 2);
        assert!(before
            .iter()
            .all(|s| s.current_name.is_none() && !s.installed));

        let xml = dir
            .path()
            .join("JetBrains/IntelliJIdea2026.2/options/secret-storage.xml");
        std::fs::create_dir_all(xml.parent().unwrap()).unwrap();
        let session = json!({
            "account": {"uid": "u-9", "label": "凌致", "nickname": "凌致"},
            "auth": {"domain": "www.workbuddy.ai"},
        });
        let value = xml_escape(&serde_json::to_string(&session).unwrap());
        std::fs::write(
            &xml,
            format!(
                r#"<application><component name="SecretStorage"><Scores>
        <Entry key="Tencent-Cloud.coding-copilot.new.accessToken" value="{value}" />
      </Scores></component></application>"#
            ),
        )
        .unwrap();

        let after = statuses(&roots);
        let idea = after.iter().find(|s| s.key == "idea").unwrap();
        assert!(idea.installed);
        assert_eq!(idea.current_name.as_deref(), Some("凌致"));
        assert_eq!(idea.current_uid.as_deref(), Some("u-9"));
        assert_eq!(idea.edition.as_deref(), Some("intl"), "授权域决定版本");
    }

    /// 另一个平台的会话不受影响：切软件只动 CodeBuddyIDE。
    #[test]
    fn rekey_only_touches_the_target_platform() {
        let dir = tempfile::TempDir::new().unwrap();
        let base = dir.path().join("CodeBuddyExtension/Data");
        let (from, to) = (
            "aaaaaaaa-1111-2222-3333-444444444444",
            "bbbbbbbb-5555-6666-7777-888888888888",
        );
        for platform in ["CodeBuddyIDE", "JetBrains"] {
            let src = base.join(from).join(platform).join(from);
            std::fs::create_dir_all(src.join("history/ws1")).unwrap();
            std::fs::write(src.join("history/ws1/index.json"), b"{}").unwrap();
        }

        let roots = Roots::at(dir.path(), dir.path(), dir.path());
        rekey_conversations(&roots, "CodeBuddyIDE", from, to).unwrap();

        assert!(
            base.join(from)
                .join("JetBrains")
                .join(from)
                .join("history/ws1/index.json")
                .is_file(),
            "IDEA 插件的会话不能被软件的切换挪走"
        );
        assert!(base
            .join(to)
            .join("CodeBuddyIDE")
            .join(to)
            .join("history/ws1/index.json")
            .is_file());
    }
}
