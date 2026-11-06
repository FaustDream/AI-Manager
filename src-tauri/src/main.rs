// Windows 下不弹控制台：release 用 windows 子系统，debug 保留控制台便于看日志
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ai_manager::app::{
    db::Db,
    log::{self, Level},
    paths,
};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

fn main() {
    install_panic_hook();
    attach_parent_console();
    let args: Vec<String> = std::env::args().collect();

    // 数据目录可指定：优先级低于环境变量调用方，高于默认的 exe 同级 data/
    if let Some(dir) = arg_value(&args, "--data-dir") {
        std::env::set_var("AIMANAGER_DATA_DIR", dir);
    }

    // 优先于扫描与 GUI，避免刷新状态时意外触发其他启动动作。
    if args.iter().any(|a| a == "--refresh-status") {
        std::process::exit(refresh_status_from_cli(&args));
    }

    // 无头扫描：排查「界面上扫不出账号」时用，不依赖窗口
    if args.iter().any(|a| a == "--scan") {
        std::process::exit(scan_from_cli(&args));
    }

    // 只读诊断：不进 GUI，直接打平台的查询接口，结果打到终端
    if let Some(iid) = arg_text(&args, "--diagnose") {
        std::process::exit(diagnose_from_cli(&args, &iid));
    }

    // 演练：只列出「会自动参与签到」的账号，不调用接口、不写业务数据
    if args.iter().any(|a| a == "--dry-run") {
        std::process::exit(dry_run_from_cli(&args));
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

/// 从终端带参启动时接上父控制台，让 --scan 的输出可见；
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
        let thread = std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_string();
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
        // 堆栈是「非预期状态」唯一能定位到根因的东西：force_capture 不依赖
        // RUST_BACKTRACE，release 包里也一定带得上
        let stack = std::backtrace::Backtrace::force_capture().to_string();
        let dir = paths::logs_dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("crash.log"))
        {
            let _ = writeln!(f, "{line}\n{stack}");
        }
        default(info);
    }));
}

fn arg_value(args: &[String], flag: &str) -> Option<PathBuf> {
    arg_text(args, flag).map(PathBuf::from)
}

fn arg_text(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// `--diagnose <iid>`：只读诊断，成功 0 / 失败 1。
///
/// 与界面上的「诊断」按钮走同一个 `Service::diagnose`——命令行版本不是第二套实现，
/// 否则「界面能诊断、命令行报错」这类差异永远查不清。
fn diagnose_from_cli(args: &[String], iid: &str) -> i32 {
    let _ = log::init(Level::Info);
    let target = arg_value(args, "--db").unwrap_or_else(paths::app_db);
    let db = match Db::open_at(&target) {
        Ok(db) => Arc::new(db),
        Err(e) => {
            err_out(&format!("打开数据库失败：{}", e.message));
            return 1;
        }
    };
    let service = ai_manager::app::checkin::Service::new(
        db,
        Arc::new(ai_manager::app::net::RealHttp::new()),
        Arc::new(ai_manager::app::clock::SystemClock),
    );
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            err_out(&format!("创建运行时失败：{e}"));
            return 1;
        }
    };
    match runtime.block_on(service.diagnose(iid)) {
        Ok(report) => {
            let checked = match report.checked_in {
                Some(true) => "已签",
                Some(false) => "未签",
                None => "未知",
            };
            out(&format!("诊断：{}（{}）", report.platform_label, checked));
            out(&format!("摘要：{}", report.summary));
            if let Some(balance) = report.balance {
                out(&format!("余额：{balance}"));
            }
            if let Some(quota) = &report.quota {
                let detail: Vec<String> = quota
                    .iter()
                    .map(|(name, used)| format!("{name} 已用 {used:.0}%"))
                    .collect();
                out(&format!("额度：{}", detail.join("；")));
            }
            out(&format!("耗时：{}ms", report.ms));
            0
        }
        Err(e) => {
            err_out(&format!("诊断失败（{}）：{}", e.kind.as_str(), e.message));
            1
        }
    }
}

/// 状态查询本身负责落库；不走启动扫描、调度或签到动作。
fn refresh_status_from_cli(args: &[String]) -> i32 {
    let _ = log::init(Level::Info);
    let started = std::time::Instant::now();
    let result = (|| -> ai_manager::app::error::Result<bool> {
        let target = arg_value(args, "--db").unwrap_or_else(paths::app_db);
        let db = Arc::new(Db::open_at(&target)?);
        let rows = ai_manager::app::accounts::repo::list(&db)?;
        let service = ai_manager::app::checkin::Service::new(
            db,
            Arc::new(ai_manager::app::net::RealHttp::new()),
            Arc::new(ai_manager::app::clock::SystemClock),
        );
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let mut counts = std::collections::BTreeMap::<String, [usize; 3]>::new();
            let mut failed = false;
            // 停用只排除自动任务；手动纠正展示仍须查询这些账号。
            for account in rows.iter().filter(|a| a.has_secret && a.requires_checkin) {
                let count = counts.entry(account.platform_label.clone()).or_default();
                match service.status(&account.iid).await {
                    Ok(status) => match status.checked_in {
                        Some(true) => count[0] += 1,
                        Some(false) => count[1] += 1,
                        None => count[2] += 1,
                    },
                    Err(e) => {
                        count[2] += 1;
                        failed = true;
                        refresh_status_error(&account.platform_label, e.kind);
                    }
                }
            }
            for (platform, [checked, unchecked, unknown]) in counts {
                let summary = format!(
                    "{platform}: checked_in true={checked} false={unchecked} unknown={unknown}"
                );
                out(&summary);
                log::action("refresh_status", "cli", !failed, 0, summary);
            }
            Ok(!failed)
        })
    })();
    let ok = match result {
        Ok(ok) => ok,
        Err(e) => {
            refresh_status_error("", e.kind);
            false
        }
    };
    log::action(
        "refresh_status",
        "cli",
        ok,
        started.elapsed().as_millis() as u64,
        "refresh_status",
    );
    i32::from(!ok)
}

fn refresh_status_error(platform: &str, kind: ai_manager::app::error::ErrorKind) {
    // 上游错误原文可能含账号标识；此 CLI 只允许平台与错误类别出现在输出中。
    let message = format!("{platform}: kind={}", kind.as_str());
    err_out(&message);
    log::fail(
        "cli",
        "refresh_status",
        &message,
        serde_json::json!({ "kind": kind.as_str() }).to_string(),
    );
}

/// `--dry-run`：只读列出自动签到会覆盖哪些账号；不调用接口、不写签到记录。
fn dry_run_from_cli(args: &[String]) -> i32 {
    let target = arg_value(args, "--db").unwrap_or_else(paths::app_db);
    let db = match Db::open_at(&target) {
        Ok(db) => db,
        Err(e) => {
            err_out(&format!("打开数据库失败：{}", e.message));
            return 1;
        }
    };
    let rows = match ai_manager::app::accounts::repo::list(&db) {
        Ok(rows) => rows,
        Err(e) => {
            err_out(&format!("读取账号失败：{}", e.message));
            return 1;
        }
    };
    let total = rows.len();
    let targets: Vec<_> = rows
        .iter()
        .filter(|a| a.enabled && a.has_secret && a.requires_checkin)
        .collect();
    let disabled = rows.iter().filter(|a| !a.enabled).count();
    let no_secret = rows.iter().filter(|a| !a.has_secret).count();
    let no_checkin = rows.iter().filter(|a| !a.requires_checkin).count();
    out(&format!(
        "演练：{total} 个账号，其中 {} 个会参与自动签到（停用 {disabled} · 无凭据 {no_secret} · 免签到 {no_checkin}）",
        targets.len()
    ));
    for account in targets {
        out(&format!(
            "  {} · {}",
            account.platform_label,
            account.display_name()
        ));
    }
    out("（dry-run：不调用接口、不写签到记录、不写客户端）");
    0
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
            out(&format!(
                "扫描：{} 个（新增 {}）",
                report.scanned, report.added
            ));
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

#[cfg(test)]
mod refresh_status_tests {
    use super::*;

    #[test]
    fn refresh_status_preserves_explicit_paths_with_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("isolated data");
        let db = data_dir.join("status test.db");
        let args = vec![
            "ai-manager".into(),
            "--refresh-status".into(),
            "--db".into(),
            db.to_string_lossy().into_owned(),
            "--data-dir".into(),
            data_dir.to_string_lossy().into_owned(),
        ];
        assert!(args.iter().any(|a| a == "--refresh-status"));
        assert_eq!(arg_value(&args, "--db"), Some(db));
        assert_eq!(arg_value(&args, "--data-dir"), Some(data_dir));
        assert_eq!(arg_text(&args, "--diagnose"), None);
        assert!(!dir.path().join("isolated data").exists());
    }
}
