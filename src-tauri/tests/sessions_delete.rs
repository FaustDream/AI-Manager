//! 删除 → 回收站 → 恢复，走一遍完整链路。
//!
//! 这是整个 P4 里唯一会**动用户数据**的地方，所以断言不看返回值，看磁盘：
//! 文件是不是真的搬走了、库行是不是真的没了、恢复之后是不是原样回来。

mod support;

use ai_manager::app::clients::Roots;
use ai_manager::app::db::Db;
use ai_manager::app::sessions::{delete, preview, sources::wb, SqliteSessions};
use ai_manager::app::task::CancelToken;
use std::sync::Arc;
use support::*;

/// 一个测试里跑完 wb + codebuddy 两条链路：`data_dir` 的覆盖是进程级的，
/// 拆成两个用例会互相踩。
#[tokio::test]
async fn delete_moves_to_trash_and_restore_brings_it_back() {
    let paths = TestPaths::new();
    let trash_root = paths.join("trash");
    let roots = fake_roots(&paths);
    let db = Arc::new(Db::open_at(&paths.join("app.db")).unwrap());
    let iid_row = seed_workbuddy(&roots);
    let (cb_key, cb_dir) = seed_codebuddy(&roots);

    // ---- 扫描进索引（删除要用行 key，key 来自索引） ----
    let service = ai_manager::app::sessions::Service::new(db.clone(), roots.clone());
    service.scan(Arc::new(CancelToken::new())).await.unwrap();
    let page = service.page(Default::default(), None, 50).await.unwrap();
    assert_eq!(page.items.len(), 2, "两个来源各一条");

    let wb_key = page
        .items
        .iter()
        .find(|r| r.kind == "workbuddy")
        .map(|r| format!("{}|{}|{}|{}", r.kind, r.source_key, r.account, r.session_id))
        .unwrap();
    assert_eq!(wb_key.split('|').next_back(), Some(iid_row.as_str()));

    // ---- 删除两条 ----
    let report = delete::delete_into(
        &roots,
        &db,
        &[wb_key.clone(), cb_key.clone()],
        false,
        &trash_root,
        None,
    )
    .unwrap();
    assert_eq!(report.sessions, 2, "{:?}", report.errors);
    assert!(report.bytes > 0);
    assert!(
        std::path::Path::new(&report.dir).exists(),
        "批次目录要建出来"
    );

    // 正文文件不在了
    let wb_file = roots
        .workbuddy(false)
        .join("projects/demo")
        .join(format!("{iid_row}.jsonl"));
    assert!(!wb_file.exists(), "删除后正文不该留在原地");
    assert!(!cb_dir.exists(), "CodeBuddy 会话目录要整体搬走");

    // 库行真的删了
    let conn = rusqlite::Connection::open(roots.workbuddy(false).join("workbuddy.db")).unwrap();
    let left: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(left, 0, "sessions 行要删掉");

    // 索引里也不该再有
    let store = SqliteSessions::new(&db);
    assert_eq!(store.count().unwrap(), 0);

    // 清单写清楚了这批删了什么
    let manifest = delete::list_in(&trash_root)
        .into_iter()
        .next()
        .expect("应该有批次");
    assert_eq!(manifest.items.len(), 2);
    assert_eq!(manifest.tag, report.tag);

    // ---- 恢复 ----
    let restored = delete::restore_from(&roots, &report.tag, false, &trash_root).unwrap();
    assert_eq!(restored.restored, 2, "{:?}", restored.errors);
    assert!(wb_file.exists(), "正文要搬回原位");
    assert_eq!(
        std::fs::read_to_string(&wb_file).unwrap().lines().count(),
        2
    );
    assert!(cb_dir.exists(), "CodeBuddy 目录要回来");

    let conn = rusqlite::Connection::open(roots.workbuddy(false).join("workbuddy.db")).unwrap();
    let back: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE id = ?1",
            [&iid_row],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(back, 1, "sessions 行要原样插回去");

    // 恢复了就不该再留着批次目录（防止重复恢复插两遍）
    assert!(!std::path::Path::new(&report.dir).exists());
}

/// 删除之后重新扫描，索引里不留幽灵行。
#[tokio::test]
async fn rescan_after_delete_has_no_ghosts() {
    let paths = TestPaths::new();
    let trash_root = paths.join("trash");
    let roots = fake_roots(&paths);
    let db = Arc::new(Db::open_at(&paths.join("app.db")).unwrap());
    let sid = seed_workbuddy(&roots);

    let service = ai_manager::app::sessions::Service::new(db.clone(), roots.clone());
    service.scan(Arc::new(CancelToken::new())).await.unwrap();
    let key = format!("workbuddy|cn|uid-1|{sid}");
    delete::delete_into(&roots, &db, &[key], false, &trash_root, None).unwrap();
    // 删除时已经让来源失效，这里再扫一遍确认真没了
    service.scan(Arc::new(CancelToken::new())).await.unwrap();
    let page = service.page(Default::default(), None, 50).await.unwrap();
    assert!(page.items.is_empty(), "删掉的会话不该被重新扫回来");
}

fn fake_roots(paths: &TestPaths) -> Roots {
    Roots::at(
        paths.join("home"),
        paths.join("appdata"),
        paths.join("local"),
    )
}

fn seed_workbuddy(roots: &Roots) -> String {
    let root = roots.workbuddy(false);
    let projects = root.join("projects").join("demo");
    std::fs::create_dir_all(&projects).unwrap();
    let sid = "0f3a9c2d8b7e4a1f9c3d5e7b9a1c2d3e".to_string();
    std::fs::write(
        projects.join(format!("{sid}.jsonl")),
        "{\"type\":\"message\",\"role\":\"user\",\"content\":\"你好\"}\n{\"type\":\"message\",\"role\":\"assistant\",\"content\":\"在\"}\n",
    )
    .unwrap();
    let skeleton = root.join("storage").join("skeleton");
    std::fs::create_dir_all(&skeleton).unwrap();
    std::fs::write(
        skeleton.join("account-snapshot.json"),
        r#"{"primary":{"uid":"uid-1"}}"#,
    )
    .unwrap();

    let conn = rusqlite::Connection::open(root.join("workbuddy.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE sessions(
             id TEXT PRIMARY KEY, title TEXT, custom_title TEXT, cwd TEXT,
             user_id TEXT, transport TEXT, last_activity_at INTEGER,
             created_at INTEGER, deleted_at INTEGER);
         CREATE TABLE session_usage(session_id TEXT PRIMARY KEY, tokens INTEGER);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions VALUES (?1,'要删的会话',NULL,'D:/work','uid-1','local',1760000000000,1760000000000,NULL)",
        [&sid],
    )
    .unwrap();
    conn.execute("INSERT INTO session_usage VALUES (?1, 42)", [&sid])
        .unwrap();
    sid
}

fn seed_codebuddy(roots: &Roots) -> (String, std::path::PathBuf) {
    let uid = "8d1f0a2c-1b3c-4d5e-8f90-a1b2c3d4e5f6";
    let ws = "a850269b796011ab7b1eb38fe878bc5f";
    let cid = "01bdbac66eb3444db066c41785c78a12";
    let history = roots
        .codebuddy()
        .join(uid)
        .join("CodeBuddyIDE")
        .join(uid)
        .join("history")
        .join(ws);
    let dir = history.join(cid);
    let messages = dir.join("messages");
    std::fs::create_dir_all(&messages).unwrap();
    std::fs::write(
        messages.join("m1.json"),
        r#"{"role":"user","message":"{\"content\":[{\"type\":\"text\",\"text\":\"第一条\"}]}"}"#,
    )
    .unwrap();
    std::fs::write(
        history.join("index.json"),
        format!(r#"{{"conversations":[{{"id":"{cid}","name":"CB 会话","createdAt":"2026-10-01T10:00:00Z","lastMessageAt":"2026-10-02T10:00:00Z"}}]}}"#),
    )
    .unwrap();
    (format!("codebuddy|CodeBuddyIDE|{uid}|{cid}"), dir)
}

/// 预览在这条链路上也要还能用（删除前先看得到内容）。
#[tokio::test]
async fn preview_reads_workbuddy_body() {
    let paths = TestPaths::new();
    let roots = fake_roots(&paths);
    let sid = seed_workbuddy(&roots);
    let target = preview::parse_key(&format!("workbuddy|cn|uid-1|{sid}")).unwrap();
    let result = preview::load(&roots, &target);
    assert_eq!(result.total, 2);
    assert_eq!(result.messages[0].text, "你好");
    assert!(wb::editions(&roots).len() == 1);
}

/// ZCode 的删除最容易被漏：正文在 `part` 表，按 `message_id` 关联而不是 `session_id`。
/// 这条用例把它钉住——删完 part 不能留孤儿行，恢复要能一行不差地插回去。
#[tokio::test]
async fn zcode_delete_and_restore_keeps_parts() {
    let paths = TestPaths::new();
    let trash_root = paths.join("trash");
    let roots = fake_roots(&paths);
    let db = Arc::new(Db::open_at(&paths.join("app.db")).unwrap());
    let sid = "session_1fef456b559223f9a8961b11c0a07b71";
    seed_zcode(&roots, sid);
    // 库行之外，ZCode 还给这条会话落了目录与 rollout 文件
    let artifacts = seed_zcode_artifacts(&roots, sid);

    let service = ai_manager::app::sessions::Service::new(db.clone(), roots.clone());
    service.scan(Arc::new(CancelToken::new())).await.unwrap();
    let page = service.page(Default::default(), None, 50).await.unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].msgs, Some(2), "两条消息");

    let key = format!("zcode|zcode||{sid}");
    let report = delete::delete_into(
        &roots,
        &db,
        std::slice::from_ref(&key),
        false,
        &trash_root,
        None,
    )
    .unwrap();
    assert_eq!(report.sessions, 1, "{:?}", report.errors);

    let db_path = roots.zcode().join("cli").join("db").join("db.sqlite");
    let counts = |path: &std::path::Path| {
        let conn = rusqlite::Connection::open(path).unwrap();
        let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        (
            count("SELECT COUNT(*) FROM session"),
            count("SELECT COUNT(*) FROM message"),
            count("SELECT COUNT(*) FROM part"),
        )
    };
    assert_eq!(
        counts(&db_path),
        (0, 0, 0),
        "会话、消息、片段都该删干净，不留孤儿行"
    );
    for path in &artifacts {
        assert!(!path.exists(), "{} 还留在 cli 目录里", path.display());
    }

    let restored = delete::restore_from(&roots, &report.tag, false, &trash_root).unwrap();
    assert_eq!(restored.restored, 1, "{:?}", restored.errors);
    assert_eq!(counts(&db_path), (1, 2, 2), "恢复要一行不差地插回去");
    for path in &artifacts {
        assert!(path.exists(), "{} 没搬回原位", path.display());
    }
}

/// ZCode 在 `cli/` 下给每条会话落的目录与 rollout 文件。
fn seed_zcode_artifacts(roots: &Roots, sid: &str) -> Vec<std::path::PathBuf> {
    let cli = roots.zcode().join("cli");
    let mut out = Vec::new();
    for rel in [format!("agents/{sid}"), format!("artifacts/{sid}")] {
        let dir = cli.join(rel);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("state.json"), "{}").unwrap();
        out.push(dir);
    }
    let rollout = cli.join("rollout").join(format!("model-io-{sid}.jsonl"));
    std::fs::create_dir_all(rollout.parent().unwrap()).unwrap();
    std::fs::write(&rollout, "{}\n").unwrap();
    out.push(rollout);
    out
}

fn seed_zcode(roots: &Roots, sid: &str) {
    let dir = roots.zcode().join("cli").join("db");
    std::fs::create_dir_all(&dir).unwrap();
    let conn = rusqlite::Connection::open(dir.join("db.sqlite")).unwrap();
    conn.execute_batch(
        "CREATE TABLE session(id TEXT PRIMARY KEY, title TEXT, directory TEXT,
             time_updated INTEGER, time_deleted INTEGER, time_archived INTEGER, parent_id TEXT);
         CREATE TABLE message(id TEXT PRIMARY KEY, session_id TEXT, data TEXT,
             sequence INTEGER, time_created INTEGER);
         CREATE TABLE part(id TEXT PRIMARY KEY, message_id TEXT, data TEXT, sequence INTEGER);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session VALUES (?1,'ZCode 会话','D:/work',1760000000000,NULL,0,NULL)",
        [sid],
    )
    .unwrap();
    for (index, role) in ["user", "assistant"].iter().enumerate() {
        let mid = format!("m{index}");
        conn.execute(
            "INSERT INTO message VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params![
                mid,
                sid,
                format!(r#"{{"role":"{role}"}}"#),
                index as i64,
                1760000000000i64
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part VALUES (?1,?2,?3,?4)",
            rusqlite::params![
                format!("p{index}"),
                mid,
                format!(r#"{{"type":"text","text":"第 {index} 条"}}"#),
                index as i64
            ],
        )
        .unwrap();
    }
}

/// 客户端已经清过库的会话（先软删、之后硬删）：索引里还留着这些行。
///
/// 这类行的「彻底删除」曾经必然失败——`delete_workbuddy` 找不到库行就报错，
/// 于是报告永远是「0 条」，那几行在列表里再也删不掉。现在要能清掉，
/// 而且只清这几行，不能把整个来源的列表一起清空。
#[tokio::test]
async fn purge_clears_rows_the_client_already_forgot() {
    let paths = TestPaths::new();
    let trash_root = paths.join("trash");
    let roots = fake_roots(&paths);
    let db = Arc::new(Db::open_at(&paths.join("app.db")).unwrap());
    let sid = seed_workbuddy(&roots);
    // 正文之外的侧车与日志：客户端会按会话 id 落三份，删完必须一起没
    let sidecars = seed_sidecars(&roots, &sid);
    // 第二条：客户端连正文都删干净了，只剩索引
    let gone = "11112222333344445555666677778888";
    {
        let conn = rusqlite::Connection::open(roots.workbuddy(false).join("workbuddy.db")).unwrap();
        conn.execute(
            "INSERT INTO sessions VALUES (?1,'客户端已清库',NULL,'D:/work','uid-1','local',1760000000000,1760000000000,NULL)",
            [gone],
        )
        .unwrap();
    }

    let service = ai_manager::app::sessions::Service::new(db.clone(), roots.clone());
    service.scan(Arc::new(CancelToken::new())).await.unwrap();
    assert_eq!(
        service
            .page(Default::default(), None, 50)
            .await
            .unwrap()
            .items
            .len(),
        2,
        "两条都要进索引"
    );

    // 客户端把这两行硬删了，索引还不知道
    {
        let conn = rusqlite::Connection::open(roots.workbuddy(false).join("workbuddy.db")).unwrap();
        conn.execute("DELETE FROM sessions", []).unwrap();
        conn.execute("DELETE FROM session_usage", []).unwrap();
    }

    let keys = [
        format!("workbuddy|cn|uid-1|{sid}"),
        format!("workbuddy|cn|uid-1|{gone}"),
    ];
    let report = delete::delete_into(&roots, &db, &keys, false, &trash_root, None).unwrap();
    assert!(
        report.errors.is_empty(),
        "客户端已清库不是失败：{:?}",
        report.errors
    );
    assert_eq!(report.sessions, 2, "两条都要算删除成功");
    assert_eq!(
        report.files, 4,
        "正文 1 + 侧车 3（assistant-display / file-tree-manifests / 会话日志）"
    );

    // 源数据必须真的不在原地了：正文、侧车、日志一个都不能剩
    for path in &sidecars {
        assert!(!path.exists(), "{} 还留在客户端目录里", path.display());
    }

    // 彻底删除还会把回收站批次一起销毁：文件不是「挪走」而是真没了
    delete::purge_into(&trash_root, &report.tag).unwrap();
    assert!(
        !std::path::Path::new(&report.dir).exists(),
        "批次目录要销毁，不能留副本"
    );

    assert_eq!(
        SqliteSessions::new(&db).count().unwrap(),
        0,
        "索引行要清掉，列表里不该再出现"
    );
}

/// 客户端给每个会话落的派生文件：显示缓存、文件树清单、会话日志。
fn seed_sidecars(roots: &Roots, sid: &str) -> Vec<std::path::PathBuf> {
    let root = roots.workbuddy(false);
    let mut out = Vec::new();
    for dir in ["assistant-display", "file-tree-manifests"] {
        let path = root.join(dir).join(format!("{sid}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"display":[]}"#).unwrap();
        out.push(path);
    }
    let log = root
        .join("logs")
        .join("2026-11-03")
        .join("sdk")
        .join("conversations")
        .join(format!("{sid}.log"));
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    std::fs::write(&log, "log line").unwrap();
    out.push(log);
    out
}

/// 回收站与客户端**不在同一个盘**：`rename` 会报 EXDEV（os error 17），
/// 必须退回「复制 + 删源」。这是最常见的一种配置（客户端在 C:、工具数据在 D:），
/// 也是「点了彻底删除却一条都没删掉」的真正原因。
///
/// 没有第二个盘的机器上跳过——跨盘路径没法用别的办法造出来。
#[tokio::test]
async fn delete_works_when_trash_lives_on_another_drive() {
    let other = std::path::Path::new("D:/");
    if !other.exists() {
        return;
    }
    let paths = TestPaths::new(); // 源目录在 C: 的 %TEMP%
    let roots = fake_roots(&paths);
    let db = Arc::new(Db::open_at(&paths.join("app.db")).unwrap());
    let sid = seed_workbuddy(&roots);
    let trash_root = other.join("ai-manager-cross-drive-test");
    let _ = std::fs::remove_dir_all(&trash_root);

    let service = ai_manager::app::sessions::Service::new(db.clone(), roots.clone());
    service.scan(Arc::new(CancelToken::new())).await.unwrap();

    let key = format!("workbuddy|cn|uid-1|{sid}");
    let report = delete::delete_into(&roots, &db, &[key], false, &trash_root, None).unwrap();
    assert!(
        report.errors.is_empty(),
        "跨盘不该报错：{:?}",
        report.errors
    );
    assert_eq!(report.sessions, 1, "跨盘也要能删掉");
    assert!(
        !roots
            .workbuddy(false)
            .join("projects/demo")
            .join(format!("{sid}.jsonl"))
            .exists(),
        "源文件要从 C: 上消失"
    );
    assert!(
        std::path::Path::new(&report.dir).starts_with(&trash_root),
        "批次要落在 D: 的回收站里"
    );

    let _ = std::fs::remove_dir_all(&trash_root);
}
