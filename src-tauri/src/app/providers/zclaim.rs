//! ZCode 活动套餐的查询与领取（v1 `zclaim.py` / `zcaptcha.py` 的直译）。
//!
//! 领取必须带阿里云验证码参数（上游强制），验证码只能在浏览器里跑（官方 SDK）：
//! 起一个只监听 127.0.0.1 随机端口的临时页面 → 用户在浏览器里完成无痕验证 →
//! 参数 POST 回来 → 关服务。窗口关掉就结束，不留常驻端口——这不是 D3 禁的
//! 「本机 IPC 端口」，是用户交互的回调通道，v1 同款。
//! 活动来源两条：billing/preview（常规）与 marketing/touch（客户端弹窗），都查才不漏。

use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::{Http, HttpRequest};
use crate::app::providers::zcode::{ZCodeProvider, APP_VERSION};
use serde::Serialize;
use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpListener;

const API_BASE: &str = "https://zcode.z.ai";
/// v1 同款：验证码页最长等 3 分钟，超时让用户重新发起。
const CAPTCHA_TIMEOUT_SECS: u64 = 180;
const DEFAULT_CAPTCHA: (&str, &str, &str) = ("cn", "no8xfe", "11xygtvd");

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct Campaign {
    pub title: String,
    pub position: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
/// 一个可领取的活动套餐。`grants` 是给人看的明细行，界面直接列出来。
#[derive(Debug, Clone, Serialize)]
pub struct ClaimPlan {
    pub plan_id: String,
    pub name: String,
    pub description: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub priority: i64,
    pub grants: Vec<String>,
    /// 权益真正生效的时刻（毫秒）；在 entitlement 上，plan.starts_at 只是活动开始
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub effective_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub campaign: Option<Campaign>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct ClaimOutcome {
    pub name: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub effective_at: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub ends_at: i64,
}

// ------------------------------------------------------------------ 查询

/// preview 为主、marketing 补充：同 id 以 preview 为准（字段更全）。
pub async fn claimable(
    cred: &crate::app::accounts::credential::Credential,
    http: &dyn Http,
    mid: Option<&str>,
) -> Result<Vec<ClaimPlan>> {
    let mut plans = preview(cred, http, mid).await?;
    let seen: std::collections::HashSet<String> =
        plans.iter().map(|item| item.plan_id.clone()).collect();
    for item in marketing_plans(cred, http, mid).await {
        if !seen.contains(&item.plan_id) {
            plans.push(item);
        }
    }
    Ok(plans)
}

fn preview_url(version: &str) -> String {
    format!("{API_BASE}/api/v1/zcode-plan/billing/preview?app_version={version}&platform=win32-x64")
}

/// 诊断入口：原样返回 `billing/preview` 的 JSON。
///
/// 平台换字段时，「列表突然变空」和「真没活动」在解析后长得一模一样。
/// 真机探针（`tests/live_probe.rs`）用这个直接看原始结构，比猜字段名快。
pub async fn preview_payload(
    cred: &crate::app::accounts::credential::Credential,
    http: &dyn Http,
    mid: Option<&str>,
) -> Result<Value> {
    preview_payload_with(cred, http, mid, APP_VERSION).await
}

/// 同上，但可指定 `app_version`：活动下发是按客户端版本分级的，
/// 排查「官方能领、这里没有」时需要换版本试。
pub async fn preview_payload_with(
    cred: &crate::app::accounts::credential::Credential,
    http: &dyn Http,
    mid: Option<&str>,
    version: &str,
) -> Result<Value> {
    get_ok(cred, http, mid, &preview_url(version), "活动预览").await
}

async fn preview(
    cred: &crate::app::accounts::credential::Credential,
    http: &dyn Http,
    mid: Option<&str>,
) -> Result<Vec<ClaimPlan>> {
    let data = preview_payload(cred, http, mid).await?;
    // 平台没下发活动时只回 {"code":0,"msg":""}，没有 data 节点——这不是解析失败
    let data = if data.get("data").is_some() {
        data
    } else {
        return Ok(Vec::new());
    };
    let mut plans: Vec<ClaimPlan> = plans_of(&data).into_iter().filter_map(parse_plan).collect();
    plans.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.plan_id.cmp(&b.plan_id))
    });
    Ok(plans)
}

/// 诊断入口：原样返回 `marketing/touch` 的 JSON（真机探针用）。
pub async fn marketing_payload(
    cred: &crate::app::accounts::credential::Credential,
    http: &dyn Http,
    mid: Option<&str>,
    seq: u32,
) -> Result<Value> {
    let url = format!("{API_BASE}/api/v1/marketing/touch?seq={seq}");
    get_ok(cred, http, mid, &url, "活动下发").await
}

/// 活动弹窗通道：失败一律当没有活动，不影响 preview。
async fn marketing_plans(
    cred: &crate::app::accounts::credential::Credential,
    http: &dyn Http,
    mid: Option<&str>,
) -> Vec<ClaimPlan> {
    let Ok(data) = marketing_payload(cred, http, mid, 0).await else {
        return Vec::new();
    };
    let deliveries = data
        .pointer("/data/deliveries")
        .and_then(|v| v.as_array())
        .cloned();
    let mut out: std::collections::BTreeMap<String, ClaimPlan> = std::collections::BTreeMap::new();
    for delivery in deliveries.unwrap_or_default() {
        let campaign = Campaign {
            title: text_of(&delivery, &["title"]),
            position: text_of(&delivery, &["position", "type"]),
        };
        for raw in walk_plans(&delivery) {
            if let Some(mut plan) = parse_plan(raw) {
                plan.campaign = Some(campaign.clone());
                out.entry(plan.plan_id.clone()).or_insert(plan);
            }
        }
    }
    out.into_values().collect()
}

// ------------------------------------------------------------------ 领取

/// 完整领取：验证码 → 提交。验证码被拒（3007）不自动重试，交给用户重新发起。
pub async fn claim(
    cred: &crate::app::accounts::credential::Credential,
    http: &dyn Http,
    mid: Option<&str>,
    plan_id: &str,
) -> Result<ClaimOutcome> {
    let (param, region) = captcha_param().await?;
    submit(cred, http, mid, plan_id, &param, &region).await
}

async fn submit(
    cred: &crate::app::accounts::credential::Credential,
    http: &dyn Http,
    mid: Option<&str>,
    plan_id: &str,
    param: &str,
    region: &str,
) -> Result<ClaimOutcome> {
    if param.trim().is_empty() {
        return Err(AppError::new(ErrorKind::Provider, "验证码参数为空"));
    }
    let tokens = ZCodeProvider::candidate_tokens(cred);
    let Some(token) = tokens.first() else {
        return Err(AppError::new(
            ErrorKind::Provider,
            "没有可用的登录凭据，请先保存该账号的登录",
        ));
    };
    let mut headers = ZCodeProvider::zai_headers(cred, token, mid);
    headers.push((
        "X-Aliyun-Captcha-Verify-Param".into(),
        param.trim().to_string(),
    ));
    if !region.trim().is_empty() {
        headers.push((
            "X-Aliyun-Captcha-Verify-Region".into(),
            region.trim().to_string(),
        ));
    }
    let url = format!("{API_BASE}/api/v1/zcode-plan/billing/claim");
    let req = HttpRequest::post(&url)
        .headers(&headers)
        .json_body(&serde_json::json!({ "plan_id": plan_id }));
    let resp = http
        .send(req)
        .await
        .map_err(|e| AppError::new(ErrorKind::Provider, format!("领取请求失败：{}", e.message)))?;
    if resp.status == 401 || resp.status == 403 {
        // 401/403 也可能是人机校验挑战，不能一律当成登录失效。
        // v1 还会看响应头 x-aliyun-captcha-verify-param；我们的 HttpResponse 不带
        // 响应头，用 body 判别，实测上游两种都会在 body 里带 captcha 字样。
        if resp.body.to_lowercase().contains("captcha") {
            return Err(AppError::new(ErrorKind::Provider, "需要重新完成人机校验"));
        }
        return Err(AppError::new(
            ErrorKind::Provider,
            "登录态已失效，请重新登录该账号",
        ));
    }
    let data = resp
        .json()
        .map_err(|_| AppError::new(ErrorKind::Provider, "领取接口返回的不是 JSON"))?;
    let code = data.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 && code != 200 {
        return Err(AppError::new(
            ErrorKind::Provider,
            failure_message(code, &data),
        ));
    }
    let plan = data.pointer("/data/plan").cloned().unwrap_or(Value::Null);
    Ok(ClaimOutcome {
        name: plan
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        effective_at: plan.get("starts_at").and_then(|v| v.as_i64()).unwrap_or(0) * 1000,
        ends_at: plan.get("ends_at").and_then(|v| v.as_i64()).unwrap_or(0) * 1000,
    })
}

fn failure_message(code: i64, body: &Value) -> String {
    let base = match code {
        1001 => "活动不存在或已结束",
        1002 => "该活动不支持领取",
        1003 => "领取条件未满足",
        1004 => "该账号已领取过",
        1005 => "领取过于频繁，请稍后再试",
        3001 => "领取失败，请稍后重试",
        3007 => "验证码校验未通过，请重新发起领取",
        401 => "登录态已失效，请重新登录",
        _ => return format!("领取失败（code {code}）"),
    };
    let server = body
        .get("msg")
        .or_else(|| body.get("message"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim();
    if server.is_empty() {
        base.to_string()
    } else {
        format!("{base}：{server}")
    }
}

// ------------------------------------------------------------------ 内部：请求与解析

/// GET + 业务码校验。401/403 一律当登录失效（活动接口没有退避重试的必要）。
async fn get_ok(
    cred: &crate::app::accounts::credential::Credential,
    http: &dyn Http,
    mid: Option<&str>,
    url: &str,
    what: &str,
) -> Result<Value> {
    let token = ZCodeProvider::candidate_tokens(cred)
        .first()
        .cloned()
        .ok_or_else(|| {
            AppError::new(
                ErrorKind::Provider,
                "没有可用的登录凭据，请先保存该账号的登录",
            )
        })?;
    let req = HttpRequest::get(url).headers(&ZCodeProvider::zai_headers(cred, &token, mid));
    let resp = http.send(req).await.map_err(|e| {
        AppError::new(
            ErrorKind::Provider,
            format!("{what}请求失败：{}", e.message),
        )
    })?;
    if resp.status == 401 || resp.status == 403 {
        return Err(AppError::new(
            ErrorKind::Provider,
            "登录态已失效，请重新登录该账号",
        ));
    }
    if resp.status >= 400 {
        return Err(AppError::new(
            ErrorKind::Provider,
            format!("{what}返回 HTTP {}", resp.status),
        ));
    }
    let data = resp
        .json()
        .map_err(|_| AppError::new(ErrorKind::Provider, format!("{what}返回的不是 JSON")))?;
    let code = data.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
    if code != 0 && code != 200 {
        return Err(AppError::new(
            ErrorKind::Provider,
            failure_message(code, &data),
        ));
    }
    Ok(data)
}

fn plans_of(data: &Value) -> Vec<&Value> {
    data.pointer("/data/plans")
        .and_then(|v| v.as_array())
        .map(|items| items.iter().filter(|item| item.is_object()).collect())
        .unwrap_or_default()
}

/// 递归找所有带 plan_id 的节点：marketing 的套餐藏在 deliveries 的任意一层。
fn walk_plans(node: &Value) -> Vec<&Value> {
    fn walk<'a>(node: &'a Value, depth: u8, out: &mut Vec<&'a Value>) {
        if depth > 8 {
            return;
        }
        match node {
            Value::Object(map) => {
                if map.contains_key("plan_id") || map.contains_key("planId") {
                    out.push(node);
                }
                for value in map.values() {
                    walk(value, depth + 1, out);
                }
            }
            Value::Array(items) => {
                for item in items {
                    walk(item, depth + 1, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(node, 0, &mut out);
    out
}

fn text_of(node: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| node.get(*key).and_then(|v| v.as_str()))
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn fmt_units(value: f64) -> String {
    let trim = |text: String| text.trim_end_matches('0').trim_end_matches('.').to_string();
    if value >= 1e8 {
        format!("{}亿", trim(format!("{:.1}", value / 1e8)))
    } else if value >= 1e4 {
        format!("{}万", trim(format!("{:.1}", value / 1e4)))
    } else {
        format!("{value:.0}")
    }
}

fn effective_at(plan: &Value) -> i64 {
    let moments: Vec<i64> = plan
        .get("entitlements")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("effective_at").and_then(|v| v.as_f64()))
                .map(|v| v as i64)
                .collect()
        })
        .unwrap_or_default();
    moments.iter().min().map(|m| m * 1000).unwrap_or(0)
}

fn parse_plan(plan: &Value) -> Option<ClaimPlan> {
    let plan_id = text_of(plan, &["plan_id", "planId"]);
    if plan_id.is_empty() {
        return None;
    }
    let mut grants = Vec::new();
    for item in plan
        .get("entitlements")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        if item.get("meter").and_then(|v| v.as_str()) != Some("model_usage")
            || item.get("unit_type").and_then(|v| v.as_str()) != Some("token")
        {
            continue;
        }
        let name = text_of(item, &["show_name", "showName"]);
        if name.is_empty() {
            continue;
        }
        let units = item
            .get("grant_units")
            .or_else(|| item.get("grantUnits"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let period = match item
            .get("period")
            .and_then(|v| v.as_str())
            .unwrap_or("one_time")
        {
            "daily" => "每日",
            "weekly" => "每周",
            "monthly" => "每月",
            _ => "一次性",
        };
        grants.push(format!("{name} · {} Token（{period}）", fmt_units(units)));
    }
    Some(ClaimPlan {
        plan_id,
        name: text_of(plan, &["name"]),
        description: text_of(plan, &["description"]),
        priority: plan.get("priority").and_then(|v| v.as_i64()).unwrap_or(0),
        grants,
        effective_at: effective_at(plan),
        campaign: None,
    })
}

// ------------------------------------------------------------------ 验证码

/// 打开浏览器过验证，返回 (验证码参数, region)。阻塞在等 POST 上，调用方放阻塞线程。
fn captcha_blocking() -> Result<(String, String)> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| AppError::new(ErrorKind::Io, format!("起验证码回调服务失败：{e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| AppError::new(ErrorKind::Io, format!("读回调端口失败：{e}")))?
        .port();
    // 场景参数先按长期观测的默认值；上游下发参数的接口需要再打一次 configs，
    // v1 拿匿名头去取，这里保持一致：取不到就用默认，不影响领取本身。
    let (region, prefix, scene_id) = DEFAULT_CAPTCHA;
    let page = captcha_page(region, prefix, scene_id);
    let url = format!("http://127.0.0.1:{port}/");

    let (tx, rx) = std::sync::mpsc::channel::<String>();
    // 超时后线程必须能自己退（模块头承诺不留常驻端口）：监听转非阻塞轮询
    // 停止位，主流程超时置位即可，不用发假连接去解阻塞
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_server = stop.clone();
    listener
        .set_nonblocking(true)
        .map_err(|e| AppError::new(ErrorKind::Io, format!("回调服务设非阻塞失败：{e}")))?;
    let server = std::thread::spawn(move || {
        while !stop_server.load(std::sync::atomic::Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false).ok();
                    let param = handle_connection(stream, &page);
                    let done = param.is_some();
                    if let Some(param) = param {
                        let _ = tx.send(param);
                    }
                    if done {
                        break;
                    }
                }
                // 非阻塞 accept 的「暂时没连接」：睡一小会儿再试
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(_) => break,
            }
        }
    });

    // 走 explorer 打默认浏览器：与 open_data_dir 同一套路，不产生控制台窗口。
    // 打不开就把地址还给用户，别让他对着 180 秒超时干等
    if let Err(e) = crate::app::process::spawn_with_args(
        &std::path::PathBuf::from("explorer.exe"),
        &[url.as_str()],
        true,
    ) {
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        return Err(AppError::new(
            ErrorKind::Provider,
            format!("浏览器打开失败（{}），请手动访问 {url}", e.message),
        ));
    }

    let param = rx.recv_timeout(std::time::Duration::from_secs(CAPTCHA_TIMEOUT_SECS));
    // 无论拿到参数还是超时，都让服务线程停轮询退出，端口随之释放
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let param =
        param.map_err(|_| AppError::new(ErrorKind::Provider, "等待验证码超时，请重新发起领取"))?;
    // detach：线程已在收尾，不等它
    let _ = server;
    if param.is_empty() {
        return Err(AppError::new(
            ErrorKind::Provider,
            "验证码参数为空，请重新发起领取",
        ));
    }
    Ok((param, region.to_string()))
}

/// 异步入口：验证码等待是纯阻塞 IO，必须丢进阻塞池。
async fn captcha_param() -> Result<(String, String)> {
    crate::app::runtime::blocking(captcha_blocking).await?
}

fn handle_connection(mut stream: std::net::TcpStream, page: &str) -> Option<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    // 读到头结束（\r\n\r\n）为止；POST 的 body 长度单独看 Content-Length
    let header_end = loop {
        match stream.read(&mut chunk) {
            Ok(0) => return None,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = find_header_end(&buf) {
                    break pos;
                }
                if buf.len() > 64 * 1024 {
                    return None;
                }
            }
            Err(_) => return None,
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let first_line = head.lines().next().unwrap_or_default();
    if first_line.starts_with("POST /done") {
        let length = head
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())?
            })
            .unwrap_or(0);
        // 头一结束就停读了，body（captchaVerifyParam 有几 KB）往往还跨着
        // TCP 段没到齐：必须读满 Content-Length 再解析，否则 JSON 截断、
        // 领取间歇性失败
        if length > 1024 * 1024 {
            return None;
        }
        let body_start = header_end + 4;
        while buf.len() < body_start + length {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
        }
        let body = &buf[body_start..(body_start + length).min(buf.len())];
        let param = serde_json::from_slice::<Value>(body)
            .ok()
            .and_then(|v| v.get("param").and_then(|p| p.as_str()).map(str::to_string))
            .unwrap_or_default();
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"ok\":true}");
        return Some(param);
    }
    let body = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\r\n{}",
        page.len(),
        page
    );
    let _ = stream.write_all(body.as_bytes());
    None
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// 验证码页：加载官方 SDK → 无痕验证 → 参数 POST 回 /done → 关窗口。
/// 整页是静态字符串，配置用占位符注入——format! 的花括号转义在这种页面里必出错。
fn captcha_page(region: &str, prefix: &str, scene_id: &str) -> String {
    let cfg = serde_json::json!({ "region": region, "prefix": prefix, "scene_id": scene_id });
    PAGE_TEMPLATE.replace("__CFG__", &cfg.to_string())
}

const PAGE_TEMPLATE: &str = r##"<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8"><title>ZCode 领取验证</title>
<style>
body{margin:0;background:#0a0a0c;color:#e8e8ee;font:14px/1.6 system-ui,"Microsoft YaHei",sans-serif;
display:flex;align-items:center;justify-content:center;height:100vh}
.box{max-width:420px;padding:28px 32px;border:1px solid #24242c;border-radius:14px;background:#131318}
h1{font-size:16px;margin:0 0 10px}
#msg{color:#9a9aa8;font-size:13px;min-height:20px}
#dot{display:inline-block;width:8px;height:8px;border-radius:50%;background:#3b82f6;margin-right:8px}
#dot.ok{background:#22c55e}#dot.err{background:#ef4444}
button{margin-top:16px;padding:8px 18px;border-radius:8px;border:1px solid #3b82f6;background:#1d4ed8;
color:#fff;font-size:13px;cursor:pointer}
#cap-holder{margin-top:14px}
</style></head>
<body><div class="box">
<h1>ZCode 活动领取</h1>
<div><span id="dot"></span><span id="msg">正在准备验证…</span></div>
<div id="cap-holder"></div>
<button id="cap-btn" hidden>点此完成验证</button>
</div>
<script>
var CFG = __CFG__, DONE = false;
function say(t, tone) {
  document.getElementById("msg").textContent = t;
  document.getElementById("dot").className = tone || "";
}
function finish(param) {
  if (DONE || !param) return;
  DONE = true;
  say("验证通过，正在提交领取…", "ok");
  fetch("/done", {method: "POST", headers: {"Content-Type": "application/json"},
    body: JSON.stringify({param: param})})
    .then(function () { say("已完成，可以关闭本页", "ok"); setTimeout(function () { window.close(); }, 1200); })
    .catch(function () { say("回传失败，请回到 AI Manager 查看", "err"); });
}
function loadSdk() {
  return new Promise(function (resolve, reject) {
    if (typeof window.initAliyunCaptcha === "function") return resolve();
    var s = document.createElement("script");
    s.src = "https://o.alicdn.com/captcha-frontend/aliyunCaptcha/AliyunCaptcha.js";
    s.onload = resolve;
    s.onerror = function () { reject(new Error("验证码组件加载失败")); };
    document.head.appendChild(s);
  });
}
loadSdk().then(function () {
  window.AliyunCaptchaConfig = {region: CFG.region, prefix: CFG.prefix};
  var timer = setTimeout(function () {
    document.getElementById("cap-btn").hidden = false;
    say("需要手动完成一次验证", "");
  }, 8000);
  window.initAliyunCaptcha({
    SceneId: CFG.scene_id, mode: "popup", language: "zh-CN", showErrorTip: false,
    element: "#cap-holder", button: "#cap-btn",
    getInstance: function (instance) {
      if (typeof instance.startTracelessVerification === "function") {
        instance.startTracelessVerification();
      } else {
        document.getElementById("cap-btn").hidden = false;
      }
    },
    success: function (p) { clearTimeout(timer); finish(typeof p === "string" ? p : p && p.captchaVerifyParam); },
    fail: function () { clearTimeout(timer); document.getElementById("cap-btn").hidden = false; say("验证未通过，请重试", "err"); },
    onError: function (e) { clearTimeout(timer); say("验证出错：" + ((e && e.message) || e), "err"); }
  });
}).catch(function (e) { say(e.message, "err"); });
</script></body></html>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plan_extracts_token_grants() {
        let plan: Value = serde_json::from_str(
            r#"{
                "plan_id": "p1", "name": "新人礼包", "description": "d", "priority": 5,
                "entitlements": [
                    {"meter": "model_usage", "unit_type": "token", "show_name": "GLM",
                     "grant_units": 1200000, "period": "daily", "effective_at": 1700000000},
                    {"meter": "other", "unit_type": "token", "show_name": "忽略", "grant_units": 1}
                ]
            }"#,
        )
        .unwrap();
        let parsed = parse_plan(&plan).unwrap();
        assert_eq!(parsed.plan_id, "p1");
        assert_eq!(parsed.grants.len(), 1);
        assert!(
            parsed.grants[0].starts_with("GLM · 120万 Token（每日）"),
            "{}",
            parsed.grants[0]
        );
        assert_eq!(parsed.effective_at, 1_700_000_000_000);
        assert!(parse_plan(&serde_json::json!({"name": "没有 id"})).is_none());
    }

    #[test]
    fn failure_messages_cover_known_codes() {
        assert_eq!(failure_message(1004, &Value::Null), "该账号已领取过");
        assert_eq!(
            failure_message(3007, &Value::Null),
            "验证码校验未通过，请重新发起领取"
        );
        assert_eq!(failure_message(9999, &Value::Null), "领取失败（code 9999）");
        assert_eq!(
            failure_message(1001, &serde_json::json!({"msg": "下架"})),
            "活动不存在或已结束：下架"
        );
    }

    #[tokio::test]
    async fn claimable_merges_marketing_and_sorts_by_priority() {
        let http = crate::app::net::MockHttp::new();
        http.on(
            &format!("{API_BASE}/api/v1/zcode-plan/billing/preview?app_version={APP_VERSION}&platform=win32-x64"),
            200,
            r#"{"code":200,"data":{"plans":[
                {"plan_id":"a","name":"常规","priority":1,"entitlements":[]}
            ]}}"#,
        );
        http.on(
            &format!("{API_BASE}/api/v1/marketing/touch?seq=0"),
            200,
            r#"{"code":200,"data":{"deliveries":[
                {"title":"弹窗活动","position":"popup","plan":{"planId":"b","name":"弹窗套餐","priority":9}}
            ]}}"#,
        );
        let cred = crate::app::accounts::credential::Credential::default();
        // 没有凭据 → 明确报错而不是空列表（调用方要能区分「没活动」和「没登录」）
        let err = claimable(&cred, &http, None).await.unwrap_err();
        assert!(
            err.message.contains("没有可用的登录凭据"),
            "{}",
            err.message
        );
    }

    #[tokio::test]
    async fn marketing_failure_degrades_to_empty_not_error() {
        // 没有凭据时 marketing 通道直接失败——最终报错仍是「没有凭据」，
        // 但这条用例钉住的是：marketing 的 Err 不许把它上面的 preview 错误盖掉
        let http = crate::app::net::MockHttp::new();
        let cred = crate::app::accounts::credential::Credential::default();
        let err = claimable(&cred, &http, None).await.unwrap_err();
        assert!(err.message.contains("没有可用的登录凭据"));
    }
}
