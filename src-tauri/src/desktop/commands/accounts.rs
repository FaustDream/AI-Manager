// accounts 的 IPC 命令：只做转接，业务在 app/ 对应模块。

use super::*;
use crate::app::error::ErrorKind;

// ------------------------------------------------------------------ 账号中心

#[tauri::command]
pub async fn account_list(
    state: State<'_, AppState>,
) -> std::result::Result<Vec<crate::app::accounts::AccountView>, String> {
    let db = state.db.clone();
    // 数据库调用必须在阻塞线程里跑，否则界面在等结果期间没有响应
    let mut rows = crate::app::runtime::blocking(move || {
        let mut rows = crate::app::accounts::repo::list(&db)?;
        // 余额 / 今日已签 从库里补出来：重扫与重启都不会把它们冲掉（契约 21）
        crate::app::checkin::store::hydrate(&db, &mut rows, &crate::app::clock::SystemClock);
        Ok::<_, AppError>(rows)
    })
    .await
    .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
    .map_err(err_text)?;
    // 脱敏在出口做一次：前端不必各自实现，也就不会有人漏掉
    for row in &mut rows {
        row.name = row.display_name();
    }
    log::log(
        Level::Info,
        "accounts",
        format!("列表返回 {} 个账号", rows.len()),
    );
    Ok(rows)
}

#[tauri::command]

pub async fn account_reorder(
    state: State<'_, AppState>,
    iids: Vec<String>,
) -> std::result::Result<(), String> {
    let db = state.db.clone();
    crate::app::runtime::blocking(move || crate::app::accounts::repo::reorder(&db, &iids))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)
}

#[tauri::command]

pub async fn account_delete(
    state: State<'_, AppState>,
    iid: String,
) -> std::result::Result<bool, String> {
    let db = state.db.clone();
    crate::app::runtime::blocking(move || crate::app::accounts::repo::delete(&db, &iid))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)
}

/// 扫描各客户端登录态并入库。
/// 走 `spawn_blocking`：读客户端库 + DPAPI/AES 解密是纯阻塞活，放主线程会让窗口变灰。
#[tauri::command]

pub async fn account_scan(
    state: State<'_, AppState>,
) -> std::result::Result<crate::app::accounts::ScanReport, String> {
    let db = state.db.clone();
    let report = crate::app::runtime::blocking(move || crate::app::accounts::scan_all(&db))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)?;
    log::log(
        Level::Info,
        "accounts",
        format!(
            "扫描完成：{} 个账号（新增 {}）",
            report.scanned, report.added
        ),
    );
    Ok(report)
}

/// 浏览器登录第一步：申请登录地址 → 后台等登录完成。
/// **不自动打开浏览器**：界面自己给出链接和「在浏览器打开」，避免一次点击弹两个窗口。
/// 登录成功后由后台任务入库并 emit `account:login`，界面据此刷新。
#[tauri::command]

pub async fn account_login_start(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    platform: String,
) -> std::result::Result<crate::app::accounts::login::LoginStart, String> {
    use tauri::Emitter;
    let pending = crate::app::accounts::login::start(&platform)
        .await
        .map_err(err_text)?;
    log::log(
        Level::Info,
        "accounts",
        format!(
            "浏览器登录：已生成 {} 的登录链接，等待用户在浏览器完成授权",
            pending.start.label
        ),
    );

    let db = state.db.clone();
    let login_state = pending.ticket.clone();
    let platform_key = platform.clone();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = crate::app::runtime::blocking(move || {
            crate::app::accounts::login::wait_for_login(&db, &platform_key, &login_state)
        })
        .await;
        let outcome = match result {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(e)) => crate::app::accounts::login::LoginOutcome {
                ok: false,
                message: e.message,
                iid: String::new(),
            },
            Err(e) => crate::app::accounts::login::LoginOutcome {
                ok: false,
                message: e.message,
                iid: String::new(),
            },
        };
        log::log(
            if outcome.ok { Level::Info } else { Level::Warn },
            "accounts",
            format!(
                "浏览器登录：{}",
                if outcome.ok {
                    outcome.message.clone()
                } else {
                    format!("失败（{}）", outcome.message)
                }
            ),
        );
        let _ = handle.emit("account:login", &outcome);
    });

    Ok(pending.start)
}

/// 切换登录。会写客户端登录态文件：失败一律带结构化 kind，界面按 kind 给话术。
#[tauri::command]

pub async fn account_switch(
    state: State<'_, AppState>,
    iid: String,
) -> std::result::Result<crate::app::accounts::SwitchResult, String> {
    let db = state.db.clone();
    // 切换后要把旧账号的会话关联到新账号，恢复逻辑需要客户端目录
    let roots = state.sessions.roots().clone();
    crate::app::runtime::blocking(move || crate::app::accounts::switch(&db, &roots, &iid))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)
}

/// 把已有会话按引用关联到当前登录账号（只有 WorkBuddy 有本机会话库）。
/// 会写客户端库：写前自动备份，客户端没关就拒绝执行。
#[tauri::command]

pub async fn account_restore_sessions(
    state: State<'_, AppState>,
    iid: String,
) -> std::result::Result<crate::app::sessions::restore::RestoreOutcome, String> {
    let db = state.db.clone();
    let roots = state.sessions.roots().clone();
    let outcome = crate::app::runtime::blocking(move || {
        crate::app::sessions::restore::for_account(&db, &roots, &iid)
    })
    .await
    .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
    .map_err(err_text)?;
    log::log(
        Level::Info,
        "accounts",
        format!("恢复会话：关联 {} 条", outcome.changed_rows),
    );
    Ok(outcome)
}

/// 账号 UID。列表载荷里不带它（契约 17：界面不渲染 uid），
/// 只有用户点「UID」时才按需取一次，复制到剪贴板。
/// 口径与 v1 一致：WorkBuddy 国际版带 `@intl` 后缀。
#[tauri::command]

pub async fn account_uid(
    state: State<'_, AppState>,
    iid: String,
) -> std::result::Result<String, String> {
    let db = state.db.clone();
    crate::app::runtime::blocking(move || {
        crate::app::accounts::repo::identity(&db, &iid)?
            .map(|identity| identity.account_id)
            .ok_or_else(|| AppError::new(ErrorKind::Invalid, "账号不存在，可能已被删除"))
    })
    .await
    .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
    .map_err(err_text)
}

/// 各客户端当前登录的账号（界面给行标「当前」）。
#[tauri::command]

pub async fn account_current() -> std::result::Result<Vec<String>, String> {
    let current = crate::app::runtime::blocking(crate::app::accounts::current_iids)
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?;
    log::log(
        Level::Info,
        "accounts",
        format!("客户端当前登录：{} 个", current.len()),
    );
    Ok(current)
}

/// 添加账号第一步：退出该平台登录态并启动客户端。
/// 第二步是用户自己在客户端里登录，然后点「扫描客户端」——见 `accounts::add` 的说明。
#[tauri::command]

pub async fn account_add_prepare(
    platform: String,
) -> std::result::Result<crate::app::accounts::add::AddOutcome, String> {
    let outcome =
        crate::app::runtime::blocking(move || crate::app::accounts::add::prepare(&platform))
            .await
            .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
            .map_err(err_text)?;
    log::log(
        Level::Info,
        "accounts",
        format!(
            "添加账号：已打开 {} 的登录页（{}）",
            outcome.label, outcome.exe
        ),
    );
    Ok(outcome)
}

/// 只打开客户端（不动登录态）：自动启动失败时的手动兜底。
#[tauri::command]

pub async fn account_open_client(platform: String) -> std::result::Result<bool, String> {
    crate::app::runtime::blocking(move || crate::app::accounts::add::open_client(&platform))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)
}
