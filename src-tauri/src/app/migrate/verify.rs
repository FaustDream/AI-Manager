//! 迁移收尾：三项计数校验 + v1 文件归档。
//!
//! 设计文档写的是「把 v1 文件整体移到 `data-v1-archive/`」，但**开发期的数据目录
//! 就是 v1 的数据目录**（`paths::data_dir()` 指向仓库里的 `data/`），照字面 move
//! 会同时毁掉 v1 和 2.0。因此这里只做**复制**：原文件留在原地，归档是第二份。
//! 这也是 `AGENTS.md` §4 第 2 条（不清空 `data/`）的唯一合规实现方式。
//!
//! ## 校验口径：查「有没有丢」，不是查「数字相不相等」
//!
//! 文档写的是「三项计数与 v1 一致」，实测**账号数天生就对不上**：v1 的
//! `accounts.dat` 里只有 7 条（WorkBuddy 5 + ZCode 2），2.0 扫客户端又发现
//! Qoder 两个版本与 Trae 共 3 条——`migrate/mod.rs` 顶部已经写明这类账号本来
//! 就不在 `accounts.dat` 里。会话数与积分台账同理：2.0 每扫一次、每过一天都会
//! 合法地多出几行。
//!
//! 所以 `ok` 的判据是**包含关系**：v1 侧的每一个 key 在 2.0 侧都必须有落点。
//! 数字照实报给界面（前端按 `ok` 决定能不能归档，不要自己比大小）。
//!
//! 三条口径（v1 侧 → 2.0 侧）：
//!   - 账号：`accounts.dat` 的 `{platform}:{account_id}` → `accounts.iid`
//!   - 会话：`session_index.json` 的 `editions.*.sessions` 的**会话 id** →
//!     `session_index` 里 `kind='workbuddy'` 的 `session_id`
//!     （v1 只索引 WorkBuddy 的会话，2.0 扫出的其余五家来源不算迁移保真度）
//!   - 积分台账：`credit_history.json` 的 `{iid}.{date}` → `credit_snapshots(iid, date)`
//!
//! 会话**不能**按 `(edition, id)` 比：实测 3 条会话在 v1 记在 `intl` 下，2.0 重扫后
//! 落在 `cn` 下（客户端版本切换后目录归属会变，2.0 是按目录重新判定的）。
//! 会话的稳定身份只有 uuid 本身，edition 只是展示属性。

use crate::app::db::Db;
use crate::app::error::{AppError, Result};
use crate::app::secret;
use base64::Engine;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
/// 一项计数：v1 侧与 2.0 侧。只用于展示。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct CountPair {
    pub v1: u32,
    pub v2: u32,
}

/// v1 有、2.0 没有的条数。这才是「迁移丢没丢数据」的判据。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Missing {
    pub accounts: u32,
    pub sessions: u32,
    pub ledger: u32,
}

impl Missing {
    fn total(&self) -> u32 {
        self.accounts + self.sessions + self.ledger
    }
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MigrationReport {
    pub accounts: CountPair,
    pub sessions: CountPair,
    pub ledger: CountPair,
    pub ok: bool,
    /// 契约固定四个字段，缺失明细留在后端，只进日志与错误提示
    #[serde(skip)]
    pub missing: Missing,
}

impl MigrationReport {
    /// 一句话说清三项计数与缺失条数，日志与错误提示共用。
    pub fn describe(&self) -> String {
        format!(
            "账号 {}/{}（缺 {}） · 会话 {}/{}（缺 {}） · 积分台账 {}/{}（缺 {}）",
            self.accounts.v1,
            self.accounts.v2,
            self.missing.accounts,
            self.sessions.v1,
            self.sessions.v2,
            self.missing.sessions,
            self.ledger.v1,
            self.ledger.v2,
            self.missing.ledger
        )
    }
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct ArchiveReport {
    pub archive_dir: String,
    pub verified: bool,
}

/// 归档范围：`migrate::import_v1` 真正读过的文件。
///
/// 不复制 `skills/`、`profiles/`、`backups/`、`trash/`、`catalog.db`——这些目录
/// 2.0 仍在原址使用，整目录拷一份 465MB 既没信息量也会把归档撑成负担。
const V1_FILES: [&str; 8] = [
    "accounts.dat",
    "checked.json",
    "daily.json",
    "credits.json",
    "credit_history.json",
    "session_index.json",
    "hub.json",
    "config.json",
];

/// 「这份数据已归档」的标记。有它就不许再归档。
const KV_DONE: &str = "migration_done";
/// 上一次归档落在哪，用于重复归档时告诉用户「上次那份在哪」。
const KV_ARCHIVE_DIR: &str = "migration_archive_dir";

/// 校验三项。只读：不写库、不碰 v1 文件。
pub fn verify(db: &Db, data_dir: &Path) -> Result<MigrationReport> {
    let v1_accounts = v1_account_iids(data_dir)?;
    let v1_sessions = v1_session_keys(data_dir)?;
    let v1_ledger = v1_ledger_keys(data_dir)?;
    let v2 = v2_keys(db)?;

    let missing = Missing {
        accounts: count_missing(&v1_accounts, &v2.accounts),
        sessions: count_missing(&v1_sessions, &v2.sessions),
        ledger: count_missing(&v1_ledger, &v2.ledger),
    };
    let report = MigrationReport {
        accounts: CountPair {
            v1: v1_accounts.len() as u32,
            v2: v2.accounts.len() as u32,
        },
        sessions: CountPair {
            v1: v1_sessions.len() as u32,
            v2: v2.session_rows,
        },
        ledger: CountPair {
            v1: v1_ledger.len() as u32,
            v2: v2.ledger.len() as u32,
        },
        ok: false,
        missing,
    };
    // v1 侧一条数据都没有 = 这台机器没有可归档的东西，
    // 不能拿「0 缺失」冒充通过，否则会给一份空归档盖上迁移完成的章
    let has_v1 = report.accounts.v1 + report.sessions.v1 + report.ledger.v1 > 0;
    Ok(MigrationReport {
        ok: has_v1 && missing.total() == 0,
        ..report
    })
}

/// 归档 v1 文件。四步：查重 → 校验 → 复制 → 再校验 → 写 kv。
///
/// 任何一步不一致都返回错误且**不写 kv**：`migration_done` 是「这份数据可以放心
/// 交给 2.0」的凭据，宁可让用户看到错误，也不能让它变成一句空话。
pub fn archive(db: &Db, data_dir: &Path) -> Result<ArchiveReport> {
    // 已经归档过就直接拒：重复点只会在 data-v1-archive/ 下无限堆目录，
    // 而界面完全看不出「上次已经备过一份」。
    if let Some(message) = archived_before(db)? {
        return Err(AppError::new("already_archived", message));
    }

    let before = verify(db, data_dir)?;
    if !before.ok {
        return Err(AppError::new(
            "migration_mismatch",
            format!("校验未通过，拒绝归档：{}", before.describe()),
        ));
    }

    let dest = unique_dir(archive_root(data_dir).join(stamp()));
    std::fs::create_dir_all(&dest)
        .map_err(|e| AppError::new("io", format!("创建归档目录失败：{e}")))?;
    for name in V1_FILES {
        let src = data_dir.join(name);
        if !src.is_file() {
            continue;
        }
        std::fs::copy(&src, dest.join(name))
            .map_err(|e| AppError::new("io", format!("复制 {name} 失败：{e}")))?;
    }

    // 复制期间 v1 侧被别处改过（v1 客户端还在跑）→ 这份归档不可信
    let after = verify(db, data_dir)?;
    if !after.ok {
        return Err(AppError::new(
            "migration_mismatch",
            format!("复制后校验失败，未写迁移标记：{}", after.describe()),
        ));
    }

    // 先记目录再记标记：标记一旦写进去，重复归档的提示就得能指出上一次归档在哪
    db.kv_set(
        KV_ARCHIVE_DIR,
        &serde_json::to_string(&dest.display().to_string())?,
    )?;
    // 存成 JSON 字符串，`settings_get` 直接可读
    db.kv_set(KV_DONE, "\"v1.4.0\"")?;

    Ok(ArchiveReport {
        archive_dir: dest.display().to_string(),
        verified: true,
    })
}

/// 已经归档过就返回一句能直接给用户看的话，否则 `None`。
fn archived_before(db: &Db) -> Result<Option<String>> {
    let Some(raw) = db.kv_get(KV_DONE)? else {
        return Ok(None);
    };
    let version = json_string(&raw);
    let dir = db
        .kv_get(KV_ARCHIVE_DIR)?
        .map(|v| json_string(&v))
        .unwrap_or_default();
    let where_ = if dir.is_empty() {
        String::new()
    } else {
        format!(" → {}", short_dir(&dir))
    };
    Ok(Some(format!(
        "已经归档过（{version}{where_}），无需重复归档"
    )))
}

/// kv 里存的是 JSON 字符串；万一被手工写坏，退回原文也比报错强。
fn json_string(raw: &str) -> String {
    serde_json::from_str::<String>(raw).unwrap_or_else(|_| raw.to_string())
}

/// 只留末两段（`data-v1-archive/<时间戳>`）：完整路径太长，界面里放不下，
/// 末两段已足以定位到那一次归档。
fn short_dir(full: &str) -> String {
    let parts: Vec<&str> = full.rsplit(['\\', '/']).take(2).collect();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}

// ---------------------------------------------------------------- v1 侧 key

fn v1_account_iids(dir: &Path) -> Result<Vec<String>> {
    let path = dir.join("accounts.dat");
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read(&path)?;
    let text = String::from_utf8_lossy(&raw);
    let blob = base64::engine::general_purpose::STANDARD
        .decode(text.trim().as_bytes())
        .map_err(|e| AppError::new("invalid", format!("accounts.dat 不是合法 base64：{e}")))?;
    // 解不开就不能说「没丢」：换用户/换机时 DPAPI 必然失败，此时必须让校验报错
    let plain = secret::unprotect(&blob).map_err(|e| {
        AppError::new(
            "crypto",
            format!("accounts.dat 无法解密（DPAPI 与当前 Windows 用户绑定）：{e}"),
        )
    })?;
    let list: Vec<Value> = serde_json::from_slice(&plain)
        .map_err(|e| AppError::new("invalid", format!("accounts.dat 内容不是账号数组：{e}")))?;

    // iid 的拼法与 `import_v1` 必须完全一致，否则包含关系会全部落空
    Ok(list
        .iter()
        .filter_map(|item| {
            let platform = item.get("platform").and_then(Value::as_str)?;
            let account_id = item.get("account_id").and_then(Value::as_str)?;
            if platform.is_empty() || account_id.is_empty() {
                return None;
            }
            Some(format!("{platform}:{account_id}"))
        })
        .collect())
}

fn v1_session_keys(dir: &Path) -> Result<Vec<String>> {
    let Some(value) = read_json(dir, "session_index.json")? else {
        return Ok(Vec::new());
    };
    let Some(editions) = value.get("editions").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    // 按条目收集（不去重）：v1 的 cn / intl 各有一份，与 2.0 的「行」对齐
    let mut out = Vec::new();
    for inner in editions.values() {
        let Some(sessions) = inner.get("sessions").and_then(Value::as_object) else {
            continue;
        };
        out.extend(sessions.keys().cloned());
    }
    Ok(out)
}

fn v1_ledger_keys(dir: &Path) -> Result<Vec<String>> {
    let Some(value) = read_json(dir, "credit_history.json")? else {
        return Ok(Vec::new());
    };
    let Some(map) = value.as_object() else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for (iid, series) in map {
        let Some(series) = series.as_object() else {
            continue;
        };
        for date in series.keys() {
            out.push(format!("{iid}|{date}"));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- 2.0 侧 key

#[derive(Debug, Default)]
struct V2Keys {
    accounts: HashSet<String>,
    sessions: HashSet<String>,
    ledger: HashSet<String>,
    /// 会话表的**行数**，与 v1 的条目数对齐用于展示。
    /// 会话身份用 `sessions`（uuid 集合），展示用行数：同一条会话在 cn / intl
    /// 两个来源下各占一行，去重后报 55 会让用户以为少了一半。
    session_rows: u32,
}

fn v2_keys(db: &Db) -> Result<V2Keys> {
    db.read(|conn| {
        let session_rows: i64 = conn.query_row(
            "SELECT COUNT(*) FROM session_index WHERE kind = 'workbuddy'",
            [],
            |r| r.get(0),
        )?;
        Ok(V2Keys {
            accounts: collect(conn, "SELECT iid FROM accounts")?,
            sessions: collect(
                conn,
                "SELECT session_id FROM session_index WHERE kind = 'workbuddy'",
            )?,
            ledger: collect(conn, "SELECT iid || '|' || date FROM credit_snapshots")?,
            session_rows: session_rows as u32,
        })
    })
}

fn collect(conn: &rusqlite::Connection, sql: &str) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut out = HashSet::new();
    for row in rows {
        out.insert(row?);
    }
    Ok(out)
}

fn count_missing(v1: &[String], v2: &HashSet<String>) -> u32 {
    v1.iter().filter(|key| !v2.contains(*key)).count() as u32
}

// ---------------------------------------------------------------- 工具

fn read_json(dir: &Path, name: &str) -> Result<Option<Value>> {
    let path = dir.join(name);
    if !path.is_file() {
        return Ok(None);
    }
    let raw = std::fs::read(&path)?;
    let value = serde_json::from_slice(&raw)
        .map_err(|e| AppError::new("invalid", format!("{name} 解析失败：{e}")))?;
    Ok(Some(value))
}

/// 归档根目录与 `data/` 同级（`D:\AI Manager\data-v1-archive`）。
fn archive_root(data_dir: &Path) -> PathBuf {
    data_dir
        .parent()
        .map(|p| p.join("data-v1-archive"))
        .unwrap_or_else(|| PathBuf::from("data-v1-archive"))
}

fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// 重名就加序号，绝不覆盖已有归档。
///
/// 重复归档已被 kv 查重挡住，这里只兜一种边角情况：kv 被手工清掉、又刚好撞上同一秒。
fn unique_dir(base: PathBuf) -> PathBuf {
    if !base.exists() {
        return base;
    }
    for n in 2..1000 {
        let name = format!(
            "{}-{n}",
            base.file_name().unwrap_or_default().to_string_lossy()
        );
        let candidate = base.with_file_name(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 最小但结构完整的 v1 目录，造法与 `tests/import_v1.rs` 一致。
    fn build_fixture(dir: &Path) {
        let accounts = serde_json::json!([
            {
                "platform": "workbuddy",
                "account_id": "5af46132-29b2-49a0-8a10-51eb6eff5e0c@intl",
                "name": "国际版账号",
                "token": "tk-should-not-leak",
                "requires_checkin": true
            },
            { "platform": "zcode", "account_id": "12754401", "name": "Z 账号", "token": "tk2" }
        ]);
        let blob = secret::protect(&serde_json::to_vec(&accounts).unwrap()).unwrap();
        std::fs::write(
            dir.join("accounts.dat"),
            base64::engine::general_purpose::STANDARD.encode(blob),
        )
        .unwrap();
        std::fs::write(
            dir.join("checked.json"),
            r#"{"zcode:12754401":"2026-10-30T11:06:25"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("daily.json"),
            r#"{"date":"2026-10-30","done":["zcode:12754401"]}"#,
        )
        .unwrap();
        std::fs::write(dir.join("credits.json"), r#"{}"#).unwrap();
        std::fs::write(
            dir.join("credit_history.json"),
            r#"{"zcode:12754401":{"2026-10-29":1.0,"2026-10-30":2.0},
                "workbuddy:5af46132-29b2-49a0-8a10-51eb6eff5e0c@intl":{"2026-10-30":0.5}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("session_index.json"),
            r#"{"version":1,"editions":{
                "cn":{"sessions":{"s1":{"title":"一"}}},
                "intl":{"sessions":{"s2":{"title":"二"},"s3":{"title":"三"}}}
            }}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("hub.json"),
            r#"{"version":2,"skills":{},"mcp":{}}"#,
        )
        .unwrap();
        std::fs::write(dir.join("config.json"), r#"{"theme":"dark"}"#).unwrap();
    }

    fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut out: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .map(|e| {
                (
                    e.file_name().to_string_lossy().to_string(),
                    std::fs::read(e.path()).unwrap(),
                )
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn imported_db(root: &Path, data: &Path) -> Db {
        let db = Db::open_at(&root.join("app.db")).unwrap();
        crate::app::migrate::import_v1(&db, data).unwrap();
        db
    }

    #[test]
    fn verify_then_archive_copies_and_keeps_v1_files() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        build_fixture(&data);
        let db = imported_db(tmp.path(), &data);

        let before = snapshot(&data);
        let report = verify(&db, &data).unwrap();
        assert!(report.ok, "{}", report.describe());
        assert_eq!(report.accounts, CountPair { v1: 2, v2: 2 });
        assert_eq!(report.sessions, CountPair { v1: 3, v2: 3 });
        assert_eq!(report.ledger, CountPair { v1: 3, v2: 3 });
        assert_eq!(report.missing, Missing::default(), "一条都不能缺");

        let archived = archive(&db, &data).unwrap();
        assert!(archived.verified);
        let dest = PathBuf::from(&archived.archive_dir);
        assert!(dest.starts_with(tmp.path().join("data-v1-archive")));
        assert!(dest.join("accounts.dat").is_file(), "归档必须包含账号库");
        assert!(dest.join("session_index.json").is_file());

        // 归档只复制：v1 原文件字节级不变，且仍在原地
        assert_eq!(snapshot(&data), before, "归档绝不能动 v1 的 data/");
        assert_eq!(
            db.kv_get("migration_done").unwrap().as_deref(),
            Some("\"v1.4.0\"")
        );
    }

    /// 2.0 侧多出来的行（扫客户端发现的新账号 / 新会话 / 新的一天快照）是正常增长，
    /// 不能因此拦住归档；反过来，少一行就是真丢了，必须拦住。
    #[test]
    fn superset_passes_but_a_missing_row_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        build_fixture(&data);
        let db = imported_db(tmp.path(), &data);

        db.write(|conn| {
            conn.execute(
                "INSERT INTO accounts
                    (iid, platform, platform_label, account_id, name, requires_checkin,
                     meta_keys, sort_order, created_at, updated_at)
                 VALUES ('qoder:x', 'qoder', 'Qoder', 'x', '扫描出来的', 1, '[]', 0, 0, 0)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let report = verify(&db, &data).unwrap();
        assert!(
            report.ok,
            "2.0 侧多 1 个扫描账号不算迁移缺陷：{}",
            report.describe()
        );
        assert_eq!(report.accounts, CountPair { v1: 2, v2: 3 });

        // 少一行 → 立刻判失败，且能说清缺的是哪一项
        db.write(|conn| {
            conn.execute("DELETE FROM accounts WHERE iid = 'zcode:12754401'", [])?;
            Ok(())
        })
        .unwrap();
        let report = verify(&db, &data).unwrap();
        assert!(!report.ok);
        assert_eq!(report.missing.accounts, 1);
        assert_eq!(report.missing.sessions, 0);
    }

    /// 会话的 edition 会随客户端版本切换而变（v1 记 `intl`、2.0 按目录判成 `cn`），
    /// 只要 uuid 还在就不算丢数据。
    #[test]
    fn edition_drift_is_not_data_loss() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        build_fixture(&data);
        let db = imported_db(tmp.path(), &data);

        db.write(|conn| {
            conn.execute(
                "UPDATE session_index SET source_key = 'intl' WHERE session_id = 's1'",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        let report = verify(&db, &data).unwrap();
        assert!(report.ok, "换了个 edition 不算丢：{}", report.describe());
        assert_eq!(report.missing.sessions, 0);
    }

    /// 归档成功时要把目录记进 kv：重复归档的提示要靠它说清「上次那份在哪」。
    #[test]
    fn archive_records_dir_in_kv() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        build_fixture(&data);
        let db = imported_db(tmp.path(), &data);

        let archived = archive(&db, &data).unwrap();
        let raw = db
            .kv_get(KV_ARCHIVE_DIR)
            .unwrap()
            .expect("归档后必须记目录");
        assert_eq!(json_string(&raw), archived.archive_dir);
        assert!(
            short_dir(&archived.archive_dir).starts_with("data-v1-archive/"),
            "提示里只显示末两段：{}",
            short_dir(&archived.archive_dir)
        );
    }

    /// 归档过一次之后再点，必须拒绝且**不新建目录**——否则 data-v1-archive/ 会无限堆。
    #[test]
    fn second_archive_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        build_fixture(&data);
        let db = imported_db(tmp.path(), &data);

        let first = archive(&db, &data).unwrap();
        let err = archive(&db, &data).unwrap_err();
        assert_eq!(err.kind, crate::app::error::ErrorKind::AlreadyArchived);
        assert!(
            err.message.contains("已经归档过"),
            "提示要能直接给用户看：{}",
            err.message
        );
        assert!(
            err.message.contains(&short_dir(&first.archive_dir)),
            "提示要说清上次归档在哪：{}",
            err.message
        );

        let dirs = std::fs::read_dir(tmp.path().join("data-v1-archive"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .count();
        assert_eq!(dirs, 1, "重复归档不能新建目录");
    }

    #[test]
    fn mismatch_blocks_archive_and_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        build_fixture(&data);

        // 空库：2.0 侧什么都没有，v1 的三项全部悬空
        let db = Db::open_at(&tmp.path().join("app.db")).unwrap();
        let report = verify(&db, &data).unwrap();
        assert!(!report.ok);
        assert_eq!(report.accounts, CountPair { v1: 2, v2: 0 });
        assert_eq!(
            report.missing,
            Missing {
                accounts: 2,
                sessions: 3,
                ledger: 3
            }
        );

        let err = archive(&db, &data).unwrap_err();
        assert_eq!(err.kind, crate::app::error::ErrorKind::MigrationMismatch);
        assert!(
            !tmp.path().join("data-v1-archive").exists(),
            "不一致时不能创建归档目录"
        );
        assert_eq!(
            db.kv_get("migration_done").unwrap(),
            None,
            "不一致时不能写迁移标记"
        );
    }

    /// 没有 v1 数据的新机器：0 缺失不等于校验通过，否则会给空归档盖章。
    #[test]
    fn empty_v1_side_is_not_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let db = Db::open_at(&tmp.path().join("app.db")).unwrap();

        let report = verify(&db, &data).unwrap();
        assert!(!report.ok);
        assert_eq!(report.accounts, CountPair { v1: 0, v2: 0 });
        assert!(archive(&db, &data).is_err());
    }

    /// 真实数据集：只在开发机有意义（依赖本机 DPAPI 与仓库里的 `data/`）。
    ///
    /// 对真实 `data/` **只读**：v1 侧只读那几个文件，2.0 侧读的是 `app.db` 的临时副本
    /// （WAL 三件一起拷，否则读到的是旧快照）。跑法：
    /// `cargo test --lib migrate::verify -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn verify_real_dataset() {
        let data = PathBuf::from("D:/AI Manager/data");
        if !data.join("accounts.dat").exists() {
            eprintln!("跳过：本机没有 v1 数据目录");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        // 只拷 db + wal：连 -shm 一起拷会带着一份过期的读标记，SQLite 会因此
        // 跳过 WAL 里的新帧，读出来是「上次 checkpoint 时」的旧快照。
        for name in ["app.db", "app.db-wal"] {
            let src = data.join(name);
            if src.is_file() {
                std::fs::copy(&src, tmp.path().join(name)).unwrap();
            }
        }
        let db = Db::open_at(&tmp.path().join("app.db")).unwrap();
        let report = verify(&db, &data).unwrap();
        println!("真实数据校验：{} → ok={}", report.describe(), report.ok);
    }
}
