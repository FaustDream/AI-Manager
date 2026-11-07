//! 健康报告：把「为什么某个功能没生效」从翻日志变成一页可见。
//!
//! 只读本地库与文件系统，**不打任何平台接口**——健康页要能离线打开，
//! 也不能因为打开一次就触发一轮网络请求。平台连通性用每个账号的
//! `last_ok_at` / `last_fail_kind` 聚合，回答的是「最近一次真实查询的结果」。

use crate::app::accounts;
use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::paths;
use chrono::Local;
use serde::Serialize;
use std::path::PathBuf;

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct AccountHealth {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total: usize,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub enabled: usize,
    /// 本机没有凭据：不能切换、不参与自动签到
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub no_secret: usize,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub expired: usize,
    /// 3 天内到期
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub expiring: usize,
    /// 最近一次查询失败（且之后没有成功过）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub failing: usize,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub checked_today: usize,
}

/// 按平台聚合的「最近一次查询结果」：不是实时探测，是事实汇总。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct PlatformHealth {
    pub platform: String,
    pub label: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub accounts: usize,
    /// 最近一次查询失败的账号数
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub failing: usize,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub last_ok_at: Option<i64>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub last_fail_at: Option<i64>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct ScheduleHealth {
    pub id: String,
    pub name: String,
    pub action: String,
    pub enabled: bool,
    /// `每天 08:30` 这类人读规则
    pub rule: String,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub last_run: Option<i64>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub next_run: Option<i64>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct Alert {
    /// 稳定标识：`credential:<iid>` / `credit:<iid>` / `failure:<iid>` / `schedule:<id>`。
    /// 「已处理」按它静默——标题里的天数会变，不能拿标题当键。
    pub key: String,
    /// `bad` = 已经坏了；`warn` = 需要处理但还没坏
    pub level: String,
    /// `credential` / `credit` / `failure` / `schedule`
    pub kind: String,
    pub iid: Option<String>,
    pub title: String,
    pub detail: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct HealthReport {
    pub version: String,
    pub data_dir: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub db_bytes: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub log_bytes: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub backup_bytes: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub generated_at: i64,
    /// 最近一次成功查到积分的时刻（全部账号取最大）。界面据此判断「数据有多旧」。
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub credits_at: Option<i64>,
    pub accounts: AccountHealth,
    pub platforms: Vec<PlatformHealth>,
    pub schedules: Vec<ScheduleHealth>,
}

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

fn dir_bytes(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.metadata().ok())
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
        .sum()
}

fn file_bytes(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

/// 生成健康报告。全部读本地，毫秒级。
pub fn report(db: &Db) -> Result<HealthReport> {
    let mut rows = accounts::repo::list(db)?;
    crate::app::checkin::store::hydrate(db, &mut rows, &crate::app::clock::SystemClock);
    // 积分数据的真实新鲜度：取各账号最近一次「积分快照」的时刻，
    // 不能用 last_ok_at —— 那可能只是查了一次签到状态，积分还是旧的。
    let store = crate::app::checkin::store::SqliteCheckin::new(db);
    let credits_at = rows
        .iter()
        .filter_map(|row| store.view_of(&row.iid).map(|view| view.at))
        .max();

    let now = crate::app::timeutil::now_ms();
    let mut health = AccountHealth {
        total: rows.len(),
        enabled: 0,
        no_secret: 0,
        expired: 0,
        expiring: 0,
        failing: 0,
        checked_today: 0,
    };
    let mut platforms: std::collections::BTreeMap<String, PlatformHealth> = Default::default();

    for row in &rows {
        // 国内版与国际版的 platform 键相同（靠 iid 后缀区分），按展示名分组才不会并成一条
        let entry = platforms
            .entry(row.platform_label.clone())
            .or_insert_with(|| PlatformHealth {
                platform: row.platform.clone(),
                label: row.platform_label.clone(),
                accounts: 0,
                failing: 0,
                last_ok_at: None,
                last_fail_at: None,
            });
        entry.accounts += 1;
        entry.last_ok_at = entry.last_ok_at.max(row.last_ok_at);
        entry.last_fail_at = entry.last_fail_at.max(row.last_fail_at);

        if row.enabled {
            health.enabled += 1;
        }
        if !row.has_secret {
            health.no_secret += 1;
        }
        if row.checked_in == Some(true) {
            health.checked_today += 1;
        }

        // 凭据到期：已过期是坏，3 天内要处理（指标卡用，不再生成告警条目）
        if row.has_secret {
            if let Some(expires) = row.expires_at {
                let days = (expires - now) / DAY_MS;
                if days < 0 {
                    health.expired += 1;
                } else if days <= 3 {
                    health.expiring += 1;
                }
            }
        }

        // 失败：只有「失败晚于最近一次成功」才算当前状态
        let failing = row.last_fail_kind.is_some()
            && row.last_fail_at.unwrap_or(0) >= row.last_ok_at.unwrap_or(0);
        if failing {
            health.failing += 1;
            if let Some(entry) = platforms.get_mut(&row.platform) {
                entry.failing += 1;
            }
        }
    }

    let now_local = Local::now();
    let raw = crate::app::schedule::list(db)?;
    let schedules = raw
        .into_iter()
        .map(|item| ScheduleHealth {
            id: item.id.clone(),
            name: item.name.clone(),
            action: item.action.label().to_string(),
            enabled: item.enabled,
            rule: item.rule_text(),
            last_run: item.last_run.as_deref().and_then(parse_ms),
            next_run: item.next_run(now_local).map(|at| at.timestamp_millis()),
        })
        .collect::<Vec<_>>();

    let data_dir = paths::data_dir();
    Ok(HealthReport {
        version: env!("CARGO_PKG_VERSION").to_string(),
        data_dir: data_dir.display().to_string(),
        db_bytes: file_bytes(&paths::app_db()) + file_bytes(&data_dir.join("app.db-wal")),
        log_bytes: dir_bytes(&paths::logs_dir()),
        backup_bytes: dir_bytes(&data_dir.join("backups")),
        generated_at: now,
        credits_at,
        accounts: health,
        platforms: platforms.into_values().collect(),
        schedules,
    })
}

fn parse_ms(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.timestamp_millis())
}

/// 导出诊断包：一个自包含的 JSON，够把问题说清楚，又不用教人打包目录。
///
/// 只带**账号标识的展示名**（已脱敏）、状态与配置，不含凭据、令牌、Cookie。
pub fn export_bundle(db: &Db) -> Result<PathBuf> {
    export_bundle_to(db, &paths::data_dir().join("exports"))
}

/// 指定目录导出：测试必须能指向临时目录，绝不能往真实 `data/` 写东西。
pub fn export_bundle_to(db: &Db, dir: &std::path::Path) -> Result<PathBuf> {
    let report = report(db)?;
    let mut rows = accounts::repo::list(db)?;
    crate::app::checkin::store::hydrate(db, &mut rows, &crate::app::clock::SystemClock);

    let accounts: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "platform": row.platform,
                "label": row.platform_label,
                "name": row.display_name(),
                "has_secret": row.has_secret,
                "enabled": row.enabled,
                "expires_at": row.expires_at,
                "checked_in": row.checked_in,
                "balance": row.balance,
                "last_ok_at": row.last_ok_at,
                "last_fail_at": row.last_fail_at,
                "last_fail_kind": row.last_fail_kind,
            })
        })
        .collect();

    // 配置：跳过派生态与快照本身，避免诊断包被历史数据撑大
    let settings: std::collections::BTreeMap<String, String> = db
        .kv_entries()?
        .into_iter()
        .filter(|(key, _)| {
            !key.starts_with("next_at:")
                && !key.starts_with("unchecked:")
                && key != "config.snapshots"
                && key != "daily:last"
        })
        .collect();

    let logs = tail_lines(&paths::logs_dir().join("app.jsonl"), 800);
    let bundle = serde_json::json!({
        "app": {
            "version": report.version,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "data_dir": report.data_dir,
            "generated_at": report.generated_at,
            "db_bytes": report.db_bytes,
            "log_bytes": report.log_bytes,
        },
        "health": report,
        "accounts": accounts,
        "settings": settings,
        "logs_tail": logs,
        "note": "本文件由 AI Manager 生成，不含任何凭据、令牌或 Cookie 明文",
    });

    std::fs::create_dir_all(dir)
        .map_err(|e| AppError::new(ErrorKind::Io, format!("创建导出目录失败：{e}")))?;
    let path = dir.join(format!(
        "diagnose-{}.json",
        Local::now().format("%Y%m%d-%H%M%S")
    ));
    let text = serde_json::to_string_pretty(&bundle)
        .map_err(|e| AppError::new(ErrorKind::Invalid, format!("序列化诊断包失败：{e}")))?;
    std::fs::write(&path, text)
        .map_err(|e| AppError::new(ErrorKind::Io, format!("写入诊断包失败：{e}")))?;
    Ok(path)
}

fn tail_lines(path: &std::path::Path, limit: usize) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if lines.len() > limit {
        lines.drain(0..lines.len() - limit);
    }
    lines
}

/// 账号数只有 0 个时给一句人话，避免健康页显示一堆 0 让人以为坏了。
pub fn empty_hint(report: &HealthReport) -> Option<String> {
    (report.accounts.total == 0)
        .then(|| "还没有账号：启动时会自动扫描客户端，也可以去账号中心添加".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> (tempfile::TempDir, Db) {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let now = crate::app::timeutil::now_ms();
        let identity = accounts::AccountIdentity {
            iid: "workbuddy:u1".into(),
            platform: "workbuddy".into(),
            platform_label: "WorkBuddy 国内版".into(),
            account_id: "u1".into(),
            name: "13800000000".into(),
            expires_at: Some(now + DAY_MS),
            source: "test".into(),
            requires_checkin: true,
            has_secret: true,
        };
        accounts::repo::upsert_identity(&db, &identity, now).unwrap();
        // 健康报告只对「有凭据」的账号算到期：没有凭据就没有可过期的登录态
        accounts::repo::set_secret(&db, &identity.iid, b"secret", identity.expires_at).unwrap();
        (dir, db)
    }

    #[test]
    fn report_counts_expiring_credentials() {
        let (_dir, db) = seeded();
        let report = report(&db).unwrap();
        assert_eq!(report.accounts.total, 1);
        assert_eq!(report.accounts.expiring, 1, "1 天后到期应计入 3 天内");
    }

    #[test]
    fn export_bundle_writes_json_without_secrets() {
        let (_dir, db) = seeded();
        let path = export_bundle_to(&db, _dir.path()).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"accounts\""));
        assert!(text.contains("WorkBuddy 国内版"));
        assert!(!text.contains("secret_blob"), "诊断包不能带凭据字段");
        let _ = std::fs::remove_file(path);
    }
}
