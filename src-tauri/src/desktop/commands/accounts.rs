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
    Ok(rows)
}

/// 设置账号别名；传空字符串恢复平台昵称。别名单独存 kv，重扫不会覆盖。
#[tauri::command]
pub async fn account_set_alias(
    state: State<'_, AppState>,
    iid: String,
    alias: String,
) -> std::result::Result<String, String> {
    let db = state.db.clone();
    crate::app::runtime::blocking(move || crate::app::accounts::repo::set_alias(&db, &iid, &alias))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)
}

/// 平台奖励积分明细：按包名聚合的剩余/总量/到期（对应网页「套餐与积分」）。
#[tauri::command]

pub async fn account_reward_credits(
    state: State<'_, AppState>,
    iid: String,
) -> std::result::Result<crate::app::providers::workbuddy::RewardCredits, String> {
    state.checkin.reward_credits(&iid).await.map_err(err_text)
}

/// 启用/停用账号：只影响自动签到与自动积分轮询，手动单账号操作仍可执行。
#[tauri::command]
pub async fn account_set_enabled(
    state: State<'_, AppState>,
    iid: String,
    enabled: bool,
) -> std::result::Result<bool, String> {
    let db = state.db.clone();
    crate::app::runtime::blocking(move || {
        crate::app::accounts::repo::set_enabled(&db, &iid, enabled)
    })
    .await
    .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
    .map_err(err_text)
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
    // 结果由调用方记账：启动扫描在后台落动作行，界面手动扫描由前端命令面回传
    crate::app::runtime::blocking(move || crate::app::accounts::scan_all(&db))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)
}

/// 用 Cookie 添加账号（豆包这类「没有可借用登录流程」的平台）。
///
/// 返回新账号的 iid：同一份 Cookie 重复添加会落回同一行，界面按 iid 就能判重。
#[tauri::command]
pub async fn account_add_cookie(
    state: State<'_, AppState>,
    platform: String,
    cookie: String,
    name: String,
) -> std::result::Result<String, String> {
    let db = state.db.clone();
    let identity = crate::app::runtime::blocking(move || {
        crate::app::accounts::cookie::add(&db, &platform, &cookie, &name)
    })
    .await
    .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
    .map_err(err_text)?;
    crate::app::log::action(
        "添加账号",
        "user",
        true,
        0,
        format!(
            "用 Cookie 添加账号：{}（{}）",
            identity.name, identity.platform_label
        ),
    );
    Ok(identity.iid)
}

/// 浏览器登录第一步：申请登录地址 → 后台等登录完成。
/// **不自动打开浏览器**：界面自己给出链接和「在浏览器打开」，避免一次点击弹两个窗口。
/// 登录成功后由后台任务入库并 emit `account:login`，界面据此刷新。
#[tauri::command]

pub async fn account_login_start(
    state: State<'_, AppState>,
    platform: String,
) -> std::result::Result<crate::app::accounts::login::LoginStart, String> {
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

    spawn_login_wait(
        state.db.clone(),
        platform,
        pending.ticket.clone(),
        "account_login",
        "浏览器登录",
    );
    Ok(pending.start)
}

/// 详情页的「重新登录」：给已有账号重新走一遍浏览器登录，成功后凭据自动落库。
///
/// 与「添加账号」的差异只有两点：
///   1. 平台键由 **iid** 推出来（WorkBuddy 国际版带 `@intl`），界面不用自己拼；
///   2. 日志按「重新登录」记，事后能分清这一次是补凭据还是加号。
///
/// 凭据由 `login::save()` 自动保存：iid 相同 = 覆盖这个账号的凭据与到期时间；
/// 登进了别的账号 = 作为新账号入库（返回值里带新 iid，界面据此提示）。
#[tauri::command]

pub async fn account_relogin_start(
    state: State<'_, AppState>,
    iid: String,
) -> std::result::Result<crate::app::accounts::login::LoginStart, String> {
    let db = state.db.clone();
    let key = {
        let db = db.clone();
        crate::app::runtime::blocking(move || {
            crate::app::accounts::login::relogin_platform(&db, &iid)
        })
        .await
        .map_err(err_text)?
        .map_err(err_text)?
    };

    let pending = crate::app::accounts::login::start(&key)
        .await
        .map_err(err_text)?;
    log::log(
        Level::Info,
        "accounts",
        format!(
            "重新登录：已生成 {} 的登录链接，等待用户在浏览器完成授权",
            pending.start.label
        ),
    );

    spawn_login_wait(
        db,
        key,
        pending.ticket.clone(),
        "account_relogin",
        "重新登录",
    );
    Ok(pending.start)
}

/// 登录的后半段：后台等授权 → 入库 → 推 `account:login`。
/// 「添加账号」与「重新登录」共用：两处各写一遍必然有一处忘了落失败日志。
fn spawn_login_wait(
    db: std::sync::Arc<crate::app::db::Db>,
    platform_key: String,
    ticket: String,
    action: &'static str,
    scene: &'static str,
) {
    // 日志要用平台名，但 platform_key 会被 move 进阻塞闭包，先留一份
    let log_platform = platform_key.clone();
    tauri::async_runtime::spawn(async move {
        let result = crate::app::runtime::blocking(move || {
            crate::app::accounts::login::wait_for_login(&db, &platform_key, &ticket)
        })
        .await;
        let outcome = match result {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(e)) | Err(e) => {
                // 失败原因必须带 kind 与 detail：以前只留一句「登录超时」，
                // 分不清是网络、平台拒绝还是用户没操作
                log::fail(
                    "accounts",
                    action,
                    format!("{scene}失败：{}", e.message),
                    format!("{} · 平台={log_platform}", e.evidence()),
                );
                crate::app::accounts::login::LoginOutcome {
                    ok: false,
                    message: e.message,
                    iid: String::new(),
                }
            }
        };
        if outcome.ok {
            log::log(
                Level::Info,
                "accounts",
                format!("{scene}：{}", outcome.message),
            );
        }
        crate::app::events::emit("account:login", &outcome);
    });
}

/// 「刷新所有账号状态」：重扫客户端登录态 + 用凭据包校正到期时间。
///
/// 要点是**让本地库追上客户端的实际登录情况**：登录态被客户端刷新过、凭据换了、
/// 到期时间变了，界面上的「剩 N 天」「已过期」都跟着一起更新。
/// 不自己实现 token 续期——凭据真过期了就重新登录，走平台自己的登录页。
///
/// 走阻塞线程：读客户端库 + 逐个解密凭据全是磁盘活，放主线程会让窗口变灰。
#[tauri::command]

pub async fn account_refresh_all(
    state: State<'_, AppState>,
) -> std::result::Result<crate::app::accounts::refresh::RefreshReport, String> {
    let db = state.db.clone();
    let started = std::time::Instant::now();
    let report =
        crate::app::runtime::blocking(move || crate::app::accounts::refresh::refresh_all(&db))
            .await
            .map_err(err_text)?
            .map_err(err_text)?;
    // 失败明细必须进日志：界面只说「N 个账号有问题」，事后要能查是哪几个、什么原因
    let detail = if report.errors.is_empty() {
        String::new()
    } else {
        format!(" | 失败明细: {}", report.errors.join("；"))
    };
    log::action(
        "account_refresh_all",
        "user",
        report.errors.is_empty(),
        started.elapsed().as_millis() as u64,
        format!(
            "刷新账号状态：{} 个账号 · 有凭据 {} · 缺凭据 {} · 已过期 {} · 临期 {} · 校正到期 {} · 当前登录 {} · 扫描 {}（新增 {}）{detail}",
            report.total,
            report.with_secret,
            report.missing_secret,
            report.expired,
            report.expiring,
            report.updated,
            report.current,
            report.scanned,
            report.added
        ),
    );
    Ok(report)
}

/// 刷新单个账号的凭据：**不管当前是否过期都重扫一次**，把客户端里的最新凭据
/// 与有效期写回本地库；刷新完仍过期才明确告诉用户「只能重新登录」。
/// **不猜任何平台的 refresh 协议**。
///
/// 走阻塞线程：重扫要读客户端文件、解密凭据，是纯磁盘活。
#[tauri::command]

pub async fn account_refresh_credential(
    state: State<'_, AppState>,
    iid: String,
) -> std::result::Result<crate::app::accounts::RefreshCredentialOutcome, String> {
    let db = state.db.clone();
    let target = iid.clone();
    let started = std::time::Instant::now();
    let outcome = crate::app::runtime::blocking(move || {
        crate::app::accounts::refresh_credential(&db, &target)
    })
    .await
    .map_err(err_text)?;
    match &outcome {
        Ok(out) => log::action(
            "account_refresh_credential",
            "user",
            out.state != crate::app::accounts::RefreshCredentialState::NeedRelogin,
            started.elapsed().as_millis() as u64,
            format!("刷新凭据 {iid}：{:?} · {}", out.state, out.message),
        ),
        Err(e) => log::fail(
            "accounts",
            "account_refresh_credential",
            format!("刷新凭据 {iid} 失败：{}", e.message),
            serde_json::json!({ "kind": e.kind.as_str(), "iid": iid }).to_string(),
        ),
    }
    outcome.map_err(err_text)
}

/// 批量刷新凭据：按平台分组，每家客户端只重扫一次。
/// `iids` 为空表示刷新库里全部账号。
#[tauri::command]

pub async fn account_refresh_credentials(
    state: State<'_, AppState>,
    iids: Option<Vec<String>>,
) -> std::result::Result<crate::app::accounts::RefreshCredentialsReport, String> {
    let db = state.db.clone();
    let started = std::time::Instant::now();
    let targets = match iids {
        Some(list) => list,
        // 没指定就刷全部：批量按钮的默认语义
        None => crate::app::runtime::blocking({
            let db = db.clone();
            move || {
                crate::app::accounts::repo::list(&db)
                    .map(|rows| rows.into_iter().map(|r| r.iid).collect::<Vec<String>>())
            }
        })
        .await
        .map_err(err_text)?
        .map_err(err_text)?,
    };
    let count = targets.len();
    let report = crate::app::runtime::blocking(move || {
        crate::app::accounts::refresh_credentials(&db, &targets)
    })
    .await
    .map_err(err_text)?
    .map_err(err_text)?;
    log::action(
        "account_refresh_credentials",
        "user",
        report.failed == 0,
        started.elapsed().as_millis() as u64,
        format!(
            "批量刷新凭据：{count} 个账号 · 更新 {} · 已是最新 {} · 需重登 {} · 不支持 {} · 失败 {}{}",
            report.refreshed,
            report.up_to_date,
            report.need_relogin,
            report.unsupported,
            report.failed,
            if report.errors.is_empty() {
                String::new()
            } else {
                format!(" | {}", report.errors.join("；"))
            }
        ),
    );
    Ok(report)
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

/// 用 WorkBuddy 国内版档案切换 CodeBuddy 软件 / IDEA 插件的登录。
/// `target`：`"software"`（CodeBuddy 软件）或 `"idea"`（IDEA 插件）。
/// 与 `account_switch` 分开：三端同号但一端一按钮，切谁只动谁。
#[tauri::command]

pub async fn account_switch_codebuddy(
    state: State<'_, AppState>,
    iid: String,
    target: String,
) -> std::result::Result<crate::app::accounts::SwitchResult, String> {
    let db = state.db.clone();
    let roots = state.sessions.roots().clone();
    crate::app::runtime::blocking(move || {
        crate::app::accounts::switch_codebuddy(&db, &roots, &iid, &target)
    })
    .await
    .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
    .map_err(err_text)
}

/// CodeBuddy 系两个客户端（软件 / IDEA 插件）的当前登录状态。
/// 详情页用它把「现在登的是谁 / 有没有装」显示清楚，切换后刷新。
#[tauri::command]

pub async fn account_codebuddy_targets(
    state: State<'_, AppState>,
) -> std::result::Result<Vec<crate::app::providers::codebuddy::CodebuddyTargetStatus>, String> {
    let roots = state.sessions.roots().clone();
    crate::app::runtime::blocking(move || crate::app::providers::codebuddy::statuses(&roots))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))
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
    crate::app::runtime::blocking(crate::app::accounts::current_iids)
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))
}

/// 现在能被切到客户端的账号 iid 集合。界面据此禁用「切不了」的切换按钮并说明原因。
#[tauri::command]

pub async fn account_switchable() -> std::result::Result<Vec<String>, String> {
    crate::app::runtime::blocking(move || {
        crate::app::accounts::switch::switchable_iids(&crate::app::db::Db::open())
    })
    .await
    .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
    .map_err(err_text)
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

// ---------------------------------------------------------------- 备注

/// 设置备注；空字符串表示清空。
#[tauri::command]

pub async fn account_set_note(
    state: State<'_, AppState>,
    iid: String,
    note: String,
) -> std::result::Result<String, String> {
    let db = state.db.clone();
    let saved = crate::app::runtime::blocking(move || {
        crate::app::accounts::repo::set_note(&db, &iid, &note)
    })
    .await
    .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
    .map_err(err_text)?;
    Ok(saved)
}

// ---------------------------------------------------------------- 批量诊断

/// 批量诊断结果：只回汇总，逐账号细节已经在日志里。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, serde::Serialize)]
pub struct BatchDiagnoseReport {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total: usize,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub ok: usize,
    /// `账号名 → 失败原因`，界面只显示前两条
    pub failed: Vec<String>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub ms: u64,
}

/// 批量诊断：并发 3（与签到批次同一上限），只读、不写库、不触发签到。
#[tauri::command]

pub async fn account_batch_diagnose(
    state: State<'_, AppState>,
    iids: Vec<String>,
) -> std::result::Result<BatchDiagnoseReport, String> {
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    let service = state.checkin.clone();
    let started = std::time::Instant::now();
    let semaphore = Arc::new(Semaphore::new(3));
    let mut tasks = tokio::task::JoinSet::new();
    for iid in iids {
        let service = service.clone();
        let permit = semaphore.clone();
        tasks.spawn(async move {
            let _permit = permit.acquire_owned().await.ok();
            let result = service.diagnose(&iid).await;
            (iid, result)
        });
    }
    let mut total = 0usize;
    let mut ok = 0usize;
    let mut failed = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        total += 1;
        match joined {
            Ok((_, Ok(_))) => ok += 1,
            Ok((iid, Err(e))) => failed.push(format!("{iid} → {}", e.message)),
            Err(e) => failed.push(format!("任务崩溃 → {e}")),
        }
    }
    let report = BatchDiagnoseReport {
        total,
        ok,
        failed,
        ms: started.elapsed().as_millis() as u64,
    };
    log::action(
        "account_batch_diagnose",
        "user",
        report.failed.is_empty(),
        report.ms,
        format!(
            "批量诊断：{} 个账号 · 成功 {} · 失败 {}",
            report.total,
            report.ok,
            report.failed.len()
        ),
    );
    Ok(report)
}

/// 导出账号清单（CSV）。给表格/对账用，**不含任何凭据**。
#[tauri::command]

pub async fn account_export(state: State<'_, AppState>) -> std::result::Result<String, String> {
    let db = state.db.clone();
    let path = crate::app::runtime::blocking(move || {
        let mut rows = crate::app::accounts::repo::list(&db)?;
        crate::app::checkin::store::hydrate(&db, &mut rows, &crate::app::clock::SystemClock);
        let mut text = String::from(
            "\u{feff}平台,昵称,备注,已启用,有凭据,到期时间,余额,今日已签,最近成功,最近失败,失败类别\n",
        );
        for row in &rows {
            let cells = [
                row.platform_label.clone(),
                row.display_name(),
                row.note.clone().unwrap_or_default(),
                if row.enabled { "是" } else { "否" }.to_string(),
                if row.has_secret { "是" } else { "否" }.to_string(),
                row.expires_at.map(fmt_ms_day).unwrap_or_default(),
                row.balance.map(|v| v.to_string()).unwrap_or_default(),
                match row.checked_in {
                    Some(true) => "已签".to_string(),
                    Some(false) => "未签".to_string(),
                    None => String::new(),
                },
                row.last_ok_at.map(fmt_ms_day).unwrap_or_default(),
                row.last_fail_at.map(fmt_ms_day).unwrap_or_default(),
                row.last_fail_kind.clone().unwrap_or_default(),
            ];
            text.push_str(
                &cells
                    .iter()
                    .map(|cell| csv_cell(cell))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            text.push('\n');
        }
        let dir = crate::app::paths::data_dir().join("exports");
        std::fs::create_dir_all(&dir)
            .map_err(|e| AppError::new(ErrorKind::Io, format!("创建导出目录失败：{e}")))?;
        let path = dir.join(format!(
            "accounts-{}.csv",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        ));
        std::fs::write(&path, text)
            .map_err(|e| AppError::new(ErrorKind::Io, format!("写入 CSV 失败：{e}")))?;
        Ok::<_, AppError>(path)
    })
    .await
    .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
    .map_err(err_text)?;
    log::action(
        "account_export",
        "user",
        true,
        0,
        format!("账号清单已导出：{}", path.display()),
    );
    Ok(path.display().to_string())
}

fn fmt_ms_day(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|at| {
            at.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_default()
}

/// CSV 单元格：逗号/引号/换行一律加引号并转义，避免备注把整行撑坏。
fn csv_cell(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}
