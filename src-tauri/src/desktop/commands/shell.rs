// shell 的 IPC 命令：只做转接，业务在 app/ 对应模块。

use super::*;
use crate::app::error::ErrorKind;

/// 前端挂载完成后回调一次。
/// 意义不在于「打个招呼」，而在于让 Rust 侧能确认命令通道真的通了——
/// 页面能打开但 `/cmd` 不通（后端没起、安全面拦截）的情况，只看页面是看不出来的。
///
/// 只有**本进程第一次**挂载记 Info：开发期热更新、刷新页面都会重新挂载，
/// 每次都写一条会把日志刷成同一句话。后续挂载是过程性事件，降到 Debug。
pub fn ui_ready(state: &AppState) -> String {
    static MOUNTS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let nth = MOUNTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if nth == 1 {
        log::log(Level::Info, "ui", "前端已就绪，IPC 通道正常");
    } else {
        log::log(Level::Debug, "ui", "界面重新挂载，IPC 通道正常");
    }
    format!("{}|{}", env!("CARGO_PKG_VERSION"), state.db_path_label())
}

/// 当前程序版本（设置页展示用）。
pub fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// 退出后端进程（页面里的「退出」按钮）。
/// 先把日志「重复 N 次」的计数补写掉，再延迟一小会儿退出——
/// 延迟是为了让本命令的 HTTP 回包先送达浏览器：立刻 exit 会让前端
/// 只看到连接被重置，分不清「正常退出」和「后端崩了」。
pub fn app_quit() {
    log::flush();
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(200));
        std::process::exit(0);
    });
}

/// 用系统默认浏览器打开一个链接（登录页的「在浏览器打开」）。
/// 只放行 http/https：这个命令的入参来自界面，不能变成任意协议的启动器。
pub fn open_external(url: String) -> std::result::Result<(), String> {
    let trimmed = url.trim();
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return Err(err_text(AppError::new(
            ErrorKind::Invalid,
            "只能打开 http/https 链接",
        )));
    }
    if !crate::app::process::open_url(trimmed) {
        return Err(err_text(AppError::new(ErrorKind::Io, "没能打开浏览器")));
    }
    Ok(())
}

/// 用**无痕窗口**打开登录页（重新授权 / 添加账号的登录链接）。
///
/// 为什么不用 `open_external`：复用浏览器已有会话时，平台会按「已登录的那个账号」
/// 直接签发凭据，用户点「重新授权 A」却拿回 B 的凭据，账号库就此串掉。
///
/// 返回实际使用的浏览器名，界面据此说明「去哪个窗口里登录」；
/// 找不到支持无痕参数的浏览器时返回 Unsupported，界面退回普通打开。
pub fn open_private_browser(url: String) -> std::result::Result<String, String> {
    let trimmed = url.trim();
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return Err(err_text(AppError::new(
            ErrorKind::Invalid,
            "只能打开 http/https 链接",
        )));
    }
    crate::app::browser::open_private(trimmed)
        .map(|browser| browser.to_string())
        .map_err(err_text)
}

/// 没有宿主窗口：命令保留为空操作，前端首屏流程不必为此分叉。
pub fn app_show_window() {}

// ---------------------------------------------------------------- 模型接口

/// 把内存快照里的配置端口校正成 kv 里的真实值。
///
/// 内存单例只记得「上次启动时用的值」：用户改过端口、或服务压根没起来时它会过期。
async fn fill_configured_port(
    db: std::sync::Arc<crate::app::db::Db>,
    mut info: crate::app::model_api::ModelApiInfo,
) -> std::result::Result<crate::app::model_api::ModelApiInfo, String> {
    let configured = crate::app::runtime::blocking({
        let db = db.clone();
        move || crate::app::model_api::configured_port(&db)
    })
    .await
    .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?;
    info.configured_port = configured;
    Ok(info)
}

/// 豆包 OpenAI 兼容接口的当前状态（实际端口 + 配置端口 + API Key + 是否在跑）。
pub async fn model_api_info(
    state: &AppState,
) -> std::result::Result<crate::app::model_api::ModelApiInfo, String> {
    fill_configured_port(state.db.clone(), crate::app::model_api::info()).await
}

/// 手动启动接口（会记住「用户要它开着」，重启后自动恢复）。
pub async fn model_api_start(
    state: &AppState,
) -> std::result::Result<crate::app::model_api::ModelApiInfo, String> {
    let info = crate::app::model_api::start(state.db.clone())
        .await
        .map_err(err_text)?;
    let info = fill_configured_port(state.db.clone(), info).await?;
    crate::app::log::action(
        "启动模型接口",
        "user",
        true,
        0,
        format!("监听 127.0.0.1:{}", info.port),
    );
    Ok(info)
}

/// 重启监听：排障用（端口漂移、客户端连不上时先重启一次）。
pub async fn model_api_restart(
    state: &AppState,
) -> std::result::Result<crate::app::model_api::ModelApiInfo, String> {
    let info = crate::app::model_api::restart(state.db.clone())
        .await
        .map_err(err_text)?;
    let info = fill_configured_port(state.db.clone(), info).await?;
    crate::app::log::action(
        "重启模型接口",
        "user",
        true,
        0,
        format!("监听 127.0.0.1:{}", info.port),
    );
    Ok(info)
}

/// 运行统计：请求数、失败数、平均耗时与最近 20 次调用。
pub fn model_api_stats() -> crate::app::model_api::ModelApiStats {
    crate::app::model_api::stats()
}

/// 带 Key 真发一次 `/v1/models` 的自检。
pub async fn model_api_probe() -> std::result::Result<crate::app::model_api::ModelApiProbe, String>
{
    crate::app::model_api::probe().await.map_err(err_text)
}

/// 手动停止接口（会记住「用户要它关着」，重启后不会自己回来）。
pub async fn model_api_stop(
    state: &AppState,
) -> std::result::Result<crate::app::model_api::ModelApiInfo, String> {
    let info = crate::app::model_api::stop(state.db.clone())
        .await
        .map_err(err_text)?;
    let info = fill_configured_port(state.db.clone(), info).await?;
    crate::app::log::action("停止模型接口", "user", true, 0, "已停止监听");
    Ok(info)
}

/// 读取当前可用的豆包模型（不需要接口在跑，也不用 API Key）。
pub async fn model_api_models(
    state: &AppState,
) -> std::result::Result<Vec<crate::app::model_api::ModelEntry>, String> {
    crate::app::model_api::available_models(state.db.clone())
        .await
        .map_err(err_text)
}

/// 改模型接口端口，立即生效（会重启监听）。端口被占用时自动退到空闲端口。
pub async fn model_api_set_port(
    state: &AppState,
    port: u16,
) -> std::result::Result<crate::app::model_api::ModelApiInfo, String> {
    let info = crate::app::model_api::set_port(state.db.clone(), port)
        .await
        .map_err(err_text)?;
    crate::app::log::action(
        "模型接口端口",
        "user",
        true,
        0,
        format!(
            "端口改为 {}（实际监听 {}）",
            info.configured_port, info.port
        ),
    );
    Ok(info)
}

/// 重新生成 API Key，立即生效（旧 Key 作废）。
pub async fn model_api_regenerate_key(
    state: &AppState,
) -> std::result::Result<crate::app::model_api::ModelApiInfo, String> {
    crate::app::model_api::regenerate_key(state.db.clone())
        .await
        .map_err(err_text)
}
