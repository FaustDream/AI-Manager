//! 账号仓储。所有 SQL 只在这一层出现，业务层拿到的是类型化数据。
//!
//! **凭据边界**：`secret_blob` 只有在 `secret_of(iid)` 时才会解密，而且一次只解一个。
//! 列表、状态、导出等路径都拿不到明文 token——`AccountView` 里也没有这个字段。

use super::view::{AccountIdentity, AccountView};
use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::secret;
use rusqlite::OptionalExtension;
use std::collections::BTreeMap;

/// 账号别名：用户自己起的叫法，和平台昵称分开放。
/// 放在 kv 而不是 `accounts` 表：它是纯展示配置，重扫不该碰，也避免为它加列。
const ALIAS_KV: &str = "account.aliases";

/// 账号标签：iid → 字符串数组。用来筛选与批量操作（工作号 / 私人号 / 待处理…）。
/// 和别名一样放 kv：纯展示配置，重扫不该碰，也不值得为它加列。
const TAGS_KV: &str = "account.tags";

/// 账号备注：iid → 一句话。写「为什么停用」「这个号绑定在哪台机器」这类上下文。
const NOTES_KV: &str = "account.notes";

/// 标签上限：超过就是标签体系出了问题，不是账号需要更多标签。
const MAX_TAGS: usize = 8;
const MAX_TAG_CHARS: usize = 16;
const MAX_NOTE_CHARS: usize = 200;

/// 从 `accounts` 表读出全部账号（不解密凭据）。
pub fn list(db: &Db) -> Result<Vec<AccountView>> {
    let mut out = db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT iid, platform, platform_label, name, expires_at, source,
                    requires_checkin, secret_blob IS NOT NULL, sort_order,
                    enabled, last_ok_at, last_fail_at, last_fail_kind
             FROM accounts
             ORDER BY sort_order, iid",
        )?;
        // 列序：0 iid · 1 platform · 2 label · 3 name · 4 expires_at
        //      5 source · 6 requires_checkin · 7 has_secret · 8 sort_order
        //      9 enabled · 10 last_ok_at · 11 last_fail_at · 12 last_fail_kind
        let rows = stmt.query_map([], |r| {
            Ok(AccountView {
                iid: r.get(0)?,
                platform: r.get(1)?,
                platform_label: r.get(2)?,
                name: r.get(3)?,
                alias: None,
                tags: Vec::new(),
                note: None,
                expires_at: r.get(4)?,
                source: r.get(5)?,
                requires_checkin: r.get::<_, i64>(6)? != 0,
                has_secret: r.get::<_, i64>(7)? != 0,
                sort_order: r.get(8)?,
                enabled: r.get::<_, i64>(9)? != 0,
                last_ok_at: r.get(10)?,
                last_fail_at: r.get(11)?,
                last_fail_kind: r.get(12)?,
                checked_in: None,
                checked_at: None,
                balance: None,
                quota: Default::default(),
                quota_at: None,
                lot_expires_at: None,
                lot_amount: None,
                next_at: None,
                error: None,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    })?;
    // 别名与标签在 db.read 之外读：同一个读连接再加锁会自锁
    let aliases = alias_map(db);
    let tags = tags_map(db);
    let notes = notes_map(db);
    for row in &mut out {
        row.alias = aliases.get(&row.iid).cloned();
        row.tags = tags.get(&row.iid).cloned().unwrap_or_default();
        row.note = notes.get(&row.iid).cloned();
    }
    Ok(out)
}

fn alias_map(db: &Db) -> BTreeMap<String, String> {
    db.kv_get(ALIAS_KV)
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str::<BTreeMap<String, String>>(&text).ok())
        .unwrap_or_default()
}

/// 设置别名；空字符串表示恢复平台昵称。返回最终保存的值。
/// 只校验长度，不限制内容——用户自己的叫法没有格式可循。
pub fn set_alias(db: &Db, iid: &str, alias: &str) -> Result<String> {
    let trimmed = alias.trim();
    if trimmed.chars().count() > 40 {
        return Err(AppError::new(ErrorKind::Invalid, "别名最多 40 个字符"));
    }
    if identity(db, iid)?.is_none() {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "账号不存在，可能已被删除",
        ));
    }
    let mut aliases = alias_map(db);
    if trimmed.is_empty() {
        aliases.remove(iid);
    } else {
        aliases.insert(iid.to_string(), trimmed.to_string());
    }
    let text = serde_json::to_string(&aliases)
        .map_err(|e| AppError::new(ErrorKind::Invalid, format!("保存别名失败：{e}")))?;
    db.kv_set(ALIAS_KV, &text)?;
    Ok(trimmed.to_string())
}

fn tags_map(db: &Db) -> BTreeMap<String, Vec<String>> {
    db.kv_get(TAGS_KV)
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str::<BTreeMap<String, Vec<String>>>(&text).ok())
        .unwrap_or_default()
}

fn notes_map(db: &Db) -> BTreeMap<String, String> {
    db.kv_get(NOTES_KV)
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str::<BTreeMap<String, String>>(&text).ok())
        .unwrap_or_default()
}

/// 设置标签：去空白、去重、限长限数量。空数组表示清空。
pub fn set_tags(db: &Db, iid: &str, tags: &[String]) -> Result<Vec<String>> {
    if identity(db, iid)?.is_none() {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "账号不存在，可能已被删除",
        ));
    }
    let mut clean: Vec<String> = Vec::new();
    for tag in tags {
        let text = tag.trim();
        if text.is_empty() {
            continue;
        }
        if text.chars().count() > MAX_TAG_CHARS {
            return Err(AppError::new(
                ErrorKind::Invalid,
                format!("单个标签最多 {MAX_TAG_CHARS} 个字符"),
            ));
        }
        if !clean.iter().any(|item| item == text) {
            clean.push(text.to_string());
        }
    }
    if clean.len() > MAX_TAGS {
        return Err(AppError::new(
            ErrorKind::Invalid,
            format!("一个账号最多 {MAX_TAGS} 个标签"),
        ));
    }
    let mut all = tags_map(db);
    if clean.is_empty() {
        all.remove(iid);
    } else {
        all.insert(iid.to_string(), clean.clone());
    }
    let text = serde_json::to_string(&all)
        .map_err(|e| AppError::new(ErrorKind::Invalid, format!("保存标签失败：{e}")))?;
    db.kv_set(TAGS_KV, &text)?;
    Ok(clean)
}

/// 设置备注；空字符串表示清空。
pub fn set_note(db: &Db, iid: &str, note: &str) -> Result<String> {
    if identity(db, iid)?.is_none() {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "账号不存在，可能已被删除",
        ));
    }
    let trimmed = note.trim();
    if trimmed.chars().count() > MAX_NOTE_CHARS {
        return Err(AppError::new(
            ErrorKind::Invalid,
            format!("备注最多 {MAX_NOTE_CHARS} 个字符"),
        ));
    }
    let mut all = notes_map(db);
    if trimmed.is_empty() {
        all.remove(iid);
    } else {
        all.insert(iid.to_string(), trimmed.to_string());
    }
    let text = serde_json::to_string(&all)
        .map_err(|e| AppError::new(ErrorKind::Invalid, format!("保存备注失败：{e}")))?;
    db.kv_set(NOTES_KV, &text)?;
    Ok(trimmed.to_string())
}

/// 全部 iid（迁移校验用：只要身份，不取整行）。
pub fn iids(db: &Db) -> Result<Vec<String>> {
    db.read(|conn| {
        let mut stmt = conn.prepare("SELECT iid FROM accounts ORDER BY iid")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    })
}

/// 单个账号的身份信息（重扫时用）。
pub fn identity(db: &Db, iid: &str) -> Result<Option<AccountIdentity>> {
    db.read(|conn| {
        let row = conn
            .query_row(
                "SELECT iid, platform, platform_label, account_id, name, expires_at, source,
                        requires_checkin, secret_blob IS NOT NULL
                 FROM accounts WHERE iid = ?1",
                [iid],
                |r| {
                    Ok(AccountIdentity {
                        iid: r.get(0)?,
                        platform: r.get(1)?,
                        platform_label: r.get(2)?,
                        account_id: r.get(3)?,
                        name: r.get(4)?,
                        expires_at: r.get(5)?,
                        source: r.get(6)?,
                        requires_checkin: r.get::<_, i64>(7)? != 0,
                        has_secret: r.get::<_, i64>(8)? != 0,
                    })
                },
            )
            .optional()?;
        Ok(row)
    })
}

/// 写入身份字段。已存在则更新身份，**不动任何异步展示字段**（那些本来也不入库）。
pub fn upsert_identity(db: &Db, identity: &AccountIdentity, now_ms: i64) -> Result<()> {
    db.write(|conn| {
        conn.execute(
            "INSERT INTO accounts
                (iid, platform, platform_label, account_id, name, expires_at, source,
                 requires_checkin, meta_keys, secret_blob, sort_order, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, '[]', NULL,
                     (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM accounts), ?9, ?9)
             ON CONFLICT(iid) DO UPDATE SET
                 platform         = excluded.platform,
                 platform_label   = excluded.platform_label,
                 account_id       = excluded.account_id,
                 name             = excluded.name,
                 -- 扫描拿不到到期时间时不要抹掉已知值：'不知道' 不等于 '没有'
                 expires_at       = COALESCE(excluded.expires_at, accounts.expires_at),
                 -- 同理，来源为空不覆盖（否则重扫一次来源列就空了）
                 source           = COALESCE(NULLIF(excluded.source, ''), accounts.source),
                 requires_checkin = excluded.requires_checkin,
                 updated_at       = excluded.updated_at",
            rusqlite::params![
                identity.iid,
                identity.platform,
                identity.platform_label,
                identity.account_id,
                identity.name,
                identity.expires_at,
                identity.source,
                identity.requires_checkin as i64,
                now_ms,
            ],
        )?;
        Ok(())
    })
}

/// 启用/停用账号。停用只影响自动任务，手动单账号操作仍可执行。
pub fn set_enabled(db: &Db, iid: &str, enabled: bool) -> Result<bool> {
    db.write(|conn| {
        let changed = conn.execute(
            "UPDATE accounts SET enabled = ?2 WHERE iid = ?1",
            rusqlite::params![iid, if enabled { 1 } else { 0 }],
        )?;
        Ok(changed > 0)
    })
}

/// 最近一次拉取成功：清空失败标记，刷新成功时刻。
pub fn note_health_ok(db: &Db, iid: &str, now_ms: i64) -> Result<()> {
    db.write(|conn| {
        conn.execute(
            "UPDATE accounts SET last_ok_at = ?2, last_fail_at = NULL, last_fail_kind = NULL
             WHERE iid = ?1",
            rusqlite::params![iid, now_ms],
        )?;
        Ok(())
    })
}

/// 清掉失败标记（重新登录成功后调用）。
///
/// 换了新凭据就等于重新开始，旧的失败记录继续挂着只会让界面一直显示「查询失败」。
/// **不写 `last_ok_at`**：那个字段说的是「最近一次成功拉取」，而重新登录不是拉取，
/// 拿登录时刻冒充成功时刻会掩盖真实的查询状况。
pub fn clear_health_fail(db: &Db, iid: &str) -> Result<()> {
    db.write(|conn| {
        conn.execute(
            "UPDATE accounts SET last_fail_at = NULL, last_fail_kind = NULL WHERE iid = ?1",
            [iid],
        )?;
        Ok(())
    })
}

/// 最近一次拉取失败：记录时刻与错误类别；不清成功时刻，界面两者都要。
pub fn note_health_fail(db: &Db, iid: &str, kind: &str, now_ms: i64) -> Result<()> {
    db.write(|conn| {
        conn.execute(
            "UPDATE accounts SET last_fail_at = ?2, last_fail_kind = ?3 WHERE iid = ?1",
            rusqlite::params![iid, now_ms, kind],
        )?;
        Ok(())
    })
}

/// 用凭据包里更权威的到期时间校正列表显示值。
///
/// 与 `set_secret` 分开：这条只碰到期时间，不碰密文——刷新账号状态时不该重写凭据。
/// 值没变就不写：到期时间是展示字段，重复刷新不必产生写盘。
pub fn set_expires_at(db: &Db, iid: &str, expires_at: i64) -> Result<()> {
    db.write(|conn| {
        conn.execute(
            "UPDATE accounts SET expires_at = ?2
             WHERE iid = ?1 AND (expires_at IS NULL OR expires_at <> ?2)",
            rusqlite::params![iid, expires_at],
        )?;
        Ok(())
    })
}

/// 更新登录态（切换账号 / 重新登录后写入）。凭据单独加密存放。
pub fn set_secret(db: &Db, iid: &str, plain: &[u8], expires_at: Option<i64>) -> Result<()> {
    let blob = secret::protect(plain)?;
    db.write(|conn| {
        conn.execute(
            "UPDATE accounts SET secret_blob = ?2, expires_at = COALESCE(?3, expires_at) WHERE iid = ?1",
            rusqlite::params![iid, blob, expires_at],
        )?;
        Ok(())
    })
}

/// **唯一**的凭据出口：一次只解一个账号。
pub fn secret_of(db: &Db, iid: &str) -> Result<Option<Vec<u8>>> {
    let blob = db.read(|conn| {
        let blob: Option<Vec<u8>> = conn
            .query_row(
                "SELECT secret_blob FROM accounts WHERE iid = ?1",
                [iid],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(blob)
    })?;
    match blob {
        Some(blob) => Ok(Some(secret::unprotect(&blob)?)),
        None => Ok(None),
    }
}

pub fn delete(db: &Db, iid: &str) -> Result<bool> {
    db.write(|conn| {
        let affected = conn.execute("DELETE FROM accounts WHERE iid = ?1", [iid])?;
        Ok(affected > 0)
    })
}

/// 拖动排序：一次事务写完全部顺序。
pub fn reorder(db: &Db, iids: &[String]) -> Result<()> {
    db.write(|conn| {
        let tx = conn.unchecked_transaction()?;
        for (index, iid) in iids.iter().enumerate() {
            tx.execute(
                "UPDATE accounts SET sort_order = ?2 WHERE iid = ?1",
                rusqlite::params![iid, index as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, Db) {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        (dir, db)
    }

    fn identity(iid: &str, name: &str) -> AccountIdentity {
        AccountIdentity {
            iid: iid.into(),
            platform: iid.split(':').next().unwrap().into(),
            platform_label: "Test".into(),
            account_id: iid.split(':').nth(1).unwrap().into(),
            name: name.into(),
            requires_checkin: true,
            ..Default::default()
        }
    }

    #[test]
    fn upsert_keeps_secret_and_created_at() {
        let (_dir, db) = setup();
        upsert_identity(&db, &identity("qoder:1", "旧"), 1000).unwrap();
        set_secret(&db, "qoder:1", b"tok", Some(9_999)).unwrap();

        upsert_identity(&db, &identity("qoder:1", "新"), 2000).unwrap();

        let view = list(&db).unwrap().remove(0);
        assert_eq!(view.name, "新");
        assert!(view.has_secret, "重扫不得清掉已有凭据");
        assert_eq!(view.expires_at, Some(9_999));
        assert_eq!(
            secret_of(&db, "qoder:1").unwrap().as_deref(),
            Some(&b"tok"[..])
        );
    }

    #[test]
    fn new_rows_append_in_scan_order() {
        let (_dir, db) = setup();
        upsert_identity(&db, &identity("a:1", "A"), 1).unwrap();
        upsert_identity(&db, &identity("b:2", "B"), 1).unwrap();
        let rows = list(&db).unwrap();
        assert_eq!(
            rows.iter().map(|r| r.sort_order).collect::<Vec<_>>(),
            vec![0, 1]
        );
    }

    #[test]
    fn reorder_persists() {
        let (_dir, db) = setup();
        for (i, iid) in ["a:1", "b:2", "c:3"].iter().enumerate() {
            upsert_identity(&db, &identity(iid, "x"), i as i64).unwrap();
        }
        reorder(&db, &["c:3".into(), "a:1".into(), "b:2".into()]).unwrap();
        let rows = list(&db).unwrap();
        assert_eq!(
            rows.iter().map(|r| r.iid.as_str()).collect::<Vec<_>>(),
            vec!["c:3", "a:1", "b:2"]
        );
    }

    #[test]
    fn alias_persists_and_survives_rescan() {
        let (_dir, db) = setup();
        upsert_identity(&db, &identity("qoder:1", "旧昵称"), 1).unwrap();

        assert_eq!(set_alias(&db, "qoder:1", "  工作号  ").unwrap(), "工作号");
        // 重扫只更新身份字段：平台昵称变了，手工别名不能被冲掉
        upsert_identity(&db, &identity("qoder:1", "平台新昵称"), 2).unwrap();
        let row = list(&db).unwrap().remove(0);
        assert_eq!(row.name, "平台新昵称");
        assert_eq!(row.alias.as_deref(), Some("工作号"));
        assert_eq!(row.display_name(), "工作号");

        set_alias(&db, "qoder:1", "").unwrap();
        let row = list(&db).unwrap().remove(0);
        assert!(row.alias.is_none(), "空字符串表示恢复平台昵称");
        assert_eq!(row.display_name(), "平台新昵称");
    }

    #[test]
    fn alias_rejects_unknown_account_and_long_text() {
        let (_dir, db) = setup();
        assert!(set_alias(&db, "qoder:none", "别名").is_err());
        upsert_identity(&db, &identity("qoder:1", "A"), 1).unwrap();
        let long = "名".repeat(41);
        assert!(set_alias(&db, "qoder:1", &long).is_err());
    }

    #[test]
    fn delete_reports_whether_removed() {
        let (_dir, db) = setup();
        upsert_identity(&db, &identity("a:1", "A"), 1).unwrap();
        assert!(delete(&db, "a:1").unwrap());
        assert!(!delete(&db, "a:1").unwrap());
    }
}
