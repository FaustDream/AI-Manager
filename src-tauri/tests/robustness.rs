//! 接口健壮性：错误路径、边界入参、SQL 注入字面量（九类测试 · 接口/安全）。
//!
//! 所有用例走 tempfile 独立库（CORE-03），绝不碰真实 data/。
//! 注入串作为**数据**经参数化 SQL 写入，断言按字面存取且表结构完好——
//! 钉死「用户输入永远只是参数」这条约定。

mod support;

use ai_manager::app::accounts::{repo, AccountIdentity};
use ai_manager::app::db::Db;
use ai_manager::app::error::ErrorKind;
use support::*;

fn identity_with(iid: &str, name: &str) -> AccountIdentity {
    AccountIdentity {
        iid: iid.to_string(),
        platform: "trae".into(),
        platform_label: "Trae CN".into(),
        account_id: "u1".into(),
        name: name.to_string(),
        expires_at: None,
        source: "storage.json".into(),
        requires_checkin: true,
        has_secret: false,
    }
}

fn seed(db: &Db, iid: &str) {
    repo::upsert_identity(db, &identity_with(iid, "测试账号"), 0).unwrap();
}

/// A-03 对不存在的 iid，每个接口都必须给出干净结果（空/False/Err），绝不 panic。
#[test]
fn nonexistent_iid_returns_clean_errors() {
    let paths = TestPaths::new();
    let db = Db::open_at(&paths.join("app.db")).unwrap();
    let ghost = "trae:ghost";

    assert!(repo::identity(&db, ghost).unwrap().is_none());
    assert!(
        !repo::delete(&db, ghost).unwrap(),
        "删除不存在的账号应返回 false"
    );
    assert!(!repo::set_enabled(&db, ghost, true).unwrap());
    assert!(repo::secret_of(&db, ghost).unwrap().is_none());

    let err = repo::set_alias(&db, ghost, "别名").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Invalid);
    assert!(err.message.contains("不存在"), "实际文案：{}", err.message);

    let err = repo::set_note(&db, ghost, "备注").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Invalid);
}

/// A-04 边界入参：限长接口在边界值上接受/拒绝一致，超限报 Invalid 而非 panic；
/// 无限长接口（身份字段）原样存取。
#[test]
fn boundary_inputs_are_accepted_or_rejected_consistently() {
    let paths = TestPaths::new();
    let db = Db::open_at(&paths.join("app.db")).unwrap();
    let iid = "trae:edge";
    seed(&db, iid);

    // 别名：40 字符是上限（含 CJK/emoji 都按 char 数）
    let alias_40: String = "界".repeat(40);
    assert_eq!(repo::set_alias(&db, iid, &alias_40).unwrap(), alias_40);
    let alias_41: String = "界".repeat(41);
    let err = repo::set_alias(&db, iid, &alias_41).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Invalid);
    assert!(err.message.contains("40"), "实际文案：{}", err.message);

    let emoji_20: String = "🦀".repeat(20);
    assert_eq!(repo::set_alias(&db, iid, &emoji_20).unwrap(), emoji_20);
    let emoji_40: String = "🦀".repeat(40);
    assert_eq!(repo::set_alias(&db, iid, &emoji_40).unwrap(), emoji_40);
    let emoji_41: String = "🦀".repeat(41);
    assert!(repo::set_alias(&db, iid, &emoji_41).is_err());

    // 空串 = 清空别名，属合法操作
    assert_eq!(repo::set_alias(&db, iid, "  ").unwrap(), "");

    // 备注：200 字符上限
    let note_200: String = "注".repeat(200);
    assert_eq!(repo::set_note(&db, iid, &note_200).unwrap(), note_200);
    let note_201: String = "注".repeat(201);
    let err = repo::set_note(&db, iid, &note_201).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Invalid);
    assert!(err.message.contains("200"), "实际文案：{}", err.message);

    // 身份字段无内容限制：10KB 昵称与 emoji iid 原样存取
    let long_name = "名".repeat(5000);
    let emoji_iid = "trae:🦀-账号";
    repo::upsert_identity(&db, &identity_with(emoji_iid, &long_name), 0).unwrap();
    let row = repo::identity(&db, emoji_iid)
        .unwrap()
        .expect("emoji iid 应可取回");
    assert_eq!(row.name, long_name);
}

/// A-05 / SEC-01 注入串作为普通数据写入后：按字面存取、表结构完好、行数不变。
#[test]
fn sql_injection_payloads_stay_literal() {
    let paths = TestPaths::new();
    let db = Db::open_at(&paths.join("app.db")).unwrap();
    let iid = "trae:inject";
    seed(&db, iid);

    // 别名通道（≤40 字）与备注通道（≤200 字）各埋一组注入串；
    // 参数化 SQL 必须把它们当普通数据按字面接受。
    let alias_payloads = [
        "x'; DROP TABLE accounts; --",
        "x' OR '1'='1' --",
        "admin'--",
        "x');INSERT INTO kv VALUES('pwn','1')--",
        "\u{0001}'; DELETE FROM kv WHERE '1'='1",
    ];
    let note_payloads = [
        "x\" || (SELECT group_concat(sql) FROM sqlite_master) --",
        "x'); INSERT INTO accounts(iid) VALUES('ghost'); --",
    ];
    let schema_before = db.schema_version().unwrap();

    for payload in alias_payloads {
        repo::set_alias(&db, iid, payload)
            .unwrap_or_else(|e| panic!("注入串「{payload}」是合法用户输入，应被接受：{e}"));
    }
    for payload in note_payloads {
        repo::set_note(&db, iid, payload)
            .unwrap_or_else(|e| panic!("注入串「{payload}」是合法用户输入，应被接受：{e}"));
    }

    // 表结构与数据完好：accounts 还在、行数不变、schema 没动、没有 pwn 键
    assert_eq!(db.schema_version().unwrap(), schema_before);
    assert_eq!(repo::iids(&db).unwrap().len(), 1);
    assert!(!db.kv_entries().unwrap().iter().any(|(k, _)| k == "pwn"));

    // 注入串按字面读回（list 走完整查询链路），幽灵账号没有被插入
    let views = repo::list(&db).unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(
        views[0].alias.as_deref(),
        Some(*alias_payloads.last().unwrap())
    );
    assert_eq!(
        views[0].note.as_deref(),
        Some(*note_payloads.last().unwrap())
    );
}
