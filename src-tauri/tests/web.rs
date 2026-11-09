//! 网页调试壳的分发器测试。放在集成测试里有一个硬理由：mock 运行时会把
//! tauri 的 app 代码单元拖进测试 exe（连带 comctl32 v6 导入），lib 单测
//! 二进制没有 manifest 会直接 STATUS_ENTRYPOINT_NOT_FOUND；集成测试 exe
//! 由 build.rs 的 rustc-link-arg-tests 嵌了 manifest，没有这个问题。

use ai_manager::desktop::{self, web::dispatch};
use serde_json::{json, Value};
use tauri::Manager;

/// 用 `set_data_dir_for_test` 把数据目录钉到独立临时目录（CORE-03），
/// 再走与两个壳完全一致的 `init_state` 装配——测试的 AppState 与生产同源。
/// 每个测试文件是独立进程，互不污染 override。
struct TestCtx {
    _dir: tempfile::TempDir,
    _app: tauri::App<tauri::test::MockRuntime>,
    handle: tauri::AppHandle<tauri::test::MockRuntime>,
}

fn test_app() -> TestCtx {
    let dir = tempfile::tempdir().expect("tempdir");
    ai_manager::app::paths::set_data_dir_for_test(dir.path());
    let state = desktop::init_state(None).expect("init_state");
    let app = tauri::test::mock_app();
    app.manage(state);
    let handle = app.app_handle().clone();
    TestCtx {
        _dir: dir,
        _app: app,
        handle,
    }
}

#[tokio::test]
async fn dispatch_roundtrips_simple_commands() {
    let ctx = test_app();
    let data = dispatch(&ctx.handle, "app_version", &json!({}))
        .await
        .expect("app_version 应成功");
    assert_eq!(data, Value::String(env!("CARGO_PKG_VERSION").to_string()));
}

#[tokio::test]
async fn unknown_command_is_a_structured_error() {
    let ctx = test_app();
    let err = dispatch(&ctx.handle, "definitely_not_a_command", &json!({}))
        .await
        .expect_err("未知命令必须报错");
    assert!(err.contains("未知命令"), "错误原文：{err}");
    assert!(
        err.contains("kind"),
        "错误必须带 kind（前端 errText 依赖）：{err}"
    );
}

#[tokio::test]
async fn missing_required_arg_fails_before_business_runs() {
    let ctx = test_app();
    let err = dispatch(&ctx.handle, "account_switch", &json!({}))
        .await
        .expect_err("缺 iid 必须报参数错");
    assert!(err.contains("iid"), "错误要指名缺哪个参数：{err}");
}

#[tokio::test]
async fn session_page_reads_empty_index() {
    let ctx = test_app();
    let data = dispatch(
        &ctx.handle,
        "session_page",
        &json!({ "cursor": null, "limit": 5, "filter": null }),
    )
    .await
    .expect("空索引上分页查询应成功");
    assert!(data.is_object(), "返回应是 Page 对象：{data}");
}

/// Result 型命令的回包必须解包成裸载荷：invoke 的语义是「成功给载荷」，
/// 整包序列化会产出 `{"Ok":…}` 壳——前端拿壳当数组用直接崩
/// （rows.reduce is not a function，2026-11-08 网页版排查）。
#[tokio::test]
async fn result_commands_return_bare_payload() {
    let ctx = test_app();
    let data = dispatch(&ctx.handle, "account_list", &json!({}))
        .await
        .expect("account_list 应成功");
    assert!(data.is_array(), "Result 命令必须解包，而不是包成壳：{data}");
}

/// 失败的 Result 命令必须走 Err 通道：失败被包进 `Ok({"Err":…})` 会让
/// 前端把错误当成功（ok:true + Err 载荷）。
#[tokio::test]
async fn result_command_failures_are_errors() {
    let ctx = test_app();
    let err = dispatch(
        &ctx.handle,
        "activity_daily_status",
        &json!({ "iid": "workbuddy:no-such@intl" }),
    )
    .await
    .expect_err("失败命令必须走 Err，而不是 Ok 包着 Err 壳");
    assert!(err.contains("kind"), "错误必须带 kind：{err}");
}

/// HTTP 全链路（axum 路由 + 安全面 + 回包形状）：临时端口起真服务，
/// 用 reqwest 打三类请求——缺自定义头、伪造 Origin、正常调用。
#[tokio::test]
async fn http_layer_enforces_guard_and_envelope() {
    use ai_manager::desktop::web;

    let dir = tempfile::tempdir().expect("tempdir");
    ai_manager::app::paths::set_data_dir_for_test(dir.path());
    let state = desktop::init_state(None).expect("init_state");

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    // serve 的 future 持有 App 本体（非 Send），不能 tokio::spawn——
    // 在专用线程上 block_on，与 bin 入口的跑法一致；MockRuntime 没有
    // tao 的主线程限制，可以在这个线程上直接建
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let app = tauri::test::mock_app();
        rt.block_on(web::serve(listener, app, state))
            .expect("serve");
    });

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/cmd/app_version");

    // 1) 缺自定义头：浏览器跨站请求会被强制先走预检（我们不回预检），
    //    直连 POST 也必须被安全面拦下
    let res = client.post(&url).send().await.expect("request");
    assert_eq!(res.status(), reqwest::StatusCode::FORBIDDEN, "缺头必须 403");

    // 2) 带头但 Origin 可疑：同样 403
    let res = client
        .post(&url)
        .header("x-ai-manager-web", "1")
        .header("Origin", "http://evil.example")
        .send()
        .await
        .expect("request");
    assert_eq!(
        res.status(),
        reqwest::StatusCode::FORBIDDEN,
        "坏 Origin 必须 403"
    );

    // 3) 正常调用：回包 {ok:true, data}，data 是版本号
    let res = client
        .post(&url)
        .header("x-ai-manager-web", "1")
        .json(&json!({}))
        .send()
        .await
        .expect("request");
    assert!(res.status().is_success());
    let body: Value = res.json().await.expect("json");
    assert_eq!(body["ok"], true, "回包：{body}");
    assert_eq!(body["data"], env!("CARGO_PKG_VERSION"), "回包：{body}");

    // 4) 未知命令：HTTP 200 + ok:false（与 invoke 的 reject 语义对齐）
    let res = client
        .post(format!("http://{addr}/cmd/definitely_not_a_command"))
        .header("x-ai-manager-web", "1")
        .json(&json!({}))
        .send()
        .await
        .expect("request");
    let body: Value = res.json().await.expect("json");
    assert_eq!(body["ok"], false, "回包：{body}");
}
