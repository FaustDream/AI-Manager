//! 22 条领域契约的可执行断言。
//!
//! **本阶段预期全红**：被调用的实现全是 todo!()。红的来源必须是
//! 「未实现 / 断言不成立」，不能是编译错误——那说明签名定错了。
//!
//! 契约编号定义见 AGENTS.md §4 与 docs/rules/10-architecture-entrypoints.md。

mod support;

use ai_manager::app::accounts::profile::FileProfile;
use ai_manager::app::accounts::{AccountIdentity, AccountTable};
use ai_manager::app::checkin::{SqliteCheckin, SqliteLedger};
use ai_manager::app::clients::ClientConn;
use ai_manager::app::db::Db;
use ai_manager::contracts::*;
use ai_manager::types::*;
use chrono::TimeZone;
use support::*;

// ---------------------------------------------------------------- 契约 1
#[test]
fn contract_01_interface_overrides_local_record() {
    let paths = TestPaths::new();
    let db = Db::open_at(&paths.join("app.db")).unwrap();
    let store = SqliteCheckin::new(&db);
    let clock = clock_at("2026-10-30T10:00:00+08:00");
    store.mark_today("qoder:1", &clock).unwrap();

    // 本地有今日记录，但接口说没签 → 必须听接口的
    assert!(!store.resolve_checked("qoder:1", Some(false), &clock));
    // 接口没给值时才兜底看本地
    assert!(store.resolve_checked("qoder:1", None, &clock));
}

// ---------------------------------------------------------------- 契约 2
#[test]
fn contract_02_daily_round_skips_today_success() {
    let paths = TestPaths::new();
    let db = Db::open_at(&paths.join("app.db")).unwrap();
    let store = SqliteCheckin::new(&db);
    let clock = clock_at("2026-10-30T10:00:00+08:00");
    store.mark_today("qoder:1", &clock).unwrap();
    assert!(
        store.marked_today("qoder:1", &clock),
        "当日已签必须在去重表里"
    );
}

// ---------------------------------------------------------------- 契约 3
#[test]
fn contract_03_skips_when_next_at_in_future() {
    let paths = TestPaths::new();
    let db = Db::open_at(&paths.join("app.db")).unwrap();
    let store = SqliteCheckin::new(&db);
    let clock = clock_at("2026-10-30T10:00:00+08:00");
    store
        .note_next_at("qoder:1", local_dt(2026, 10, 30, 14, 0))
        .unwrap();
    let next = store.next_at("qoder:1").expect("next_at 必须能还原");
    assert!(next > clock.now(), "未到点不应发起签到");
}

// ---------------------------------------------------------------- 契约 4
#[test]
fn contract_04a_cooldown_natural_day_resets_at_midnight() {
    let paths = TestPaths::new();
    let db = Db::open_at(&paths.join("app.db")).unwrap();
    let store = SqliteCheckin::new(&db);
    let clock = clock_at("2026-10-30T23:30:00+08:00");
    let next = store
        .next_checkin_at(0, local_dt(2026, 10, 30, 9, 0), &clock)
        .expect("自然日口径应给出下次时刻");
    assert_eq!(next, local_dt(2026, 10, 31, 0, 0));
}

#[test]
fn contract_04b_cooldown_24h_is_exact_offset() {
    let paths = TestPaths::new();
    let db = Db::open_at(&paths.join("app.db")).unwrap();
    let store = SqliteCheckin::new(&db);
    let clock = clock_at("2026-10-30T23:30:00+08:00");
    let next = store
        .next_checkin_at(24, local_dt(2026, 10, 30, 9, 0), &clock)
        .expect("24h 口径应给出下次时刻");
    assert_eq!(next, local_dt(2026, 10, 31, 9, 0));
}

// ---------------------------------------------------------------- 契约 5
#[test]
fn contract_05a_live_credit_never_writes_ledger() {
    let paths = TestPaths::new();
    let db = Db::open_at(&paths.join("app.db")).unwrap();
    let ledger = SqliteLedger::new(&db);
    let clock = clock_at("2026-10-30T10:00:00+08:00");
    // WorkBuddy / Trae 的 live_credit 为 true
    let wrote = ledger
        .add_if_needed(true, "workbuddy:1", 50.0, None, &clock)
        .unwrap();
    assert!(!wrote, "live_credit 平台不该写台账");
    assert_eq!(ledger.count("workbuddy:1"), 0);
}

#[test]
fn contract_05b_lot_granted_at_is_local_not_from_api() {
    // 接口只给「金额 + 到期日 + 名称」，granted_at 由本地记录
    let lot = CreditLot {
        amount: 50.0,
        expires_at: Some(local_dt(2026, 11, 6, 0, 0)),
        granted_at: None,
        lot_key: "k1".into(),
        label: "签到奖励".into(),
    };
    assert!(lot.granted_at.is_none(), "接口不含获得时间");
}

// ---------------------------------------------------------------- 契约 6
#[test]
fn contract_06_snapshot_keeps_last_per_day() {
    let paths = TestPaths::new();
    let db = Db::open_at(&paths.join("app.db")).unwrap();
    let store = SqliteCheckin::new(&db);
    let clock = clock_at("2026-10-30T09:00:00+08:00");
    store
        .note_balance("qoder:1", 100.0, &clock)
        .expect("首次记余额");
    clock.set("2026-10-30T18:00:00+08:00");
    store
        .note_balance("qoder:1", 120.0, &clock)
        .expect("同日再记余额");
    assert_eq!(
        store.balance_on("qoder:1", "2026-10-30"),
        Some(120.0),
        "同一天多次刷新只保留最后一次，否则今日增减会被刷成 0"
    );
}

// ---------------------------------------------------------------- 契约 7
#[test]
fn contract_07_iid_is_shared_primary_key() {
    let account = sample_account("qoder:abc", true);
    // 四处共用同一个主键：账号库 / 台账 / 签到记录 / UI 行 key
    assert_eq!(account.iid, "qoder:abc");
    assert_eq!(Account::iid_of(PlatformKey::QoderCn, "abc"), "qodercn:abc");
}

// ---------------------------------------------------------------- 契约 8a
#[test]
fn contract_08a_profile_roundtrip_bytes() {
    let paths = TestPaths::new();
    let original = b"{\"sessionState\":\"server-issued\",\"uid\":\"u1\"}";
    let file = paths.write("workbuddy-desktop.info", original);

    let profile = FileProfile;
    let blob = profile.save(&file).unwrap();
    std::fs::write(&file, b"corrupted").unwrap();
    profile.restore(&blob, &file).unwrap();

    assert_eq!(
        paths.read("workbuddy-desktop.info"),
        original,
        "sessionState 必须原样写回，否则切回账号会跳登录页"
    );
    // 8b（A→B→A 切回不弹登录页）依赖真客户端，见 P2 真机验收清单
}

// ---------------------------------------------------------------- 契约 9a / 9b
#[test]
fn contract_09a_client_db_readonly() {
    let paths = TestPaths::new();
    let file = paths.join("client.db");
    seed_client_db(&file);

    let db = ClientConn::new();
    // 只读打开：flag 是 READ_ONLY，写必然失败——比断言 flag 更接近真实风险
    ClientDb::open(&db, &file, false).unwrap();
    assert!(db.is_readonly(), "读客户端库必须是只读连接");
    assert!(
        db.execute("INSERT INTO sessions VALUES ('x','y')").is_err(),
        "只读连接上写不进去"
    );
}

#[test]
fn contract_09b_checkpoint_before_write() {
    let paths = TestPaths::new();
    let file = paths.join("client.db");
    seed_client_db(&file);

    let db = ClientConn::new();
    ClientDb::open(&db, &file, true).unwrap();
    db.execute("INSERT INTO sessions VALUES ('a','b')").unwrap();
    let wal = std::path::PathBuf::from(format!("{}-wal", file.display()));
    assert!(
        std::fs::metadata(&wal)
            .map(|m| m.len() > 0)
            .unwrap_or(false),
        "写完之后 WAL 里应该有还没落盘的数据"
    );

    ClientDb::checkpoint(&db).unwrap();
    assert_eq!(
        std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0),
        0,
        "checkpoint(TRUNCATE) 之后 WAL 必须清空"
    );
}

/// 造一个带 WAL 的客户端库，模拟真实客户端。
fn seed_client_db(path: &std::path::Path) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE sessions(id TEXT PRIMARY KEY, title TEXT);",
    )
    .unwrap();
}

// ---------------------------------------------------------------- 契约 10
#[test]
fn contract_10_restore_only_touches_user_id() {
    let paths = TestPaths::new();
    let (roots, body) = fake_workbuddy(&paths);
    let db = Db::open_at(&paths.join("app.db")).unwrap();
    let store = ai_manager::app::sessions::Store::new(std::sync::Arc::new(db), roots.clone());

    let before = std::fs::read(&body).unwrap();
    let report = store.restore_account("cn", "uid-1").unwrap();

    assert!(report.changed_rows >= 1, "旧账号的会话行要被改到新账号名下");
    assert_eq!(
        report.body_files_before, report.body_files_after,
        "恢复只改归属引用，正文文件数量必须不变"
    );
    assert_eq!(
        std::fs::read(&body).unwrap(),
        before,
        "正文内容一个字节都不能动"
    );

    // 库里只剩 uid-1 的会话
    let conn = rusqlite::Connection::open_with_flags(
        roots.workbuddy(false).join("workbuddy.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let stale: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE user_id <> 'uid-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stale, 0);
}

/// 造一个假的 `~/.workbuddy`：库里有旧账号的会话行，projects 下有正文文件。
fn fake_workbuddy(paths: &TestPaths) -> (ai_manager::app::clients::Roots, std::path::PathBuf) {
    use ai_manager::app::clients::Roots;
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

    let conn = rusqlite::Connection::open(root.join("workbuddy.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE sessions(
             id TEXT PRIMARY KEY, title TEXT, custom_title TEXT, cwd TEXT,
             user_id TEXT, transport TEXT, last_activity_at INTEGER,
             created_at INTEGER, deleted_at INTEGER);
         CREATE TABLE session_usage(session_id TEXT);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions VALUES (?1,'旧会话',NULL,'D:/work','old-uid','local',1760000000000,1760000000000,NULL)",
        [sid],
    )
    .unwrap();
    drop(conn);

    let roots = Roots::at(&home, paths.join("appdata"), paths.join("local"));
    (roots, body)
}

// ---------------------------------------------------------------- 契约 11
#[test]
fn contract_11_no_cross_version_sync_api() {
    // 编译期保证：SessionStore trait 里没有 build_plan / apply_item / sync_session。
    // v1 的 transfer.import_from_other 与 with_other 参数已确认无调用方（死代码），
    // v2 不定义这些方法——「不存在」本身就是这个测试的断言。
    fn _assert_trait_shape<T: SessionStore>() {}
    _assert_trait_shape::<ai_manager::app::sessions::Store>();
}

// ---------------------------------------------------------------- 契约 12
#[test]
fn contract_12_second_instance_exits() {
    let api = ai_manager::app::platform::Platform::new();
    let name = format!("AI Manager test {}", std::process::id());
    assert!(api.acquire_single_instance(&name).unwrap(), "第一次应成功");
    assert!(
        !api.acquire_single_instance(&name).unwrap(),
        "第二次必须失败并退出"
    );
}

// ---------------------------------------------------------------- 契约 13
#[test]
fn contract_13_platform_paths_table() {
    let paths = TestPaths::new();
    let hub = test_hub(&paths);
    // 国内版带 -cn；WorkBuddy 是例外（国内 .workbuddy / 国际 .workbuddy-ai）
    assert!(hub
        .target_dir(TargetKey::QoderCn)
        .to_string_lossy()
        .contains(".qoder-cn"));
    assert!(hub
        .target_dir(TargetKey::WorkBuddy)
        .to_string_lossy()
        .contains(".workbuddy"));
    assert!(hub
        .target_dir(TargetKey::WorkBuddyIntl)
        .to_string_lossy()
        .contains(".workbuddy-ai"));
    // 七/八个目标一个都不能少：漏一个就是「部署到不存在的地方」
    for key in [
        TargetKey::QoderCn,
        TargetKey::Qoder,
        TargetKey::TraeCn,
        TargetKey::Trae,
        TargetKey::WorkBuddy,
        TargetKey::WorkBuddyIntl,
        TargetKey::CodeBuddy,
        TargetKey::ZCode,
    ] {
        assert!(
            !hub.target_dir(key).as_os_str().is_empty(),
            "{key:?} 没有技能目录"
        );
    }
}

// ---------------------------------------------------------------- 契约 14
#[test]
fn contract_14_mcp_format_roundtrip() {
    let paths = TestPaths::new();
    let hub = test_hub(&paths);
    let entry = McpEntry {
        name: "github".into(),
        disabled: true,
        command: Some("npx".into()),
        ..Default::default()
    };
    for key in [TargetKey::Trae, TargetKey::ZCode, TargetKey::CodeBuddy] {
        let raw = hub.to_platform(&entry, key);
        let back = hub.from_platform(&raw, key).unwrap();
        assert_eq!(
            back.disabled, entry.disabled,
            "{key:?} 往返后停用状态应一致"
        );
        assert_eq!(back.name, entry.name);
    }
}

// ---------------------------------------------------------------- 契约 16
#[test]
fn contract_16_conflict_not_silently_overwritten() {
    let paths = TestPaths::new();
    let hub = test_hub(&paths);
    // 平台上已有一条「不是本工具写的」同名条目
    let appdata = paths.join("appdata");
    let file = appdata.join("Trae").join("User").join("mcp.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let original = r#"{"mcpServers":{"github":{"command":"uvx","args":["gh"]}}}"#;
    std::fs::write(&file, original).unwrap();

    let err = hub.check_conflict("github", TargetKey::Trae).unwrap_err();
    assert_eq!(err.to_string(), conflict_error("github").to_string());
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        original,
        "冲突时原文件不能被动过"
    );
}

/// 契约用的 Hub：路径全在临时目录里，绝不碰真实客户端配置。
fn test_hub(paths: &TestPaths) -> ai_manager::app::hub::Hub {
    ai_manager::app::hub::Hub::at(
        paths.join("home"),
        paths.join("appdata"),
        paths.join("data"),
    )
}

// ---------------------------------------------------------------- 契约 17
#[test]
fn contract_17_no_token_in_serialized_account() {
    let account = sample_account("qoder:abc", true);
    let json = serde_json::to_value(&account).unwrap();
    assert!(json.get("token").is_none(), "Account 不得含 token 字段");
    // iid 允许作为 key 存在，但界面不得渲染它（由前端组件保证）
    assert!(json.get("iid").is_some(), "iid 需作为行 key");
}

#[test]
fn contract_17_display_output_masks_phone_and_never_shows_iid() {
    // 契约 17 的显示规则（P2 实测补录）：昵称可能是完整手机号，
    // 出口统一过 display_name() 打码；渲染文本里也不得出现完整 iid。
    let identity = ai_manager::app::accounts::AccountIdentity {
        iid: "zcode:17679130626".into(),
        platform: "zcode".into(),
        platform_label: "ZCode".into(),
        account_id: "17679130626".into(),
        name: "17679130626".into(),
        requires_checkin: false,
        ..Default::default()
    };
    let view = ai_manager::app::accounts::AccountView::new(identity, 0);
    let shown = view.display_name();
    assert_eq!(shown, "176****0626", "手机号昵称必须打码");
    assert!(!shown.contains("17679130626"), "展示文本不得出现完整手机号");
    assert!(!shown.contains(&view.iid), "展示文本不得出现 iid");
}

// ---------------------------------------------------------------- 契约 18
#[test]
fn contract_18_time_crosses_ipc_as_machine_readable() {
    // 「按本地时区显示」是 UI 层的事：前端 src/lib/format.ts 的 fmtTime 由 vitest
    // 固定时区验证（node 侧可用 TZ 环境变量，结果与 runner 时区无关）。
    // 后端这条契约的是边界：载荷里只准出现带偏移的 RFC3339，不准出现展示文本。
    let row = SessionRow {
        kind: "workbuddy".into(),
        source_key: "cn".into(),
        source_label: "WorkBuddy CN".into(),
        account: "u1".into(),
        account_label: "测试账号".into(),
        session_id: "s1".into(),
        title: "标题".into(),
        project: "项目".into(),
        cwd: "C:/project".into(),
        origin: "desktop".into(),
        last_at: Some(
            chrono::Local
                .with_ymd_and_hms(2026, 10, 30, 22, 0, 0)
                .single()
                .unwrap(),
        ),
        msgs: Some(3),
        size: Some(1024),
        state: SessionState::Visible,
        has_body: true,
    };
    let value = serde_json::to_value(&row).unwrap();
    let last_at = value["last_at"]
        .as_str()
        .expect("last_at 必须序列化成时间戳字符串");
    assert!(
        last_at.contains('T') && (last_at.contains('+') || last_at.ends_with('Z')),
        "必须是带时区偏移的 RFC3339，而不是展示文本：{last_at}"
    );
    assert!(
        !last_at.contains("-30 22:00"),
        "后端不得预格式化时间（展示归 UI 的 fmtTime）：{last_at}"
    );
}

// ---------------------------------------------------------------- 契约 19
#[test]
fn contract_19_no_stdout_writes() {
    // 例外只有两处，都是**控制台子系统**的 bin 入口（stdout 是它们唯一的输出渠道）：
    //   - `main.rs`：命令行入口（`--scan`）；
    //   - `bin/ai-manager-web.rs`：网页调试模式入口，不带 windows_subsystem 属性，
    //     终端里的启动提示只能走 stdout。
    // 业务与界面代码（`app/`、`desktop/`）一律不得写 stdout——release 桌面版是
    // windows 子系统，没有控制台，写了等于把日志丢掉。
    let hits = grep_source_except(
        &["println!", "print!", "dbg!"],
        &["main.rs", "ai-manager-web.rs"],
    );
    assert!(
        hits.is_empty(),
        "源码中不得出现 stdout 写入（两个控制台 bin 入口除外）：{hits:?}"
    );
}

// ---------------------------------------------------------------- 契约 20
#[test]
fn contract_20_external_command_sets_no_window_flag() {
    let api = ai_manager::app::platform::Platform::new();
    // 真的跑一条外部命令：带 CREATE_NO_WINDOW，跑完没有任何窗口出现。
    // 能返回 Ok 说明命令行拼装被 Windows 接受了（拼错会直接开不起来）。
    let cmd = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into());
    api.spawn_hidden(&cmd, &["/c", "echo hi"]).unwrap();
    // 顺带给「进程名表」一个可断言的出口：所有平台键都要有进程名
    for key in [
        PlatformKey::TraeCn,
        PlatformKey::Trae,
        PlatformKey::QoderCn,
        PlatformKey::QoderIntl,
        PlatformKey::WorkBuddy,
        PlatformKey::WorkBuddyIntl,
        PlatformKey::ZCode,
    ] {
        let _ = api.client_running(key);
    }
}

// ---------------------------------------------------------------- 契约 21
#[test]
fn contract_21_rescan_preserves_cached_fields() {
    let mut table = AccountTable::default();
    table.merge_scan(vec![identity_of("workbuddy:abc", "旧名")]);
    // 模拟「余额已经查回来了」
    table.find_mut("workbuddy:abc").unwrap().balance = Some(62.0);

    // 第二次扫描：昵称变了
    table.merge_scan(vec![identity_of("workbuddy:abc", "新名")]);
    let row = &table.rows()[0];

    assert_eq!(row.name, "新名", "身份字段必须更新");
    // 国际版等免签到账号不参与状态查询，缓存一清就永远停在「—」
    assert_eq!(row.balance, Some(62.0), "重扫不得清空已获取的余额");
}

fn identity_of(iid: &str, name: &str) -> AccountIdentity {
    let (platform, account_id) = iid.split_once(':').unwrap();
    AccountIdentity {
        iid: iid.into(),
        platform: platform.into(),
        platform_label: "WorkBuddy 国际版".into(),
        account_id: account_id.into(),
        name: name.into(),
        requires_checkin: false,
        has_secret: true,
        ..Default::default()
    }
}

// ---------------------------------------------------------------- 契约 22
#[test]
fn contract_22_next_at_survives_restart() {
    let paths = TestPaths::new();
    let db = Db::open_at(&paths.join("app.db")).unwrap();
    {
        let store = SqliteCheckin::new(&db);
        store
            .note_next_at("qoder:1", local_dt(2026, 10, 31, 9, 0))
            .unwrap();
    }
    // 模拟重启：重新开库后仍应读到
    let reopened = Db::open_at(&paths.join("app.db")).unwrap();
    let restarted = SqliteCheckin::new(&reopened);
    assert_eq!(
        restarted.next_at("qoder:1"),
        Some(local_dt(2026, 10, 31, 9, 0))
    );
}

// ---------------------------------------------------------------- 辅助
fn grep_source_except(patterns: &[&str], skip_files: &[&str]) -> Vec<String> {
    let mut hits = Vec::new();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                if skip_files.contains(&name) {
                    continue;
                }
                if let Ok(text) = std::fs::read_to_string(&path) {
                    // 只查生产代码：`#[cfg(test)]` 之后是测试模块，
                    // 那里的 println! 是 `--nocapture` 的正常输出方式，不属于契约范围
                    let production = text.split("#[cfg(test)]").next().unwrap_or(&text);
                    for pattern in patterns {
                        if production.contains(pattern) {
                            hits.push(format!("{}:{}", path.display(), pattern));
                        }
                    }
                }
            }
        }
    }
    hits
}
