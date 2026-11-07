//! GUI 沙箱演示数据播种器：向 `SANDBOX_DIR` 指定的沙箱目录写演示账号。
//!
//! 仅本机手动使用（同 `live_probe` 约定，`#[ignore]` 不进常规门禁）：
//! `SANDBOX_DIR=<沙箱目录> cargo test --test sandbox_seed -- --ignored`
//!
//! 演示账号全部是假数据（无凭据、无网络动作），用于 GUI 界面测试的表格渲染；
//! 双重防线：目标目录必须有沙箱标记文件，且路径不得是 `\data` 结尾的真实库。

use ai_manager::app::accounts::{repo, AccountIdentity};
use ai_manager::app::db::Db;

#[test]
#[ignore]
fn seed_sandbox_demo() {
    let dir = std::env::var("SANDBOX_DIR").expect("需要 SANDBOX_DIR 环境变量指向沙箱目录");
    let root = std::path::Path::new(&dir);
    assert!(
        root.join("SANDBOX-DO-NOT-PUT-REAL-DATA.txt").exists(),
        "目标目录缺少沙箱标记，拒绝播种"
    );
    assert!(!dir.ends_with("data"), "禁止把真实 data/ 当沙箱");

    let db = Db::open_at(&root.join("app.db")).unwrap();

    let demo = [
        ("trae_cn", "Trae CN"),
        ("trae", "Trae Intl"),
        ("qoder", "Qoder CN"),
        ("qoder_intl", "Qoder Intl"),
        ("workbuddy", "WorkBuddy CN"),
        ("workbuddy_intl", "WorkBuddy Intl"),
        ("zcode", "ZCode"),
    ];
    for (n, (platform, label)) in demo.iter().enumerate() {
        for k in 0..4u32 {
            let iid = format!("{platform}:demo-{n}{k}");
            let identity = AccountIdentity {
                iid: iid.clone(),
                platform: platform.to_string(),
                platform_label: label.to_string(),
                account_id: format!("demo-{n}{k}"),
                name: format!("演示账号 {}-{}", label, k + 1),
                expires_at: Some(1_800_000_000_000 + (k as i64) * 86_400_000),
                source: "演示数据".into(),
                requires_checkin: k % 2 == 0,
                has_secret: false,
            };
            repo::upsert_identity(&db, &identity, 0).unwrap();
            if k == 0 {
                repo::set_alias(&db, &iid, &format!("{label} 工作号")).unwrap();
            }
            if k == 1 {
                repo::set_note(&db, &iid, "演示用备注：绑定在测试机").unwrap();
            }
            if k == 2 {
                repo::set_enabled(&db, &iid, false).unwrap();
            }
        }
    }
    println!("已写入 {} 条演示账号", demo.len() * 4);
}
