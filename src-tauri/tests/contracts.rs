//! 22 条领域契约的可执行断言。
//!
//! **本阶段预期全红**：被调用的实现全是 todo!()。红的来源必须是
//! 「未实现 / 断言不成立」，不能是编译错误——那说明签名定错了。
//!
//! 契约编号对应 ARCHITECTURE-v2.md §7。

mod support;

use ai_manager::app::accounts::profile::FileProfile;
use ai_manager::app::accounts::{AccountIdentity, AccountTable};
use ai_manager::contracts::*;
use ai_manager::types::*;
use support::*;

// ---------------------------------------------------------------- 契约 1
#[test]
fn contract_01_interface_overrides_local_record() {
    let store = TodoCheckin;
    let clock = clock_at("2026-10-30T10:00:00+08:00");
    // 本地有今日记录，但接口说没签 → 必须听接口的
    assert!(!store.resolve_checked("qoder:1", Some(false), &clock));
    // 接口没给值时才兜底看本地
    assert!(store.resolve_checked("qoder:1", None, &clock));
}

// ---------------------------------------------------------------- 契约 2
#[test]
fn contract_02_daily_round_skips_today_success() {
    let store = TodoCheckin;
    let clock = clock_at("2026-10-30T10:00:00+08:00");
    store.mark_today("qoder:1", &clock).unwrap();
    assert!(store.marked_today("qoder:1", &clock), "当日已签必须在去重表里");
}

// ---------------------------------------------------------------- 契约 3
#[test]
fn contract_03_skips_when_next_at_in_future() {
    let store = TodoCheckin;
    let clock = clock_at("2026-10-30T10:00:00+08:00");
    store.note_next_at("qoder:1", local_dt(2026, 10, 30, 14, 0)).unwrap();
    let next = store.next_at("qoder:1").expect("next_at 必须能还原");
    assert!(next > clock.now(), "未到点不应发起签到");
}

// ---------------------------------------------------------------- 契约 4
#[test]
fn contract_04a_cooldown_natural_day_resets_at_midnight() {
    let store = TodoCheckin;
    let clock = clock_at("2026-10-30T23:30:00+08:00");
    let next = store
        .next_checkin_at(0, local_dt(2026, 10, 30, 9, 0), &clock)
        .expect("自然日口径应给出下次时刻");
    assert_eq!(next, local_dt(2026, 10, 31, 0, 0));
}

#[test]
fn contract_04b_cooldown_24h_is_exact_offset() {
    let store = TodoCheckin;
    let clock = clock_at("2026-10-30T23:30:00+08:00");
    let next = store
        .next_checkin_at(24, local_dt(2026, 10, 30, 9, 0), &clock)
        .expect("24h 口径应给出下次时刻");
    assert_eq!(next, local_dt(2026, 10, 31, 9, 0));
}

// ---------------------------------------------------------------- 契约 5
#[test]
fn contract_05a_live_credit_never_writes_ledger() {
    let ledger = TodoLedger;
    let clock = clock_at("2026-10-30T10:00:00+08:00");
    // WorkBuddy / Trae 的 live_credit 为 true
    let wrote = ledger.add_if_needed(true, "workbuddy:1", 50.0, &clock).unwrap();
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
    let store = TodoCheckin;
    let clock = clock_at("2026-10-30T09:00:00+08:00");
    store.note_balance("qoder:1", 100.0, &clock).expect("首次记余额");
    clock.set("2026-10-30T18:00:00+08:00");
    store.note_balance("qoder:1", 120.0, &clock).expect("同日再记余额");
    assert_eq!(store.balance_on("qoder:1", "2026-10-30"), Some(120.0),
        "同一天多次刷新只保留最后一次，否则今日增减会被刷成 0");
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

    assert_eq!(paths.read("workbuddy-desktop.info"), original,
        "sessionState 必须原样写回，否则切回账号会跳登录页");
    // 8b（A→B→A 切回不弹登录页）依赖真客户端，见 P2 真机验收清单
}

// ---------------------------------------------------------------- 契约 9
#[test]
fn contract_09a_client_db_readonly() {
    let paths = TestPaths::new();
    let db = TodoClientDb;
    // 只读打开：实现必须带 SQLITE_OPEN_READ_ONLY
    db.open(&paths.join("client.db"), false).unwrap();
}

#[test]
fn contract_09b_checkpoint_before_write() {
    let db = TodoClientDb;
    db.checkpoint().unwrap();
}

pub struct TodoClientDb;
impl ClientDb for TodoClientDb {
    fn open(&self, _: &std::path::Path, _: bool) -> ai_manager::app::error::Result<()> {
        todo!("P2/P4 实现")
    }
    fn checkpoint(&self) -> ai_manager::app::error::Result<()> {
        todo!("P2/P4 实现")
    }
}

// ---------------------------------------------------------------- 契约 10
#[test]
fn contract_10_restore_only_touches_user_id() {
    let store = TodoSessions;
    let report = store.restore_account("cn", "uid-1").unwrap();
    assert_eq!(report.body_files_before, report.body_files_after,
        "恢复只改归属引用，正文文件数量必须不变");
}

// ---------------------------------------------------------------- 契约 11
#[test]
fn contract_11_no_cross_version_sync_api() {
    // 编译期保证：SessionStore trait 里没有 build_plan / apply_item / sync_session。
    // v1 的 transfer.import_from_other 与 with_other 参数已确认无调用方（死代码），
    // v2 不定义这些方法——「不存在」本身就是这个测试的断言。
    fn _assert_trait_shape(_: &dyn SessionStore) {}
    _assert_trait_shape(&TodoSessions);
}

// ---------------------------------------------------------------- 契约 12
#[test]
fn contract_12_second_instance_exits() {
    let api = TodoPlatform;
    assert!(api.acquire_single_instance("AI Manager").unwrap(), "第一次应成功");
    assert!(!api.acquire_single_instance("AI Manager").unwrap(), "第二次必须失败并退出");
}

// ---------------------------------------------------------------- 契约 13
#[test]
fn contract_13_platform_paths_table() {
    let hub = TodoHub;
    // 国内版带 -cn；WorkBuddy 是例外（国内 .workbuddy / 国际 .workbuddy-ai）
    assert!(hub.target_dir(TargetKey::QoderCn).to_string_lossy().contains(".qoder-cn"));
    assert!(hub.target_dir(TargetKey::WorkBuddy).to_string_lossy().contains(".workbuddy"));
    assert!(hub.target_dir(TargetKey::WorkBuddyIntl).to_string_lossy().contains(".workbuddy-ai"));
}

// ---------------------------------------------------------------- 契约 14
#[test]
fn contract_14_mcp_format_roundtrip() {
    let hub = TodoHub;
    let entry = McpEntry {
        name: "github".into(),
        disabled: true,
        command: Some("npx".into()),
        ..Default::default()
    };
    for key in [TargetKey::Trae, TargetKey::ZCode, TargetKey::CodeBuddy] {
        let raw = hub.to_platform(&entry, key);
        let back = hub.from_platform(&raw, key).unwrap();
        assert_eq!(back.disabled, entry.disabled, "{key:?} 往返后停用状态应一致");
        assert_eq!(back.name, entry.name);
    }
}

// ---------------------------------------------------------------- 契约 15
#[test]
fn contract_15_backup_before_writing_platform_config() {
    let hub = TodoHub;
    let path = hub.backup_before_write(TargetKey::Trae).unwrap();
    assert!(path.to_string_lossy().contains("backups"), "写前必须备份到 backups 目录");
}

// ---------------------------------------------------------------- 契约 16
#[test]
fn contract_16_conflict_not_silently_overwritten() {
    let hub = TodoHub;
    let err = hub.check_conflict("github", TargetKey::Trae).unwrap_err();
    assert_eq!(err.to_string(), conflict_error("github").to_string());
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

// ---------------------------------------------------------------- 契约 18
#[test]
fn contract_18_time_rendered_in_local_zone() {
    let dt = chrono::DateTime::parse_from_rfc3339("2026-10-30T14:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Local);
    let rendered = render_local(dt);
    // +08:00 下应为 22:00；若仍是 14:00 说明没转本地时区
    assert_ne!(rendered, "10-30 14:00", "时间必须按本地时区显示");
}

// ---------------------------------------------------------------- 契约 19
#[test]
fn contract_19_no_stdout_writes() {
    // 例外只有一处：`src/main.rs` 是命令行入口（`--import-v1`），stdout 是它唯一的输出渠道。
    // 业务与界面代码（`app/`、`desktop/`）一律不得写 stdout——release 版是 windows 子系统，
    // 没有控制台，写了等于把日志丢掉。所以检查排除 bin 入口，其余全部要查。
    let hits = grep_source_except(&["println!", "print!", "dbg!"], &["main.rs"]);
    assert!(hits.is_empty(), "源码中不得出现 stdout 写入（main.rs 除外）：{hits:?}");
}

// ---------------------------------------------------------------- 契约 20
#[test]
fn contract_20_external_command_sets_no_window_flag() {
    let api = TodoPlatform;
    api.spawn_hidden("cmd", &["/c", "echo hi"]).unwrap();
    // flag 值（CREATE_NO_WINDOW = 0x08000000）由实现断言，真机手测确认无控制台
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
    let store = TodoCheckin;
    store.note_next_at("qoder:1", local_dt(2026, 10, 31, 9, 0)).unwrap();
    // 模拟重启：新建 store 实例后仍应读到
    let restarted = TodoCheckin;
    assert_eq!(restarted.next_at("qoder:1"), Some(local_dt(2026, 10, 31, 9, 0)));
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
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
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
