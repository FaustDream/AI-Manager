// Windows 下不弹控制台：release 用 windows 子系统，debug 保留控制台便于看日志
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ai_manager::app::{
    db::Db,
    log::{self, Level},
    migrate::import_v1,
    paths,
};
use std::io::Write;
use std::path::PathBuf;

fn main() {
    install_panic_hook();
    attach_parent_console();
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
        err_out(&format!("启动失败：{e}"));
        std::process::exit(1);
    }
}

/// release 是 GUI 子系统，进程没有 stdout 句柄，println! 写失败会 panic；
/// CLI 输出本来就是尽力而为，写不出去就放弃。
fn out(text: &str) {
    let _ = writeln!(std::io::stdout(), "{text}");
}

fn err_out(text: &str) {
    let _ = writeln!(std::io::stderr(), "{text}");
}

/// 从终端带参启动时接上父控制台，让 --import-v1 / --scan 的输出可见；
/// 双击启动没有父控制台，附加失败就静默放弃（不弹窗、不新建控制台）。
fn attach_parent_console() {
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// panic 落盘：release 是 GUI 子系统，stderr 不可见，任何 panic 不落盘就
/// 永远查不到（v1 的 crash.log 就是这么来的）。追加写 data/logs/crash.log，
/// 带时间戳、线程与位置；这是「出了问题第一时间能查」的底座，先于一切执行。
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let thread = std::thread::current().name().unwrap_or("<unnamed>").to_string();
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "非字符串 panic".to_string()
        };
        let line = format!("[{stamp}] thread {thread} @ {location}: {payload}");
        err_out(&line);
        let dir = paths::logs_dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("crash.log"))
        {
            let _ = writeln!(f, "{line}");
        }
        default(info);
    }));
}

fn arg_value(args: &[String], flag: &str) -> Option<PathBuf> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
}

fn import_from_cli(args: &[String]) -> i32 {
    let _ = log::init(Level::Info);
    let source = arg_value(args, "--from").unwrap_or_else(|| PathBuf::from("data"));
    let target = arg_value(args, "--db").unwrap_or_else(paths::app_db);

    out(&format!("源目录：{}", source.display()));
    out(&format!("目标库：{}", target.display()));

    let db = match Db::open_at(&target) {
        Ok(db) => db,
        Err(e) => {
            err_out(&format!("打开数据库失败：{}", e.message));
            return 1;
        }
    };

    match import_v1(&db, &source) {
        Ok(report) => {
            out(&report.summary());
            if report.orphan_ledger > 0 {
                out(&format!(
                    "提示：{} 条台账记录暂无对应账号（Qoder / Trae 靠扫描发现，P2 会接上）",
                    report.orphan_ledger
                ));
            }
            for warning in &report.warnings {
                out(&format!("提示：{warning}"));
            }
            0
        }
        Err(e) => {
            err_out(&format!("导入失败：{}", e.message));
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
            err_out(&format!("打开数据库失败：{}", e.message));
            return 1;
        }
    };
    match ai_manager::app::accounts::scan_all(&db) {
        Ok(report) => {
            out(&format!("扫描：{} 个（新增 {}）", report.scanned, report.added));
            for (label, count) in &report.by_platform {
                out(&format!("  {label}: {count}"));
            }
            for e in &report.errors {
                out(&format!("  错误：{e}"));
            }
            0
        }
        Err(e) => {
            err_out(&format!("扫描失败：{}", e.message));
            1
        }
    }
}
