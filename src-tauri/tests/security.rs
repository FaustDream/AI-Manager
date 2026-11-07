//! 安全：凭据密文落盘、失败证据零泄露（九类测试 · 安全 SEC-02/SEC-03）。
//!
//! 核心策略钉死两件事：
//! 1. secret 进库必须经 DPAPI，磁盘上任何字节（db / wal / 日志）都找不到明文；
//! 2. 平台失败的日志证据只含 `kind + HTTP 状态码`，响应原文（可能含 token）
//!    永远不进证据——`net.rs::json()` 与 Provider 的 ≥400 分支都按此约定裁剪，
//!    这里用「正文里埋标记 token」的 mock 端到端验证。

mod support;

use ai_manager::app::accounts::{repo, AccountIdentity};
use ai_manager::app::checkin::Service;
use ai_manager::app::db::Db;
use ai_manager::app::log::{self, Level};
use ai_manager::app::net::MockHttp;
use ai_manager::app::secret;
use std::sync::Arc;
use support::*;

const STATUS_URL: &str = "https://api.trae.cn/trae/api/v2/ug/checkin_credits/status";
/// 唯一性标记：日志/磁盘扫描按这个字节串找，避免误伤正常文案。
const TOKEN: &str = "SECMARK-tok-f6e5d4c3b2a1";

fn seed_trae(db: &Db) -> String {
    let iid = "trae:sec".to_string();
    let identity = AccountIdentity {
        iid: iid.clone(),
        platform: "trae".into(),
        platform_label: "Trae CN".into(),
        account_id: "u1".into(),
        name: "安全测试账号".into(),
        expires_at: None,
        source: "storage.json".into(),
        requires_checkin: true,
        has_secret: true,
    };
    repo::upsert_identity(db, &identity, 0).unwrap();
    iid
}

/// SEC-02 凭据只以 DPAPI 密文落盘；同用户可解密回读，磁盘字节无明文。
#[test]
fn credential_never_hits_disk_in_plaintext() {
    let paths = TestPaths::new();
    let db = Db::open_at(&paths.join("app.db")).unwrap();
    let iid = seed_trae(&db);

    let secret_json = format!(r#"{{"token":"{TOKEN}","expires_at":"","meta":{{}}}}"#);
    repo::set_secret(&db, &iid, secret_json.as_bytes(), None).unwrap();

    // 同用户解密回读一致（DPAPI 可用性 + blob 未损坏）
    assert_eq!(
        repo::secret_of(&db, &iid).unwrap().unwrap(),
        secret_json.as_bytes()
    );

    // 磁盘上（库 + WAL）找不到明文 token，也找不到任何 protect 输出的明文前缀
    for name in ["app.db", "app.db-wal", "app.db-shm"] {
        let path = paths.join(name);
        if !path.exists() {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            !bytes.windows(TOKEN.len()).any(|w| w == TOKEN.as_bytes()),
            "{name} 中出现了明文凭据！"
        );
    }
    // protect 输出本身也不等于明文
    let blob = secret::protect(secret_json.as_bytes()).unwrap();
    assert_ne!(blob, secret_json.as_bytes());
}

/// SEC-03 失败证据只含 kind + 状态码：响应原文（埋了标记 token）不进日志、不进错误、不进磁盘。
#[tokio::test]
async fn failure_evidence_redacts_response_body() {
    let paths = TestPaths::new();
    let logs = paths.join("logs");
    log::init_at(Level::Debug, &logs).unwrap();

    let db = Arc::new(Db::open_at(&paths.join("app.db")).unwrap());
    let iid = seed_trae(&db);
    let secret_json = format!(r#"{{"token":"{TOKEN}","expires_at":"","meta":{{}}}}"#);
    repo::set_secret(&db, &iid, secret_json.as_bytes(), None).unwrap();

    let http = Arc::new(MockHttp::new());
    // 场景一：HTTP 500 且响应正文埋 token；场景二：200 但正文不是 JSON 且埋 token
    let body_500 = format!(r#"{{"error":"upstream says {TOKEN} rejected"}}"#);
    let body_200 = format!("不是 JSON 的垃圾正文，{TOKEN} 埋在这里等泄露");
    http.on_sequence(STATUS_URL, vec![(500, &body_500), (200, &body_200)]);
    let clock = Arc::new(clock_at("2026-11-07T10:00:00+08:00"));
    let service = Service::new(db, http, clock);

    // 两次诊断都失败，错误与证据都不携带正文
    for _ in 0..2 {
        let err = service.diagnose(&iid).await.unwrap_err();
        assert!(
            !err.message.contains(TOKEN),
            "错误消息泄漏正文：{}",
            err.message
        );
        let evidence = err.evidence();
        assert!(!evidence.contains(TOKEN), "证据泄漏正文：{evidence}");
    }

    // 日志确实写了失败行（排除「没日志所以扫不到」的假阴性），再全目录扫 token
    let app_log = std::fs::read(logs.join("app.log")).unwrap();
    let text = String::from_utf8_lossy(&app_log);
    assert!(
        text.contains("checkin_diagnose"),
        "失败日志应存在，实际：{text}"
    );

    let mut scanned = 0;
    for entry in walk_files(&paths.join(".")) {
        let bytes = std::fs::read(&entry).unwrap();
        scanned += 1;
        assert!(
            !bytes.windows(TOKEN.len()).any(|w| w == TOKEN.as_bytes()),
            "{} 中出现了响应正文里的标记 token！",
            entry.display()
        );
    }
    println!("SEC-03 扫描磁盘文件 {scanned} 个，零 token 泄露");
}

/// 递归列出目录下所有文件（小目录，无需 walkdir 依赖）。
fn walk_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}
