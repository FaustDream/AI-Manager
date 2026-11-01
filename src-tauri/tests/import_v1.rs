//! v1 数据导入的验收测试。
//!
//! 两组：
//!   - 合成数据集：确定性强，断言计数、幂等、v1 目录不被改动
//!   - 真实数据集（`#[ignore]`）：用本机 `data/` 跑一次，验证 DPAPI 与真实结构

use ai_manager::app::db::Db;
use ai_manager::app::migrate::import_v1;
use ai_manager::app::secret;
use base64::Engine;
use std::path::{Path, PathBuf};

fn write(dir: &Path, name: &str, content: impl AsRef<[u8]>) {
    std::fs::write(dir.join(name), content).unwrap();
}

/// 造一份最小但结构完整的 v1 数据目录。
fn build_fixture(dir: &Path) {
    // 账号库：整包 base64(DPAPI(json))
    let accounts = serde_json::json!([
        {
            "platform": "workbuddy",
            "account_id": "5af46132-29b2-49a0-8a10-51eb6eff5e0c@intl",
            "name": "国际版账号",
            "token": "tk-should-not-leak",
            "refresh_token": "rt-should-not-leak",
            "expires_at": "2026-12-01T00:00:00+08:00",
            "requires_checkin": true,
            "meta": { "refresh_token": "rt-should-not-leak", "uid": "u1", "email": "a@b.c" }
        },
        {
            "platform": "zcode",
            "account_id": "12754401",
            "name": "Z 账号",
            "token": "tk2",
            "requires_checkin": false,
            "meta": { "credentials": "secret" }
        }
    ]);
    let plain = serde_json::to_vec(&accounts).unwrap();
    let blob = secret::protect(&plain).expect("DPAPI 加密应成功");
    let encoded = base64::engine::general_purpose::STANDARD.encode(blob);
    write(dir, "accounts.dat", encoded);

    write(
        dir,
        "checked.json",
        r#"{"workbuddy:5af46132-29b2-49a0-8a10-51eb6eff5e0c@intl":"2026-10-30T11:06:25"}"#,
    );
    write(
        dir,
        "daily.json",
        r#"{"date":"2026-10-30","done":["workbuddy:5af46132-29b2-49a0-8a10-51eb6eff5e0c@intl","qoder:01a0b289-3dd7-7260-ae98-eb52eefca9d2"],"last_round":"2026-10-30T21:28:44","last_result":"同日轮询 · 10 个账号 · 无需签到"}"#,
    );
    write(
        dir,
        "credits.json",
        r#"{"qoder:01a0b289-3dd7-7260-ae98-eb52eefca9d2":[{"amount":100.0,"granted":"2026-10-27T03:12:46.212524+00:00","expires":"2026-11-26T03:12:46.212524+00:00","key":"qoder:01a0:2026-10-27 11:12:46:100"}]}"#,
    );
    write(
        dir,
        "credit_history.json",
        r#"{"workbuddy:5af46132-29b2-49a0-8a10-51eb6eff5e0c@intl":{"2026-10-29":0.94,"2026-10-30":0.0}}"#,
    );
    write(
        dir,
        "session_index.json",
        r#"{"version":1,"updated_at":"2026-10-30T20:00:00","editions":{
            "cn":{"sessions":{"s1":{"origin":"o1","refs":["r1"],"cwd":"C:/w","slug":"s1","title":"标题一","last":"2026-10-30T19:00:00","seen_at":"2026-10-30T19:00:00"}}},
            "intl":{"sessions":{"s2":{"origin":"o2","refs":[],"cwd":"C:/w2","slug":"s2","title":"标题二","last":"2026-10-29T10:00:00","seen_at":"2026-10-29T10:00:00"}}}
        }}"#,
    );
    write(
        dir,
        "hub.json",
        r#"{"version":2,"library":"C:/lib","auto_sync":true,"default_targets":["qoder_cn","trae_cn"],
            "skills":{"skill-a":{"targets":["qoder_cn"],"deployed":{"qoder_cn":{"digest":"abc"}}}},
            "mcp":{"mcp-b":{"config":{"command":"node"},"targets":["trae_cn"],"deployed":{},"preserve":true}}}"#,
    );
    write(
        dir,
        "config.json",
        r#"{"theme":"dark","account_order":["zcode:12754401","workbuddy:5af46132-29b2-49a0-8a10-51eb6eff5e0c@intl"],"session_opts":{"page":50}}"#,
    );
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

#[test]
fn imports_full_v1_dataset() {
    let tmp = tempfile::TempDir::new().unwrap();
    let v1 = tmp.path().join("data");
    std::fs::create_dir_all(&v1).unwrap();
    build_fixture(&v1);
    let before = snapshot(&v1);

    let db = Db::open_at(&tmp.path().join("app.db")).unwrap();
    let report = import_v1(&db, &v1).expect("导入应成功");

    assert_eq!(report.accounts, 2);
    assert_eq!(report.sessions, 2, "cn + intl 各一条");
    assert_eq!(report.skills, 1);
    assert_eq!(report.mcp, 1);
    assert_eq!(report.credit_lots, 1);
    assert_eq!(report.credit_snapshots, 2);
    assert_eq!(report.checkins, 1);
    assert_eq!(report.daily_marks, 2);
    assert_eq!(
        report.orphan_ledger, 2,
        "qoder 在 daily 与 credits 各一条记录暂无账号行"
    );

    // v1 目录必须字节级不变
    assert_eq!(snapshot(&v1), before, "导入器绝不能修改 v1 的 data/");
    assert_eq!(db.migrated_from().unwrap().as_deref(), Some("v1.4.0"));
}

#[test]
fn keeps_intl_suffix_and_stores_credentials_encrypted() {
    let tmp = tempfile::TempDir::new().unwrap();
    let v1 = tmp.path().join("data");
    std::fs::create_dir_all(&v1).unwrap();
    build_fixture(&v1);
    let db = Db::open_at(&tmp.path().join("app.db")).unwrap();
    import_v1(&db, &v1).unwrap();

    let (meta_keys, blob, sort_order): (String, Vec<u8>, i64) = db
        .read(|conn| {
            let row = conn.query_row(
                "SELECT meta_keys, secret_blob, sort_order FROM accounts WHERE iid = ?1",
                ["workbuddy:5af46132-29b2-49a0-8a10-51eb6eff5e0c@intl"],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            Ok(row)
        })
        .expect("iid 必须带 @intl，与 v1 台账 key 一致");

    // meta 只存键名，值（refresh_token / email）绝不落明文列
    let keys: Vec<String> = serde_json::from_str(&meta_keys).unwrap();
    assert!(keys.contains(&"refresh_token".to_string()));
    assert!(
        !meta_keys.contains("rt-should-not-leak"),
        "meta 明文值不得入库"
    );
    assert!(!meta_keys.contains("a@b.c"));

    // 凭据在密文里，解密后能拿回原文
    let plain = secret::unprotect(&blob).unwrap();
    let text = String::from_utf8(plain).unwrap();
    assert!(text.contains("tk-should-not-leak"), "密文应可解回完整凭据");
    assert_eq!(sort_order, 1, "account_order 决定排序");

    // 整库不应出现明文 token
    let dump: String = db
        .read(|conn| {
            let mut stmt = conn.prepare("SELECT iid || name || meta_keys FROM accounts")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            Ok(rows.filter_map(|r| r.ok()).collect::<Vec<_>>().join("|"))
        })
        .unwrap();
    assert!(
        !dump.contains("should-not-leak"),
        "明文凭据不得出现在可查询列"
    );
}

#[test]
fn second_import_is_rejected() {
    let tmp = tempfile::TempDir::new().unwrap();
    let v1 = tmp.path().join("data");
    std::fs::create_dir_all(&v1).unwrap();
    build_fixture(&v1);
    let db = Db::open_at(&tmp.path().join("app.db")).unwrap();

    import_v1(&db, &v1).unwrap();
    let err = import_v1(&db, &v1).expect_err("重复导入必须被拒");
    assert!(
        err.message.contains("已导入"),
        "错误信息应说清原因：{}",
        err.message
    );
}

#[test]
fn missing_files_are_skipped_not_fatal() {
    let tmp = tempfile::TempDir::new().unwrap();
    let v1 = tmp.path().join("data");
    std::fs::create_dir_all(&v1).unwrap();
    // 只放一个 config.json，其余全部缺失
    write(&v1, "config.json", r#"{"theme":"light"}"#);

    let db = Db::open_at(&tmp.path().join("app.db")).unwrap();
    let report = import_v1(&db, &v1).expect("缺文件不应中断导入");
    assert_eq!(report.accounts, 0);
    assert!(
        report.warnings.iter().any(|w| w.contains("accounts.dat")),
        "缺失要有明确提示"
    );
    assert_eq!(
        db.kv_get("config.theme").unwrap().as_deref(),
        Some("\"light\"")
    );
}

/// 真实数据集：只在开发机有意义（依赖本机 DPAPI 与 v1 目录）。
/// 跑法：`cargo test --test import_v1 -- --ignored --nocapture`
#[test]
#[ignore]
fn imports_real_v1_dataset() {
    let data = PathBuf::from("D:/AI Manager/data");
    if !data.exists() {
        eprintln!("跳过：本机没有 v1 数据目录");
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let db = Db::open_at(&tmp.path().join("app.db")).unwrap();
    let report = import_v1(&db, &data).expect("真实数据导入应成功");
    println!("{}", report.summary());
    println!(
        "孤立台账 {}/ 警告 {:?}",
        report.orphan_ledger, report.warnings
    );

    assert!(report.accounts >= 7, "账号库实测 7 个");
    assert!(report.sessions >= 100, "会话索引实测 103 条");
    assert_eq!(report.skills, 6);
    assert_eq!(report.mcp, 4);
}
