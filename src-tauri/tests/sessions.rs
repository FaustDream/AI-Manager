//! 会话索引的两条核心行为：**游标分页不重不漏**、**增量扫描不白扫**。
//!
//! 这两个错了都不会报错，只会表现成「翻到第 3 页少一条」「刷新一次要 30 秒」——
//! 属于用户能感觉到但日志里看不出来的那类问题，所以必须用断言钉住。

mod support;

use ai_manager::app::clients::Roots;
use ai_manager::app::db::Db;
use ai_manager::app::sessions::store::{Facets, Filter, RawRow, SqliteSessions};
use ai_manager::app::sessions::Service;
use ai_manager::types::SessionState;
use ai_manager::app::task::CancelToken;
use std::sync::Arc;
use support::*;

fn row(index: usize, state: SessionState) -> RawRow {
    let mut row = RawRow::empty("workbuddy", "cn", "WorkBuddy 国内版");
    row.session_id = format!("s{index:04}");
    row.title = format!("会话 {index}");
    row.account = "u1".into();
    row.last_at = 1_760_000_000_000 + index as i64 * 1000;
    row.msgs = index as i64;
    row.size = index as i64 * 100;
    row.state = state;
    row.has_body = true;
    row
}

#[test]
fn cursor_pages_do_not_repeat_or_skip() {
    let paths = TestPaths::new();
    let db = Db::open_at(&paths.join("app.db")).unwrap();
    let store = SqliteSessions::new(&db);
    let rows: Vec<RawRow> = (0..250).map(|i| row(i, SessionState::Visible)).collect();
    store.replace_source("cn", &rows).unwrap();

    let filter = Filter::default();
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let page = store.page(&filter, cursor.as_deref(), 100).unwrap();
        pages += 1;
        assert_eq!(page.total, 250);
        seen.extend(page.items.iter().map(|r| r.session_id.clone()));
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
        assert!(pages < 10, "翻页停不下来，游标有问题");
    }
    assert_eq!(pages, 3, "250 条按 100 一页应该正好 3 页");
    assert_eq!(seen.len(), 250, "翻页不能漏行");
    let unique: std::collections::HashSet<_> = seen.iter().collect();
    assert_eq!(unique.len(), 250, "翻页不能重复");

    // 游标必须含唯一键：同一毫秒的两条也能正确翻页
    let mut tied = Vec::new();
    for i in 0..4 {
        let mut item = row(i, SessionState::Visible);
        item.last_at = 1_760_000_000_000;
        tied.push(item);
    }
    let db2 = Db::open_at(&paths.join("tie.db")).unwrap();
    let store2 = SqliteSessions::new(&db2);
    store2.replace_source("cn", &tied).unwrap();
    let first = store2.page(&filter, None, 2).unwrap();
    let second = store2
        .page(&filter, first.next_cursor.as_deref(), 2)
        .unwrap();
    let mut ids: Vec<String> = first
        .items
        .iter()
        .chain(second.items.iter())
        .map(|r| r.session_id.clone())
        .collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 4, "同一时间戳的会话也要一条不漏地翻完");
}

#[test]
fn sort_by_size_and_msgs_pages_cleanly() {
    use ai_manager::app::sessions::store::Sort;

    let paths = TestPaths::new();
    let db = Db::open_at(&paths.join("app.db")).unwrap();
    let store = SqliteSessions::new(&db);
    let rows: Vec<RawRow> = (0..120).map(|i| row(i, SessionState::Visible)).collect();
    store.replace_source("cn", &rows).unwrap();

    for sort in [Sort::Size, Sort::Msgs] {
        let filter = Filter {
            sort,
            ..Default::default()
        };
        let mut seen = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let page = store.page(&filter, cursor.as_deref(), 50).unwrap();
            seen.extend(page.items.iter().map(|r| r.session_id.clone()));
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(seen.len(), 120, "{sort:?} 档翻页不能漏行");
        let unique: std::collections::HashSet<_> = seen.iter().collect();
        assert_eq!(unique.len(), 120, "{sort:?} 档翻页不能重复");
        // 大小/消息数随 index 递增，倒序第一行应是最大的那条
        assert_eq!(seen[0], "s0119");
    }

    // 中途换排序：游标档位与请求不一致时从头翻，不错位
    let first = store
        .page(
            &Filter {
                sort: Sort::Size,
                ..Default::default()
            },
            None,
            10,
        )
        .unwrap();
    let again = store
        .page(
            &Filter {
                sort: Sort::Msgs,
                ..Default::default()
            },
            first.next_cursor.as_deref(),
            10,
        )
        .unwrap();
    assert_eq!(again.items[0].session_id, "s0119", "换档后应从头开始");
}

#[test]
fn filters_and_facets_are_consistent() {
    let paths = TestPaths::new();
    let db = Db::open_at(&paths.join("app.db")).unwrap();
    let store = SqliteSessions::new(&db);
    store
        .replace_source(
            "cn",
            &[row(1, SessionState::Visible), row(2, SessionState::Deleted)],
        )
        .unwrap();
    let mut other = row(3, SessionState::Visible);
    other.source_key = "intl".into();
    other.source_label = "WorkBuddy 国际版".into();
    store.replace_source("intl", &[other]).unwrap();

    let deleted = store
        .page(
            &Filter {
                state: Some("deleted".into()),
                ..Default::default()
            },
            None,
            50,
        )
        .unwrap();
    assert_eq!(deleted.items.len(), 1);
    assert_eq!(deleted.total, 1);

    let intl = store
        .page(
            &Filter {
                source: Some("intl".into()),
                ..Default::default()
            },
            None,
            50,
        )
        .unwrap();
    assert_eq!(intl.items.len(), 1);
    assert_eq!(intl.items[0].source_label, "WorkBuddy 国际版");

    let facets: Facets = store.facets().unwrap();
    assert_eq!(facets.total, 3);
    assert_eq!(facets.sources.len(), 2);
    let visible = facets.states.iter().find(|f| f.value == "visible").unwrap();
    assert_eq!(visible.count, 2);
    assert_eq!(visible.label, "可见");
}

#[tokio::test]
async fn scan_is_incremental_and_picks_up_changes() {
    let paths = TestPaths::new();
    let (roots, body) = fake_workbuddy(&paths);
    let db = Arc::new(Db::open_at(&paths.join("app.db")).unwrap());
    let service = Service::new(db.clone(), roots.clone());

    let first = service.scan(Arc::new(CancelToken::new())).await.unwrap();
    assert_eq!(first.scanned, 1, "第一次要真扫 cn");
    assert_eq!(first.rows, 1);
    assert!(first.errors.is_empty(), "{:?}", first.errors);

    let rows = SqliteSessions::new(&db)
        .page(&Filter::default(), None, 50)
        .unwrap();
    assert_eq!(rows.items.len(), 1);
    let item = &rows.items[0];
    assert_eq!(item.msgs, Some(2), "jsonl 里两行 message");
    assert!(item.has_body);
    assert_eq!(item.state, SessionState::Visible);
    assert_eq!(item.account, "uid-1", "指纹里的 uid 决定归属");

    // 第二次：指纹没变 → 整源跳过，不重读正文
    let second = service.scan(Arc::new(CancelToken::new())).await.unwrap();
    assert_eq!(second.scanned, 0);
    assert_eq!(second.skipped, 1);

    // 正文追加一条消息 → 指纹变化 → 重扫并更新派生字段
    let mut text = std::fs::read_to_string(&body).unwrap();
    text.push_str("{\"type\":\"message\",\"role\":\"user\"}\n");
    std::fs::write(&body, text).unwrap();
    let third = service.scan(Arc::new(CancelToken::new())).await.unwrap();
    assert_eq!(third.scanned, 1, "正文变了必须重扫");
    let rows = SqliteSessions::new(&db)
        .page(&Filter::default(), None, 50)
        .unwrap();
    assert_eq!(rows.items[0].msgs, Some(3));
}

/// 假 `~/.workbuddy`：库里有当前账号的会话 + projects 下的正文。
fn fake_workbuddy(paths: &TestPaths) -> (Roots, std::path::PathBuf) {
    let home = paths.join("home");
    let root = home.join(".workbuddy");
    let projects = root.join("projects").join("demo");
    std::fs::create_dir_all(&projects).unwrap();
    let sid = "0f3a9c2d8b7e4a1f9c3d5e7b9a1c2d3e";
    let body = projects.join(format!("{sid}.jsonl"));
    std::fs::write(
        &body,
        "{\"type\":\"message\",\"role\":\"user\"}\n{\"type\":\"message\",\"role\":\"ai\"}\n",
    )
    .unwrap();

    // 当前登录账号放在 account-snapshot.json 里：归属判定靠它，不靠反推
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
             created_at INTEGER, deleted_at INTEGER);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions VALUES (?1,'重构会话域',NULL,'D:/work','uid-1','local',1760000000000,1760000000000,NULL)",
        [sid],
    )
    .unwrap();
    (
        Roots::at(&home, paths.join("appdata"), paths.join("local")),
        body,
    )
}
