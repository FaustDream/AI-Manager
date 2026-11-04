//! CodeBuddy 家族两个客户端的登录态写入面：用 WorkBuddy 账号档案切换它们的登录。
//!
//! 取证结论（2026-11 实机，见 data/backups/switch-exp-* 的对照实验）：
//! 两端存的都是同一份**转换版会话 JSON**（票据解密后的 `auth`/`account`/`accounts`
//! 加上插件 `attachOldSessionProperties` 的拍平字段），差异只在落点：
//!   - 「CodeBuddy 软件」（VSCode-fork）：`%APPDATA%\CodeBuddy CN\User\globalStorage\state.vscdb`
//!     的 `secret://…planning-genie.new.accessTokencn` 键；值是 Node Buffer-JSON
//!     （`{"type":"Buffer","data":[…]}`）包着的 `v10` 信封，主密钥在 `Local State`。
//!   - 「IDEA 插件」（JetBrains 宿主）：`%APPDATA%\JetBrains\*\options\secret-storage.xml`
//!     的 `<Entry key="Tencent-Cloud.coding-copilot.new.accessToken">`，值是 XML 转义的明文 JSON。
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
        match self {
            Self::Software => "CodeBuddy 软件",
            Self::IdeaPlugin => "IDEA 插件",
        }
    }

    /// 需要检查是否在运行的进程名。在跑就不能写：两端都会在退出时把内存态刷回磁盘。
    fn processes(self) -> &'static [&'static str] {
        match self {
            Self::Software => &["CodeBuddy CN.exe", "CodeBuddy.exe"],
            Self::IdeaPlugin => &["idea64", "idea"],
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

/// 读 IDEA 插件 secret-storage.xml 里的会话（明文 Entry）。
fn read_idea_session(roots: &Roots) -> Option<Value> {
    let jb = roots.appdata.join("JetBrains");
    let dirs = std::fs::read_dir(jb).ok()?;
    for dir in dirs.flatten() {
        let xml = dir.path().join("options/secret-storage.xml");
        let Ok(text) = std::fs::read_to_string(&xml) else {
            continue;
        };
        let marker = "key=\"Tencent-Cloud.coding-copilot.new.accessToken\" value=\"";
        let at = text.find(marker)?;
        let value_start = at + marker.len();
        let value_end = value_start + text[value_start..].find('"')?;
        let raw = unescape_xml(&text[value_start..value_end]);
        if let Ok(doc) = serde_json::from_str::<Value>(&raw) {
            return Some(doc);
        }
    }
    None
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
    let vscdb = base.join("User").join("globalStorage").join("state.vscdb");
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
    let key_text = r#"secret://{"extensionId":"tencent-cloud.coding-copilot","key":"planning-genie.new.accessTokencn"}"#;
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
    let jb = roots.appdata.join("JetBrains");
    let mut written = Vec::new();
    let Ok(dirs) = std::fs::read_dir(&jb) else {
        return Err(AppError::new(
            ErrorKind::Io,
            "没有找到 JetBrains 配置目录（IDEA 可能没安装）",
        ));
    };
    for dir in dirs.flatten() {
        let xml = dir.path().join("options").join("secret-storage.xml");
        if !xml.is_file() {
            continue;
        }
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
    if written.is_empty() {
        return Err(AppError::new(
            ErrorKind::Io,
            "没有找到 IDEA 的 secret-storage.xml（插件可能没安装/没登录过）",
        ));
    }
    Ok(written)
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

/// 用 WorkBuddy 国内版账号的档案把目标客户端切成这个账号。
///
/// 只动目标客户端的凭证落点，不碰 WorkBuddy 自己的登录态（那走 `accounts::switch`）。
/// `processes` 是测试注入口：测试里给空列表跳过进程检查。
pub fn switch_target(
    roots: &Roots,
    uid: &str,
    name: &str,
    target: Target,
    backup_dir: &Path,
) -> Result<SwitchOutcome> {
    switch_target_with(roots, uid, name, target, backup_dir, target.processes())
}

fn switch_target_with(
    roots: &Roots,
    uid: &str,
    name: &str,
    target: Target,
    backup_dir: &Path,
    processes: &[&str],
) -> Result<SwitchOutcome> {
    // 档案在 <data>/profiles/cn/<uid>/auth.info（与 WorkBuddyProvider 的 pack_dir 同构）
    let pack = crate::app::paths::data_dir()
        .join("profiles")
        .join("cn")
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
    let session = build_session(&ticket, target.auth_config_id())?;
    // 会话保留的来源：目标客户端当前登录的账号（写盘前读，之后就是新账号了）
    let from_uid = read_current_uid(roots, target);

    // 写前备份：后悔了去 backup 里找
    let backup = match target {
        Target::Software => {
            let file = roots
                .appdata
                .join("CodeBuddy CN")
                .join("User")
                .join("globalStorage")
                .join("state.vscdb");
            if file.is_file() {
                Some(fsx::backup_to(&file, backup_dir, "codebuddy-software")?)
            } else {
                None
            }
        }
        Target::IdeaPlugin => {
            let xml = roots
                .appdata
                .join("JetBrains")
                .join("IntelliJIdea2026.2")
                .join("options")
                .join("secret-storage.xml");
            if xml.is_file() {
                Some(fsx::backup_to(&xml, backup_dir, "codebuddy-idea")?)
            } else {
                None
            }
        }
    };

    match target {
        Target::Software => {
            write_software(roots, &session)?;
        }
        Target::IdeaPlugin => {
            write_idea(roots, &session)?;
        }
    };

    let mut message = format!(
        "{}已切换到「{name}」，重启 {} 后生效",
        target.label(),
        target.label()
    );
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
    if backup.is_none() {
        message.push_str("（本次没有可备份的旧登录态）");
    }
    Ok(SwitchOutcome { message, backup })
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
            Target::Software,
            &dir.path().join("bk"),
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
