// Windows 下不弹控制台：release 用 windows 子系统，debug 保留控制台便于看日志
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ai_manager::app::{db::Db, log::{self, Level}, migrate::import_v1, paths};
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // 导入器只走命令行，界面不暴露（P1 设计如此：开发者手动跑一次，P7 才做自动导入）
    if args.iter().any(|a| a == "--import-v1") {
        std::process::exit(import_from_cli(&args));
    }

    // 无头扫描：排查「界面上扫不出账号」时用，不依赖窗口
    if args.iter().any(|a| a == "--scan") {
        std::process::exit(scan_from_cli(&args));
    }

    if let Err(e) = ai_manager::desktop::run() {
        eprintln!("启动失败：{e}");
        std::process::exit(1);
    }
}

fn arg_value(args: &[String], flag: &str) -> Option<PathBuf> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
}

fn import_from_cli(args: &[String]) -> i32 {    let _ = log::init(Level::Info);
    let source = arg_value(args, "--from").unwrap_or_else(|| PathBuf::from("data"));
    let target = arg_value(args, "--db").unwrap_or_else(paths::app_db);

    println!("源目录：{}", source.display());
    println!("目标库：{}", target.display());

    let db = match Db::open_at(&target) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("打开数据库失败：{}", e.message);
            return 1;
        }
    };

    match import_v1(&db, &source) {
        Ok(report) => {
            println!("{}", report.summary());
            if report.orphan_ledger > 0 {
                println!("提示：{} 条台账记录暂无对应账号（Qoder / Trae 靠扫描发现，P2 会接上）", report.orphan_ledger);
            }
            for warning in &report.warnings {
                println!("提示：{warning}");
            }
            0
        }
        Err(e) => {
            eprintln!("导入失败：{}", e.message);
            1
        }
    }
}

fn scan_from_cli(args: &[String]) -> i32 {
    let _ = log::init(Level::Info);
    let target = arg_value(args, "--db").unwrap_or_else(paths::app_db);
    let db = match Db::open_at(&target) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("打开数据库失败：{}", e.message);
            return 1;
        }
    };
    match ai_manager::app::accounts::scan_all(&db) {
        Ok(report) => {
            println!("扫描：{} 个（新增 {}）", report.scanned, report.added);
            for (label, count) in &report.by_platform {
                println!("  {label}: {count}");
            }
            for e in &report.errors {
                println!("  错误：{e}");
            }
            0
        }
        Err(e) => {
            eprintln!("扫描失败：{}", e.message);
            1
        }
    }
}
