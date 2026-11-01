//! v1 数据导入（`data/*.json` + `accounts.dat` → `app.db`）。
//!
//! 三条硬规则：
//!   1. **绝不修改 v1 的 `data/`**——原样保留，随时可回退
//!   2. **整包一个事务**——失败回滚，不留半迁移状态
//!   3. **不写任何凭据到日志**，凭据只进 DPAPI 加密列
//!
//! ## 主键：`@intl` 不是历史遗留，是 `account_id` 的一部分
//!
//! 实测 `accounts.dat`：国际版账号的 `account_id` 本身就形如 `{uuid}@intl`（36+5 字符），
//! 因此 `iid = "{platform}:{account_id}"` 天然带后缀，与 `credit_history.json` 的 key 一致。
//! **剥离后缀会切断 WorkBuddy 国际版的积分历史**——这是设计文档早期的一处误判，已更正。
//!
//! 真正的「对不上」是另一回事：Qoder / Trae 的账号**不在 `accounts.dat` 里**（靠扫描客户端
//! 文件发现），所以台账里有它们的 iid 而账号表暂时没有。这属于正常状态，导入时照原样写入，
//! 只统计数量作为提示，绝不丢弃、也不报错。

use crate::app::db::Db;
use crate::app::error::{AppError, Result};
use crate::app::secret;
use base64::Engine;
use serde_json::Value;
use std::path::Path;

/// 迁移收尾（P7）：计数校验与归档。与导入器分开，因为归档要「先校验后写」。
pub mod verify;

#[derive(Debug, Default, Clone)]
pub struct ImportReport {
    pub accounts: usize,
    pub checkins: usize,
    pub daily_marks: usize,
    pub credit_lots: usize,
    pub credit_snapshots: usize,
    pub sessions: usize,
    pub skills: usize,
    pub mcp: usize,
    /// 台账里有、账号库暂时没有的记录条数（Qoder / Trae 靠扫描发现，属正常）。
    /// 按行计，不是按 iid 去重——目的是让「有多少条数据暂时悬空」可见。
    pub orphan_ledger: usize,
    pub warnings: Vec<String>,
}

impl ImportReport {
    pub fn summary(&self) -> String {
        format!(
            "账号 {} · 签到 {} · 日标记 {} · 积分批次 {} · 积分快照 {} · 会话 {} · 技能 {} · MCP {}",
            self.accounts,
            self.checkins,
            self.daily_marks,
            self.credit_lots,
            self.credit_snapshots,
            self.sessions,
            self.skills,
            self.mcp
        )
    }
}

pub fn import_v1(db: &Db, data_dir: &Path) -> Result<ImportReport> {
    if let Some(from) = db.migrated_from()? {
        return Err(AppError::new(
            "conflict",
            format!("已导入过（来自 {from}），拒绝重复导入"),
        ));
    }
    if !data_dir.exists() {
        return Err(AppError::new(
            "invalid",
            format!("v1 数据目录不存在：{}", data_dir.display()),
        ));
    }

    let report = db.write(|conn| {
        let tx = conn.unchecked_transaction()?;
        let mut rep = ImportReport::default();
        let now = now_ms();

        // ---- 账号库（凭据：整包 DPAPI → 按账号单独加密）----
        let account_iids = import_accounts(&tx, data_dir, now, &mut rep)?;

        // ---- 台账（照原样写入，不因账号暂缺而丢弃）----
        import_checkins(&tx, data_dir, &account_iids, &mut rep)?;
        import_daily(&tx, data_dir, &account_iids, &mut rep)?;
        import_credit_lots(&tx, data_dir, &account_iids, &mut rep)?;
        import_credit_history(&tx, data_dir, &account_iids, &mut rep)?;

        // ---- 会话索引 ----
        import_sessions(&tx, data_dir, &mut rep)?;

        // ---- 技能与 MCP ----
        import_hub(&tx, data_dir, now, &mut rep)?;

        // ---- 配置：逐键进 kv，不解释语义（解释留给各阶段）----
        import_config(&tx, data_dir, now, &mut rep)?;

        tx.commit()?;
        Ok(rep)
    })?;

    db.mark_migrated("v1.4.0")?;
    Ok(report)
}

// ---------------------------------------------------------------- 各段导入

fn import_accounts(
    tx: &rusqlite::Transaction<'_>,
    dir: &Path,
    now: i64,
    rep: &mut ImportReport,
) -> Result<Vec<String>> {
    let path = dir.join("accounts.dat");
    if !path.exists() {
        rep.warnings
            .push("accounts.dat 不存在，跳过账号导入".into());
        return Ok(Vec::new());
    }

    let raw = std::fs::read(&path)?;
    let text = String::from_utf8_lossy(&raw);
    let blob = base64::engine::general_purpose::STANDARD
        .decode(text.trim().as_bytes())
        .map_err(|e| AppError::new("invalid", format!("accounts.dat 不是合法 base64：{e}")))?;
    let plain = match secret::unprotect(&blob) {
        Ok(plain) => plain,
        Err(_) => {
            // DPAPI 与当前用户绑定：换用户/换机解不开。跳过账号但继续其余表。
            rep.warnings.push(
                "accounts.dat 无法解密（DPAPI 与当前 Windows 用户绑定），已跳过账号导入".into(),
            );
            return Ok(Vec::new());
        }
    };
    let list: Vec<Value> = serde_json::from_slice(&plain)
        .map_err(|e| AppError::new("invalid", format!("accounts.dat 内容不是账号数组：{e}")))?;

    let order = read_account_order(dir);
    let mut iids = Vec::with_capacity(list.len());

    for (idx, item) in list.iter().enumerate() {
        let platform = item
            .get("platform")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let account_id = item
            .get("account_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if platform.is_empty() || account_id.is_empty() {
            rep.warnings
                .push(format!("第 {idx} 条账号缺 platform/account_id，已跳过"));
            continue;
        }
        // iid 原样拼接：@intl 属于 account_id 本身，不能剥离
        let iid = format!("{platform}:{account_id}");
        let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
        let expires_at = item.get("expires_at").and_then(parse_ts);
        let requires = item
            .get("requires_checkin")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        // meta 是自由 dict，实测躺着 refresh_token / 手机号 —— 只保留键名
        let meta_keys: Vec<String> = item
            .get("meta")
            .and_then(Value::as_object)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();

        // 凭据：整条原始 JSON 单独加密。token / refresh_token / meta 全在密文里，
        // 不需要逐字段认识它们；读出来才解密，内存明文窗口 = 一个账号。
        let secret_blob = match secret::protect(&serde_json::to_vec(item)?) {
            Ok(blob) => Some(blob),
            Err(e) => {
                rep.warnings
                    .push(format!("{iid} 凭据加密失败（{e}），该账号将无登录态"));
                None
            }
        };

        let sort_order = order
            .iter()
            .position(|x| x == &iid)
            .map(|i| i as i64)
            .unwrap_or(idx as i64);

        tx.execute(
            "INSERT INTO accounts
                (iid, platform, platform_label, account_id, name, expires_at, source,
                 requires_checkin, meta_keys, secret_blob, sort_order, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, '', ?7, ?8, ?9, ?10, ?11, ?11)
             ON CONFLICT(iid) DO NOTHING",
            rusqlite::params![
                iid,
                platform,
                platform_label(platform),
                account_id,
                name,
                expires_at,
                requires as i64,
                serde_json::to_string(&meta_keys)?,
                secret_blob,
                sort_order,
                now,
            ],
        )?;
        iids.push(iid);
    }
    rep.accounts = iids.len();
    Ok(iids)
}

fn import_checkins(
    tx: &rusqlite::Transaction<'_>,
    dir: &Path,
    known: &[String],
    rep: &mut ImportReport,
) -> Result<()> {
    let Some(value) = read_json(dir, "checked.json", rep) else {
        return Ok(());
    };
    let Some(map) = value.as_object() else {
        rep.warnings.push("checked.json 不是对象，已跳过".into());
        return Ok(());
    };
    let known = known_set(known);
    for (iid, at) in map {
        let Some(ts) = parse_ts(at) else { continue };
        count_orphan(iid, &known, rep);
        tx.execute(
            "INSERT INTO checkin_log(iid, checked_at) VALUES (?1, ?2)
             ON CONFLICT(iid, checked_at) DO NOTHING",
            rusqlite::params![iid, ts],
        )?;
        rep.checkins += 1;
    }
    Ok(())
}

fn import_daily(
    tx: &rusqlite::Transaction<'_>,
    dir: &Path,
    known: &[String],
    rep: &mut ImportReport,
) -> Result<()> {
    let Some(value) = read_json(dir, "daily.json", rep) else {
        return Ok(());
    };
    let date = value
        .get("date")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if date.is_empty() {
        rep.warnings.push("daily.json 缺 date，已跳过".into());
        return Ok(());
    }
    let known = known_set(known);
    if let Some(done) = value.get("done").and_then(Value::as_array) {
        for item in done {
            let Some(iid) = item.as_str() else { continue };
            count_orphan(iid, &known, rep);
            tx.execute(
                "INSERT INTO daily_marks(iid, date) VALUES (?1, ?2)
                 ON CONFLICT(iid, date) DO NOTHING",
                rusqlite::params![iid, date],
            )?;
            rep.daily_marks += 1;
        }
    }
    // 轮询状态属于「跑起来的数据」，存 kv 即可，不必建表（P3 会重新定义）
    for key in ["last_round", "last_result"] {
        if let Some(v) = value.get(key) {
            put_kv(tx, &format!("daily.{key}"), &serde_json::to_string(v)?)?;
        }
    }
    Ok(())
}

fn import_credit_lots(
    tx: &rusqlite::Transaction<'_>,
    dir: &Path,
    known: &[String],
    rep: &mut ImportReport,
) -> Result<()> {
    let Some(value) = read_json(dir, "credits.json", rep) else {
        return Ok(());
    };
    let Some(map) = value.as_object() else {
        rep.warnings.push("credits.json 不是对象，已跳过".into());
        return Ok(());
    };
    let known = known_set(known);
    for (iid, lots) in map {
        let Some(lots) = lots.as_array() else {
            continue;
        };
        count_orphan(iid, &known, rep);
        for lot in lots {
            let amount = lot.get("amount").and_then(Value::as_f64).unwrap_or(0.0);
            let granted_at = lot.get("granted").and_then(parse_ts);
            let expires_at = lot.get("expires").and_then(parse_ts);
            let lot_key = lot
                .get("key")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("{iid}:{granted_at:?}:{amount}"));
            tx.execute(
                "INSERT INTO credit_lots(iid, amount, expires_at, granted_at, lot_key)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(lot_key) DO NOTHING",
                rusqlite::params![iid, amount, expires_at, granted_at, lot_key],
            )?;
            rep.credit_lots += 1;
        }
    }
    Ok(())
}

fn import_credit_history(
    tx: &rusqlite::Transaction<'_>,
    dir: &Path,
    known: &[String],
    rep: &mut ImportReport,
) -> Result<()> {
    let Some(value) = read_json(dir, "credit_history.json", rep) else {
        return Ok(());
    };
    let Some(map) = value.as_object() else {
        rep.warnings
            .push("credit_history.json 不是对象，已跳过".into());
        return Ok(());
    };
    let known = known_set(known);
    for (iid, series) in map {
        let Some(series) = series.as_object() else {
            continue;
        };
        count_orphan(iid, &known, rep);
        for (date, balance) in series {
            let Some(balance) = balance.as_f64() else {
                continue;
            };
            tx.execute(
                "INSERT INTO credit_snapshots(iid, date, balance) VALUES (?1, ?2, ?3)
                 ON CONFLICT(iid, date) DO UPDATE SET balance = excluded.balance",
                rusqlite::params![iid, date, balance],
            )?;
            rep.credit_snapshots += 1;
        }
    }
    Ok(())
}

fn import_sessions(
    tx: &rusqlite::Transaction<'_>,
    dir: &Path,
    rep: &mut ImportReport,
) -> Result<()> {
    let Some(value) = read_json(dir, "session_index.json", rep) else {
        return Ok(());
    };
    let Some(editions) = value.get("editions").and_then(Value::as_object) else {
        rep.warnings
            .push("session_index.json 缺 editions，已跳过".into());
        return Ok(());
    };
    for (edition, inner) in editions {
        let Some(sessions) = inner.get("sessions").and_then(Value::as_object) else {
            continue;
        };
        let label = match edition.as_str() {
            "cn" => "国内版",
            "intl" => "国际版",
            other => other,
        };
        for (sid, row) in sessions {
            let get = |k: &str| {
                row.get(k)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            let last_at = row.get("last").and_then(parse_ts);
            let refs = row
                .get("refs")
                .cloned()
                .unwrap_or_else(|| Value::Array(vec![]));
            tx.execute(
                "INSERT INTO session_index
                    (kind, source_key, source_label, account, session_id,
                     title, slug, cwd, origin, refs, last_at, seen_at)
                 VALUES ('workbuddy', ?1, ?2, '', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(kind, source_key, account, session_id) DO NOTHING",
                rusqlite::params![
                    edition,
                    label,
                    sid,
                    get("title"),
                    get("slug"),
                    get("cwd"),
                    get("origin"),
                    serde_json::to_string(&refs)?,
                    last_at,
                    row.get("seen_at").and_then(Value::as_str),
                ],
            )?;
            rep.sessions += 1;
        }
    }
    Ok(())
}

fn import_hub(
    tx: &rusqlite::Transaction<'_>,
    dir: &Path,
    now: i64,
    rep: &mut ImportReport,
) -> Result<()> {
    let Some(value) = read_json(dir, "hub.json", rep) else {
        return Ok(());
    };

    for (kind, counter) in [("skill", 0u8), ("mcp", 1u8)] {
        let field = if counter == 0 { "skills" } else { "mcp" };
        let Some(items) = value.get(field).and_then(Value::as_object) else {
            continue;
        };
        for (name, item) in items {
            let targets = item
                .get("targets")
                .cloned()
                .unwrap_or_else(|| Value::Array(vec![]));
            let deployed = item
                .get("deployed")
                .cloned()
                .unwrap_or_else(|| Value::Object(Default::default()));
            tx.execute(
                "INSERT INTO hub_items(kind, name, targets, deployed, digest, updated_at)
                 VALUES (?1, ?2, ?3, ?4, '', ?5)
                 ON CONFLICT(kind, name) DO NOTHING",
                rusqlite::params![
                    kind,
                    name,
                    serde_json::to_string(&targets)?,
                    serde_json::to_string(&deployed)?,
                    now
                ],
            )?;
            if counter == 0 {
                rep.skills += 1;
            } else {
                rep.mcp += 1;
            }
        }
    }

    // 全局设置进 kv，避免为三个标量建表
    for key in ["version", "library", "auto_sync", "default_targets"] {
        if let Some(v) = value.get(key) {
            put_kv(tx, &format!("hub.{key}"), &serde_json::to_string(v)?)?;
        }
    }
    Ok(())
}

fn import_config(
    tx: &rusqlite::Transaction<'_>,
    dir: &Path,
    _now: i64,
    rep: &mut ImportReport,
) -> Result<()> {
    let Some(value) = read_json(dir, "config.json", rep) else {
        return Ok(());
    };
    let Some(map) = value.as_object() else {
        rep.warnings.push("config.json 不是对象，已跳过".into());
        return Ok(());
    };
    for (key, value) in map {
        put_kv(tx, &format!("config.{key}"), &serde_json::to_string(value)?)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- 工具

/// 缺失文件不中断导入：记一条 warn 继续（老用户可能从没用过某个功能）。
fn read_json(dir: &Path, name: &str, rep: &mut ImportReport) -> Option<Value> {
    let path = dir.join(name);
    if !path.exists() {
        rep.warnings.push(format!("{name} 不存在，已跳过"));
        return None;
    }
    match std::fs::read(&path)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
    {
        Some(value) => Some(value),
        None => {
            rep.warnings.push(format!("{name} 解析失败，已跳过"));
            None
        }
    }
}

fn read_account_order(dir: &Path) -> Vec<String> {
    std::fs::read(dir.join("config.json"))
        .ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .and_then(|v| v.get("account_order").and_then(Value::as_array).cloned())
        .map(|items| {
            items
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn put_kv(tx: &rusqlite::Transaction<'_>, key: &str, value: &str) -> Result<()> {
    tx.execute(
        "INSERT INTO kv(key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        rusqlite::params![key, value, now_ms()],
    )?;
    Ok(())
}

fn known_set(known: &[String]) -> std::collections::HashSet<&str> {
    known.iter().map(String::as_str).collect()
}

fn count_orphan(iid: &str, known: &std::collections::HashSet<&str>, rep: &mut ImportReport) {
    if !known.is_empty() && !known.contains(iid) {
        rep.orphan_ledger += 1;
    }
}

fn platform_label(platform: &str) -> &'static str {
    match platform {
        "workbuddy" => "WorkBuddy",
        "qoder" | "qoder_intl" => "Qoder",
        "trae" => "Trae",
        "zcode" => "ZCode",
        "codebuddy" => "CodeBuddy",
        _ => "其他",
    }
}

fn parse_ts(value: &Value) -> Option<i64> {
    crate::app::timeutil::parse_ts(value)
}

fn now_ms() -> i64 {
    crate::app::timeutil::now_ms()
}
