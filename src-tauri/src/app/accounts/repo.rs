//! 账号仓储。所有 SQL 只在这一层出现，业务层拿到的是类型化数据。
//!
//! **凭据边界**：`secret_blob` 只有在 `secret_of(iid)` 时才会解密，而且一次只解一个。
//! 列表、状态、导出等路径都拿不到明文 token——`AccountView` 里也没有这个字段。

use super::view::{AccountIdentity, AccountView};
use crate::app::db::Db;
use crate::app::error::Result;
use crate::app::secret;
use rusqlite::OptionalExtension;

/// 从 `accounts` 表读出全部账号（不解密凭据）。
pub fn list(db: &Db) -> Result<Vec<AccountView>> {
    db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT iid, platform, platform_label, name, expires_at, source,
                    requires_checkin, secret_blob IS NOT NULL, sort_order
             FROM accounts
             ORDER BY sort_order, iid",
        )?;
        // 列序：0 iid · 1 platform · 2 label · 3 name · 4 expires_at
        //      5 source · 6 requires_checkin · 7 has_secret · 8 sort_order
        let rows = stmt.query_map([], |r| {
            Ok(AccountView {
                iid: r.get(0)?,
                platform: r.get(1)?,
                platform_label: r.get(2)?,
                name: r.get(3)?,
                expires_at: r.get(4)?,
                source: r.get(5)?,
                requires_checkin: r.get::<_, i64>(6)? != 0,
                has_secret: r.get::<_, i64>(7)? != 0,
                sort_order: r.get(8)?,
                checked_in: None,
                checked_at: None,
                balance: None,
                quota: Default::default(),
                next_at: None,
                error: None,
            })
        })?;
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
            .query_row("SELECT secret_blob FROM accounts WHERE iid = ?1", [iid], |r| r.get(0))
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
        assert_eq!(secret_of(&db, "qoder:1").unwrap().as_deref(), Some(&b"tok"[..]));
    }

    #[test]
    fn new_rows_append_in_scan_order() {
        let (_dir, db) = setup();
        upsert_identity(&db, &identity("a:1", "A"), 1).unwrap();
        upsert_identity(&db, &identity("b:2", "B"), 1).unwrap();
        let rows = list(&db).unwrap();
        assert_eq!(rows.iter().map(|r| r.sort_order).collect::<Vec<_>>(), vec![0, 1]);
    }

    #[test]
    fn reorder_persists() {
        let (_dir, db) = setup();
        for (i, iid) in ["a:1", "b:2", "c:3"].iter().enumerate() {
            upsert_identity(&db, &identity(iid, "x"), i as i64).unwrap();
        }
        reorder(&db, &["c:3".into(), "a:1".into(), "b:2".into()]).unwrap();
        let rows = list(&db).unwrap();
        assert_eq!(rows.iter().map(|r| r.iid.as_str()).collect::<Vec<_>>(), vec!["c:3", "a:1", "b:2"]);
    }

    #[test]
    fn delete_reports_whether_removed() {
        let (_dir, db) = setup();
        upsert_identity(&db, &identity("a:1", "A"), 1).unwrap();
        assert!(delete(&db, "a:1").unwrap());
        assert!(!delete(&db, "a:1").unwrap());
    }
}
