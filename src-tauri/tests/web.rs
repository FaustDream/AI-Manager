//! HTTP 壳全链路验证：真实 axum HTTP（/cmd + /events）+ 真实命令函数，
//! 直接持有 AppState，不经过任何桌面运行时。
//!
//! 安全面：缺自定义头 / 恶意 Origin 必须 403；本机 Origin（dev 5273 与
//! 托管自身 5274）与无 Origin 的本机工具放行。静态托管：命中文件给文件、
//! 未知路径回落 index.html（SPA）。

use ai_manager::desktop::{self, web, AppState};
use serde_json::{json, Value};

/// `set_data_dir_for_test` 是进程级 override：并行测试会互相改目录，
/// 导致 SQLite 锁到别人的临时库。用异步锁把整条用例串起来——同步
/// MutexGuard 持锁跨 await 会阻塞运行时线程（clippy::await_holding_lock）。
static DATA_DIR_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 独立临时数据目录（CORE-03：测试绝不碰真实 data/）+ 与生产同源的 init_state。
/// TempDir 必须活到用例结束：提前 drop 会把正在用的库删掉。
struct TestCtx {
    _dir: tempfile::TempDir,
    state: AppState,
}

fn test_ctx() -> TestCtx {
    let dir = tempfile::tempdir().expect("tempdir");
    ai_manager::app::paths::set_data_dir_for_test(dir.path());
    let state = desktop::init_state(None).expect("init_state");
    TestCtx { _dir: dir, state }
}

async fn call(state: &AppState, name: &str, args: Value) -> Result<Value, String> {
    web::dispatch(state, name, &args).await
}

#[tokio::test]
async fn app_version_over_dispatch() {
    let _guard = DATA_DIR_LOCK.lock().await;
    let ctx = test_ctx();
    let v = call(&ctx.state, "app_version", json!({})).await.unwrap();
    assert_eq!(v, json!(env!("CARGO_PKG_VERSION")));
}

#[tokio::test]
async fn unknown_command_returns_args_error() {
    let _guard = DATA_DIR_LOCK.lock().await;
    let ctx = test_ctx();
    let err = call(&ctx.state, "no_such_command", json!({}))
        .await
        .unwrap_err();
    let parsed: Value = serde_json::from_str(&err).unwrap();
    // args_error 走 ErrorKind::Invalid（序列化 kind=invalid），前端 errMsg 同口径
    assert_eq!(parsed["kind"], "invalid");
}

#[tokio::test]
async fn missing_arg_returns_args_error() {
    let _guard = DATA_DIR_LOCK.lock().await;
    let ctx = test_ctx();
    // account_set_alias 需要 iid / alias，空参必须报「缺少参数」
    let err = call(&ctx.state, "account_set_alias", json!({}))
        .await
        .unwrap_err();
    let parsed: Value = serde_json::from_str(&err).unwrap();
    assert_eq!(parsed["kind"], "invalid");
    assert!(parsed["message"].as_str().unwrap().contains("iid"));
}

#[tokio::test]
async fn full_http_roundtrip_and_security() {
    let _guard = DATA_DIR_LOCK.lock().await;
    let TestCtx { _dir, state } = test_ctx();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { web::serve(listener, state).await.unwrap() });

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/cmd/app_version");

    // 缺自定义头 → 403（CSRF 防线）
    let resp = client.post(&url).send().await.unwrap();
    assert_eq!(resp.status(), 403);

    // 恶意 Origin → 403
    let resp = client
        .post(&url)
        .header("x-ai-manager-web", "1")
        .header("Origin", "https://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    // 本机工具形态（带头、无 Origin）→ 200 + 真实数据
    let resp = client
        .post(&url)
        .header("x-ai-manager-web", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["data"], json!(env!("CARGO_PKG_VERSION")));

    // 托管页面同源形态：生产里页面与命令通道同端口（5274），浏览器对同源
    // POST 也会带 Origin，白名单漏了它整个界面就全 403。
    let resp = client
        .post(&url)
        .header("x-ai-manager-web", "1")
        .header("Origin", "http://127.0.0.1:5274")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // 白名单外的本机端口不在信任面内（测试服务器自己就是随机端口）
    let resp = client
        .post(&url)
        .header("x-ai-manager-web", "1")
        .header("Origin", format!("http://{addr}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    // dev server 的 Origin → 放行
    let resp = client
        .post(&url)
        .header("x-ai-manager-web", "1")
        .header("Origin", "http://127.0.0.1:5273")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // 未知命令 → 200 + ok:false + 结构化 args 错误
    let resp = client
        .post(format!("http://{addr}/cmd/no_such_command"))
        .header("x-ai-manager-web", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], false);
    let err: Value = serde_json::from_str(body["error"].as_str().unwrap()).unwrap();
    assert_eq!(err["kind"], "invalid");
    drop(_dir);
}

#[tokio::test]
async fn static_hosting_serves_files_and_spa_fallback() {
    let _guard = DATA_DIR_LOCK.lock().await;
    let TestCtx { _dir, state } = test_ctx();
    // 临时 dist：index.html + 一个 js 产物，验证托管与回落
    let dist = tempfile::tempdir().unwrap();
    std::fs::write(dist.path().join("index.html"), "<html>spa-entry</html>").unwrap();
    std::fs::create_dir_all(dist.path().join("assets")).unwrap();
    std::fs::write(dist.path().join("assets/app.js"), "console.log(1)").unwrap();
    std::env::set_var("AIMANAGER_WEB_DIST", dist.path());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { web::serve(listener, state).await.unwrap() });
    let client = reqwest::Client::new();

    // 根路径 → index.html
    let resp = client.get(format!("http://{addr}/")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
    assert!(resp.text().await.unwrap().contains("spa-entry"));

    // 真实产物 → 对应 mime
    let resp = client
        .get(format!("http://{addr}/assets/app.js"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/javascript"));

    // SPA 路由（无对应文件）→ 回落 index.html
    let resp = client
        .get(format!("http://{addr}/accounts"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.text().await.unwrap().contains("spa-entry"));

    // 路径穿越 → 不给文件，回落 index（防线在拼接之前）。
    // 必须用裸 TCP 发未归一化的路径：reqwest 会在客户端把 /../ 折叠掉，测不到防线。
    {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
        sock.write_all(b"GET /../secret HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut raw = String::new();
        sock.read_to_string(&mut raw).await.unwrap();
        assert!(raw.contains("200"), "穿越路径应回落 index：{raw}");
        assert!(
            raw.contains("spa-entry"),
            "穿越路径不得返回任何真实文件：{raw}"
        );
    }

    std::env::remove_var("AIMANAGER_WEB_DIST");
    drop(_dir);
}

#[test]
fn error_text_is_the_json_the_frontend_parses() {
    // 前端 errText 认识 {kind, message}；命令的 error 字符串就是 AppError 的
    // serde 形状（desktop::err_text 只做 to_string），这里直接钉住序列化契约
    let e =
        ai_manager::app::error::AppError::new(ai_manager::app::error::ErrorKind::Io, "连接超时");
    let text = serde_json::to_string(&e).unwrap();
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["kind"], "io");
    assert_eq!(parsed["message"], "连接超时");
}

/// 回归：**完整档案的 WorkBuddy 账号必须可切**。
///
/// 曾因预检把完整 iid 当 uid 传给 `pack_ready`，档案路径被拼成
/// `profiles\cn\workbuddy:<uid>`（冒号不是合法目录名），永远判「无档案」——
/// 于是所有 WorkBuddy 账号的切换按钮全灰，而真正的切换写入路径是对的，
/// 也就永远不会有人发现。这里从 `account_switchable` 入口整条链路钉死。
#[tokio::test]
async fn workbuddy_account_with_complete_pack_is_switchable() {
    let _guard = DATA_DIR_LOCK.lock().await;
    let ctx = test_ctx();

    // 完整档案 = `profiles/cn/<uid>/` 下有 `app/session` 目录 + `auth.info` 文件
    let pack = ctx._dir.path().join("profiles").join("cn").join("u1");
    std::fs::create_dir_all(pack.join("app/session")).unwrap();
    std::fs::write(pack.join("auth.info"), b"{\"account\":{\"uid\":\"u1\"}}").unwrap();

    // 入库账号 + 一份能解出的凭据（set_secret 走真实 DPAPI 加密）
    let identity = ai_manager::app::accounts::AccountIdentity {
        iid: "workbuddy:u1".into(),
        platform: "workbuddy".into(),
        platform_label: "WorkBuddy 国内版".into(),
        account_id: "u1".into(),
        name: "测试".into(),
        requires_checkin: true,
        has_secret: true,
        ..Default::default()
    };
    let db = ai_manager::app::db::Db::open().expect("打开临时库");
    ai_manager::app::accounts::repo::upsert_identity(&db, &identity, 1).unwrap();
    let mut cred = ai_manager::app::accounts::credential::Credential {
        token: "tok".into(),
        ..Default::default()
    };
    cred.put("uid", serde_json::json!("u1"));
    ai_manager::app::accounts::repo::set_secret(&db, "workbuddy:u1", &cred.to_bytes().unwrap(), None)
        .unwrap();

    // dispatch 返回命令的裸返回值（HTTP 层才包 {ok,data} 信封）
    let v = call(&ctx.state, "account_switchable", json!({})).await.unwrap();
    let list = v
        .as_array()
        .unwrap_or_else(|| panic!("switchable 响应异常：{v}"));
    assert!(
        list.iter().any(|s| s == "workbuddy:u1"),
        "有完整档案的 WorkBuddy 账号必须可切，实际：{list:?}"
    );
}
