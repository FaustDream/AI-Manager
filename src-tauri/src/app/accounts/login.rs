//! 浏览器登录：申请登录地址 → 打开浏览器 → 轮询换 token → 直接入库。
//!
//! 移植自 v1 的 `logins.py` / `zlogin.py`。三家都不是 OAuth 回调模式，所以
//! **不需要本地 HTTP 服务器**：
//!   - WorkBuddy：`auth/state` 拿 state，`auth/token?state=` 长轮询；
//!   - Qoder：PKCE challenge + `deviceToken/poll` 长轮询；
//!   - ZCode：`oauth/cli/init` 拿 authorize_url + flow_id，`oauth/cli/poll/{id}` 轮询。
//!
//! 这条路的价值：**客户端完全不受影响**。借客户端登录要关掉客户端、挪走登录态、
//! 再重新打开，用户看到的是「我的软件被关了」，所以除 Trae（登录态设备绑定、
//! 只读）外全部走浏览器。

use crate::app::accounts::credential::Credential;
use crate::app::accounts::{repo, AccountIdentity};
use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::{Http, HttpRequest, RealHttp};
use crate::app::providers::zcode as zcode_provider;
use crate::app::timeutil::now_ms;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{Local, TimeZone};
use serde::Serialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

/// 登录有效期：够用户扫码/输密码，超时就当放弃
const LOGIN_TIMEOUT_SECS: u64 = 5 * 60;
const QODER_TIMEOUT_SECS: u64 = 15 * 60;

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct LoginStart {
    /// 要在浏览器里打开的地址
    pub url: String,
    pub platform: String,
    pub label: String,
}

/// 内部用：`ticket` 是换 token 需要的凭据（state / nonce+verifier / flow_id），
/// **不出现在返回给界面的结构里**。
pub struct Pending {
    pub start: LoginStart,
    pub ticket: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct LoginOutcome {
    pub ok: bool,
    /// 成功时是昵称，失败时是原因
    pub message: String,
    pub iid: String,
}

// ------------------------------------------------------------------ 入口

/// 这个平台支持浏览器登录吗（Trae 的登录态设备绑定，只读）。
pub fn supported(platform: &str) -> bool {
    matches!(
        platform,
        "workbuddy" | "workbuddy_intl" | "qoder" | "qoder_intl" | "zcode"
    )
}

pub async fn start(platform: &str) -> Result<Pending> {
    match platform {
        "workbuddy" | "workbuddy_intl" => start_workbuddy(platform).await,
        "qoder" | "qoder_intl" => start_qoder(platform).await,
        "zcode" => start_zcode().await,
        other => Err(AppError::new(
            ErrorKind::Unsupported,
            format!("{other} 不支持浏览器登录"),
        )),
    }
}

/// 阻塞等登录完成并入库。命令层用 `runtime::blocking` 包起来。
pub fn wait_for_login(db: &Db, platform: &str, ticket: &str) -> Result<LoginOutcome> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::new(ErrorKind::Io, format!("建轮询运行时失败：{e}")))?;
    runtime.block_on(async {
        match platform {
            "workbuddy" | "workbuddy_intl" => wait_workbuddy(db, platform, ticket).await,
            "qoder" | "qoder_intl" => wait_qoder(db, platform, ticket).await,
            "zcode" => wait_zcode(db, ticket).await,
            other => Err(AppError::new(
                ErrorKind::Unsupported,
                format!("{other} 不支持浏览器登录"),
            )),
        }
    })
}

// -------------------------------------------------------------- WorkBuddy

const WB_UA: &str = "WorkBuddy/5.5.6";
const WB_APP_VERSION: &str = "2.0.0";
const CODE_PENDING: i64 = 11217;
const CODE_ACCOUNT_PENDING: i64 = 12151;

fn wb_endpoint(intl: bool) -> &'static str {
    if intl {
        "https://www.workbuddy.ai"
    } else {
        "https://copilot.tencent.com"
    }
}

async fn start_workbuddy(platform: &str) -> Result<Pending> {
    let intl = platform.ends_with("_intl");
    let http = RealHttp::new();
    let mut req = HttpRequest::post(format!(
        "{}/v2/plugin/auth/state?platform=workbuddy",
        wb_endpoint(intl)
    ));
    req.headers = vec![
        ("User-Agent".into(), WB_UA.into()),
        // 三个 X-No-* 是 v1 实测必须的：带上它们才不会被要求先有登录态
        ("X-No-Authorization".into(), "true".into()),
        ("X-No-User-Id".into(), "true".into()),
        ("X-No-Enterprise-Id".into(), "true".into()),
        ("X-No-Department-Info".into(), "true".into()),
        ("Content-Type".into(), "application/json".into()),
    ];
    req.body = Some("{}".into());

    let resp = http.send(req).await?;
    if !resp.is_ok() {
        return Err(AppError::new(
            ErrorKind::Io,
            format!("申请登录地址失败：HTTP {}", resp.status),
        ));
    }
    let body = resp.json()?;
    let code = body.get("code").and_then(Value::as_i64).unwrap_or(0);
    if code != 0 {
        return Err(AppError::new(
            ErrorKind::Io,
            format!(
                "申请登录地址失败：{}",
                body.get("msg")
                    .and_then(Value::as_str)
                    .unwrap_or("未知错误")
            ),
        ));
    }
    let data = body.get("data").cloned().unwrap_or(Value::Null);
    let state = data
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let auth_url = data
        .get("authUrl")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if state.is_empty() || auth_url.is_empty() {
        return Err(AppError::new(ErrorKind::Io, "服务端没有返回登录地址"));
    }
    let sep = if auth_url.contains('?') { "&" } else { "?" };
    let url = format!(
        "{auth_url}{sep}version={WB_APP_VERSION}&loginSessionId={}",
        random_hex(16)
    );
    Ok(Pending {
        start: LoginStart {
            url,
            platform: platform.to_string(),
            label: wb_label(intl).to_string(),
        },
        ticket: json!({ "state": state }).to_string(),
    })
}

async fn wait_workbuddy(db: &Db, platform: &str, ticket: &str) -> Result<LoginOutcome> {
    let intl = platform.ends_with("_intl");
    let state = serde_json::from_str::<Value>(ticket)
        .ok()
        .and_then(|v| v.get("state").and_then(Value::as_str).map(str::to_string))
        .ok_or_else(|| AppError::new(ErrorKind::Invalid, "登录会话已失效，请重新发起"))?;
    let http = RealHttp::new();
    let token_url = format!(
        "{}/v2/plugin/auth/token?state={}",
        wb_endpoint(intl),
        urlencode(&state)
    );
    let deadline = now_ms() + (LOGIN_TIMEOUT_SECS as i64) * 1000;
    let mut token: Option<Value> = None;

    while now_ms() < deadline {
        if let Ok(resp) = http.get(&token_url).await {
            if resp.is_ok() {
                if let Ok(body) = resp.json() {
                    let code = body.get("code").and_then(Value::as_i64).unwrap_or(-1);
                    if code == 0 {
                        let data = body.get("data").cloned().unwrap_or(Value::Null);
                        if data
                            .get("accessToken")
                            .and_then(Value::as_str)
                            .map(|t| !t.is_empty())
                            .unwrap_or(false)
                        {
                            token = Some(data);
                            break;
                        }
                    } else if code != CODE_PENDING {
                        return Err(AppError::new(
                            ErrorKind::Io,
                            format!(
                                "登录失败：{}",
                                body.get("msg")
                                    .and_then(Value::as_str)
                                    .unwrap_or("未知错误")
                            ),
                        ));
                    }
                }
            }
        }
        sleep(1000).await;
    }

    let Some(data) = token else {
        return Err(AppError::new(ErrorKind::Io, "登录超时（5 分钟）"));
    };
    let access = str_of(&data, "accessToken");
    let refresh = str_of(&data, "refreshToken");
    let domain = str_of(&data, "domain");
    let expires_at = expiry_ms(&data);

    // 账号信息要再等一拍：token 刚下发时 account 接口还会说「没准备好」
    let mut who = Value::Null;
    while now_ms() < deadline {
        let mut req = HttpRequest::get(format!(
            "{}/v2/plugin/login/account?state={}",
            wb_endpoint(intl),
            urlencode(&state)
        ));
        req.headers = vec![
            ("User-Agent".into(), WB_UA.into()),
            ("Authorization".into(), format!("Bearer {access}")),
            ("X-Domain".into(), domain.clone()),
            ("X-No-User-Id".into(), "true".into()),
            ("X-No-Enterprise-Id".into(), "true".into()),
        ];
        if let Ok(resp) = http.send(req).await {
            if let Ok(body) = resp.json() {
                let code = body.get("code").and_then(Value::as_i64).unwrap_or(-1);
                if code == 0 {
                    who = body.get("data").cloned().unwrap_or(Value::Null);
                    break;
                }
                if code != CODE_ACCOUNT_PENDING {
                    break;
                }
            }
        }
        sleep(1000).await;
    }

    let uid = str_of(&who, "uid");
    let nickname = {
        let name = str_of(&who, "nickname");
        if name.is_empty() {
            let phone = str_of(&who, "phoneNumber");
            if phone.is_empty() {
                "WorkBuddy 账号".to_string()
            } else {
                phone
            }
        } else {
            name
        }
    };
    // 与客户端扫描同一套主键规则：国际版加 @intl，避免同 uid 两版冲突
    let account_id = if uid.is_empty() {
        format!("acct-{}", random_hex(6))
    } else if intl {
        format!("{uid}@intl")
    } else {
        uid.clone()
    };

    let mut meta = Map::new();
    meta.insert("uid".into(), json!(uid));
    meta.insert("domain".into(), json!(domain));
    meta.insert("edition".into(), json!(if intl { "intl" } else { "cn" }));
    meta.insert("refresh_token".into(), json!(refresh.clone()));
    meta.insert("account_info".into(), who);
    let credential = Credential {
        token: access,
        refresh_token: refresh,
        expires_at: iso_of(expires_at),
        meta,
    };

    save(
        db,
        AccountIdentity {
            iid: format!("workbuddy:{account_id}"),
            platform: "workbuddy".into(),
            platform_label: wb_label(intl).into(),
            account_id,
            name: nickname.clone(),
            expires_at,
            source: "浏览器登录".into(),
            requires_checkin: !intl,
            has_secret: true,
        },
        &credential,
        expires_at,
        &nickname,
    )
}

fn wb_label(intl: bool) -> &'static str {
    if intl {
        "WorkBuddy 国际版"
    } else {
        "WorkBuddy 国内版"
    }
}

// ------------------------------------------------------------------ Qoder

const QODER_CLIENT_ID: &str = "e883ade2-e6e3-4d6d-adf7-f92ceff5fdcb";

fn qoder_web(intl: bool) -> &'static str {
    if intl {
        "https://qoder.com"
    } else {
        "https://qoder.cn"
    }
}

fn qoder_api(intl: bool) -> &'static str {
    if intl {
        "https://openapi.qoder.sh"
    } else {
        "https://openapi.qoder.com.cn"
    }
}

async fn start_qoder(platform: &str) -> Result<Pending> {
    let intl = platform.ends_with("_intl");
    let verifier = URL_SAFE_NO_PAD.encode(random_bytes(64));
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let nonce = uuid_like();
    let machine_id = uuid_like();
    let url = format!(
        "{}/device/selectAccounts?challenge={}&challenge_method=S256&nonce={}&machine_id={}&client_id={}",
        qoder_web(intl),
        urlencode(&challenge),
        urlencode(&nonce),
        urlencode(&machine_id),
        QODER_CLIENT_ID
    );
    Ok(Pending {
        start: LoginStart {
            url,
            platform: platform.to_string(),
            label: qoder_label(intl).to_string(),
        },
        ticket: json!({ "nonce": nonce, "verifier": verifier, "machine_id": machine_id })
            .to_string(),
    })
}

async fn wait_qoder(db: &Db, platform: &str, ticket: &str) -> Result<LoginOutcome> {
    let intl = platform.ends_with("_intl");
    let t: Value = serde_json::from_str(ticket)
        .map_err(|_| AppError::new(ErrorKind::Invalid, "登录会话已失效，请重新发起"))?;
    let nonce = str_of(&t, "nonce");
    let verifier = str_of(&t, "verifier");
    let machine_id = str_of(&t, "machine_id");
    let http = RealHttp::new();

    let poll_url = format!(
        "{}/api/v1/deviceToken/poll?nonce={}&verifier={}&challenge_method=S256",
        qoder_api(intl),
        urlencode(&nonce),
        urlencode(&verifier)
    );
    let deadline = now_ms() + (QODER_TIMEOUT_SECS as i64) * 1000;
    let mut device: Option<Value> = None;
    while now_ms() < deadline {
        if let Ok(resp) = http.get(&poll_url).await {
            if resp.is_ok() {
                if let Ok(body) = resp.json() {
                    if !str_of(&body, "token").is_empty() {
                        device = Some(body);
                        break;
                    }
                }
            }
        }
        sleep(2000).await;
    }
    let Some(device) = device else {
        return Err(AppError::new(ErrorKind::Io, "登录超时（15 分钟）"));
    };
    let device_token = str_of(&device, "token");

    // 换成正式 access token；国内 CLI 不发 job token，4xx 时设备 token 本身就能用
    let mut access = String::new();
    let mut refresh = str_of(&device, "refresh_token");
    let mut expires_at = None;
    let mut req = HttpRequest::post(format!("{}/api/v1/me/jobToken", qoder_api(intl)));
    req.headers = vec![
        ("Authorization".into(), format!("Bearer {device_token}")),
        ("Content-Type".into(), "application/json".into()),
    ];
    req.body = Some(json!({ "clientId": QODER_CLIENT_ID }).to_string());
    match http.send(req).await {
        Ok(resp) if resp.is_ok() => {
            if let Ok(body) = resp.json() {
                access = str_of(&body, "token");
                let r = str_of(&body, "refresh_token");
                if !r.is_empty() {
                    refresh = r;
                }
                expires_at = body.get("expires_in").and_then(Value::as_i64).map(|sec| {
                    if sec > 1_000_000_000_000 {
                        sec
                    } else if sec > 1_000_000_000 {
                        sec * 1000
                    } else {
                        now_ms() + sec * 1000
                    }
                });
            }
        }
        _ => {}
    }
    if access.is_empty() {
        access = device_token.clone();
        expires_at = device_expiry(&device);
    }

    // 昵称/邮箱：登录回调只给 id 的情况很常见
    let mut uid = str_of(&device, "user_id");
    let mut name = str_of(&device, "user_name");
    let mut email = String::new();
    let mut req = HttpRequest::get(format!("{}/api/v1/userinfo", qoder_api(intl)));
    req.headers = vec![("Authorization".into(), format!("Bearer {device_token}"))];
    if let Ok(resp) = http.send(req).await {
        if resp.is_ok() {
            if let Ok(body) = resp.json() {
                let node = body.get("data").cloned().unwrap_or(body);
                email = str_of(&node, "email");
                let n = str_of(&node, "name");
                if !n.is_empty() {
                    name = n;
                }
                for key in ["id", "user_id", "uid"] {
                    let v = str_of(&node, key);
                    if !v.is_empty() {
                        uid = v;
                        break;
                    }
                }
            }
        }
    }

    let account_id = if !uid.is_empty() {
        uid
    } else if !email.is_empty() {
        email.clone()
    } else {
        format!("acct-{}", random_hex(6))
    };
    let nickname = if !name.is_empty() {
        name
    } else if !email.is_empty() {
        email.clone()
    } else {
        "Qoder 账号".to_string()
    };

    let mut meta = Map::new();
    meta.insert("refresh_token".into(), json!(refresh.clone()));
    meta.insert("machine_id".into(), json!(machine_id));
    meta.insert("device_token".into(), json!(device_token));
    meta.insert(
        "device_refresh".into(),
        json!(str_of(&device, "refresh_token")),
    );
    meta.insert("email".into(), json!(email));
    let credential = Credential {
        token: access,
        refresh_token: refresh,
        expires_at: iso_of(expires_at),
        meta,
    };

    save(
        db,
        AccountIdentity {
            iid: format!("{}:{account_id}", if intl { "qoder_intl" } else { "qoder" }),
            platform: if intl { "qoder_intl" } else { "qoder" }.into(),
            platform_label: qoder_label(intl).into(),
            account_id,
            name: nickname.clone(),
            expires_at,
            source: "浏览器登录".into(),
            requires_checkin: true,
            has_secret: true,
        },
        &credential,
        expires_at,
        &nickname,
    )
}

fn qoder_label(intl: bool) -> &'static str {
    if intl {
        "Qoder 国际版"
    } else {
        "Qoder CN"
    }
}

// ------------------------------------------------------------------ ZCode

const ZCODE_INIT: &str = "https://zcode.z.ai/api/v1/oauth/cli/init";
const ZCODE_POLL: &str = "https://zcode.z.ai/api/v1/oauth/cli/poll";
const BIGMODEL_BIZ: &str = "https://bigmodel.cn";
const BIGMODEL_ANTHROPIC: &str = "https://open.bigmodel.cn/api/anthropic";
const START_PLAN_ANTHROPIC: &str = "https://zcode.z.ai/api/v1/zcode-plan/anthropic";
const ZCODE_KEY_NAME: &str = "zcode-api-key";

async fn start_zcode() -> Result<Pending> {
    let poll_token = random_hex(32);
    let mid = uuid_like();
    let http = RealHttp::new();
    let mut req = HttpRequest::post(ZCODE_INIT);
    req.headers = zcode_headers(&poll_token, &mid);
    req.body = Some(json!({ "provider": "bigmodel" }).to_string());
    let resp = http.send(req).await?;
    let body = resp.json().unwrap_or(Value::Null);
    let code = body.get("code").and_then(Value::as_i64).unwrap_or(0);
    if code != 0 {
        return Err(AppError::new(
            ErrorKind::Io,
            format!(
                "申请登录地址失败：{}",
                body.get("msg")
                    .and_then(Value::as_str)
                    .unwrap_or("未知错误")
            ),
        ));
    }
    let data = body.get("data").cloned().unwrap_or(Value::Null);
    let authorize = str_of(&data, "authorize_url");
    let flow_id = str_of(&data, "flow_id");
    if authorize.is_empty() || flow_id.is_empty() {
        return Err(AppError::new(ErrorKind::Io, "服务端没有返回登录地址"));
    }
    let poll_token = {
        let v = str_of(&data, "poll_token");
        if v.is_empty() {
            poll_token
        } else {
            v
        }
    };
    Ok(Pending {
        start: LoginStart {
            url: authorize,
            platform: "zcode".into(),
            label: "ZCode".into(),
        },
        ticket: json!({
            "flow_id": flow_id,
            "poll_token": poll_token,
            "mid": mid,
            "interval": data.get("poll_interval_sec").and_then(Value::as_f64).unwrap_or(2.0),
        })
        .to_string(),
    })
}

async fn wait_zcode(db: &Db, ticket: &str) -> Result<LoginOutcome> {
    let t: Value = serde_json::from_str(ticket)
        .map_err(|_| AppError::new(ErrorKind::Invalid, "登录会话已失效，请重新发起"))?;
    let flow_id = str_of(&t, "flow_id");
    let poll_token = str_of(&t, "poll_token");
    let mid = str_of(&t, "mid");
    let interval = t
        .get("interval")
        .and_then(Value::as_f64)
        .unwrap_or(2.0)
        .max(1.0);
    let http = RealHttp::new();
    let poll_url = format!("{ZCODE_POLL}/{}", urlencode(&flow_id));
    let deadline = now_ms() + (LOGIN_TIMEOUT_SECS as i64) * 1000;
    let mut ready: Option<Value> = None;

    while now_ms() < deadline {
        let mut req = HttpRequest::get(&poll_url);
        req.headers = zcode_headers(&poll_token, &mid);
        match http.send(req).await {
            Ok(resp) if resp.status < 400 => {
                if let Ok(body) = resp.json() {
                    let code = body.get("code").and_then(Value::as_i64).unwrap_or(0);
                    if code != 0 {
                        return Err(AppError::new(
                            ErrorKind::Io,
                            format!(
                                "登录失败：{}",
                                body.get("msg")
                                    .and_then(Value::as_str)
                                    .unwrap_or("未知错误")
                            ),
                        ));
                    }
                    let data = body.get("data").cloned().unwrap_or(Value::Null);
                    match str_of(&data, "status").as_str() {
                        "pending" => {}
                        "failed" => {
                            return Err(AppError::new(
                                ErrorKind::Io,
                                "授权被拒绝或登录失败，请重试",
                            ))
                        }
                        "ready" => {
                            ready = Some(data);
                            break;
                        }
                        other => {
                            return Err(AppError::new(
                                ErrorKind::Io,
                                format!("登录返回了未知状态：{other}"),
                            ))
                        }
                    }
                }
            }
            Ok(resp) => {
                // 408/429 是「还没好」，其余 4xx 才当失败
                if resp.status != 408 && resp.status != 429 {
                    return Err(AppError::new(
                        ErrorKind::Io,
                        format!("登录轮询失败：HTTP {}", resp.status),
                    ));
                }
            }
            Err(_) => {}
        }
        sleep((interval * 1000.0) as u64).await;
    }

    let Some(data) = ready else {
        return Err(AppError::new(ErrorKind::Io, "登录超时（5 分钟）"));
    };

    let jwt = str_of(&data, "token");
    let node = data
        .get("bigmodel")
        .or_else(|| data.get("zai"))
        .cloned()
        .unwrap_or(Value::Null);
    let access = {
        let v = str_of(&node, "access_token");
        if v.is_empty() {
            str_of(&node, "accessToken")
        } else {
            v
        }
    };
    if jwt.is_empty() || access.is_empty() {
        return Err(AppError::new(ErrorKind::Io, "登录返回的数据不完整，请重试"));
    }

    // credentials 结构沿用客户端：zcodejwttoken + oauth:<provider>:*
    let mut user = Map::new();
    if let Some(obj) = data.get("user").and_then(Value::as_object) {
        for (key, value) in obj {
            user.insert(key.clone(), value.clone());
        }
    }
    if !user.contains_key("id") {
        if let Some(uid) = user.get("user_id") {
            user.insert("id".into(), uid.clone());
        }
    }
    let mut credentials = Map::new();
    credentials.insert("zcodejwttoken".into(), json!(jwt));
    credentials.insert("oauth:active_provider".into(), json!("bigmodel"));
    credentials.insert("oauth:bigmodel:access_token".into(), json!(access));
    credentials.insert(
        "oauth:bigmodel:user_info".into(),
        json!(Value::Object(user).to_string()),
    );
    let refresh = {
        let v = str_of(&node, "refresh_token");
        if v.is_empty() {
            str_of(&node, "refreshToken")
        } else {
            v
        }
    };
    if !refresh.is_empty() {
        credentials.insert("oauth:bigmodel:refresh_token".into(), json!(refresh));
    }
    let credentials = Value::Object(credentials);

    // 身份、昵称、到期都交给 provider 里那套逻辑，避免两处口径漂移
    let (account_id, nickname, expires_at, _provider, email) =
        zcode_provider::identity(&credentials);

    // 客户端还需要 config.json 里的 apiKey 才真能用：尽力换一个，换不到也照样入库
    let api_key = resolve_biz_api_key(BIGMODEL_BIZ, &access, false).await;
    let config = assemble_zcode_config(&jwt, api_key.as_deref().unwrap_or(""));

    let mut meta = Map::new();
    meta.insert("credentials".into(), credentials);
    meta.insert("config".into(), config);
    meta.insert("device_mid".into(), json!(mid));
    meta.insert("provider".into(), json!("bigmodel"));
    meta.insert("email".into(), json!(email));
    let credential = Credential {
        token: jwt,
        refresh_token: String::new(),
        expires_at: iso_of(expires_at),
        meta,
    };

    save(
        db,
        AccountIdentity {
            iid: format!("zcode:{account_id}"),
            platform: "zcode".into(),
            platform_label: "ZCode".into(),
            account_id,
            name: nickname.clone(),
            expires_at,
            source: "浏览器登录".into(),
            requires_checkin: false,
            has_secret: true,
        },
        &credential,
        expires_at,
        &nickname,
    )
}

fn zcode_headers(token: &str, mid: &str) -> Vec<(String, String)> {
    vec![
        ("User-Agent".into(), "ZCode/1.0".into()),
        ("HTTP-Referer".into(), "https://zcode.z.ai".into()),
        ("X-Title".into(), "Z Code@electron".into()),
        ("X-Platform".into(), "win32-x64".into()),
        ("X-Release-Channel".into(), "stable".into()),
        ("X-Client-Language".into(), "zh-CN".into()),
        ("X-Client-Timezone".into(), "Asia/Shanghai".into()),
        ("X-Os-Category".into(), "windows".into()),
        ("X-Device-Mid".into(), mid.into()),
        ("Authorization".into(), format!("Bearer {token}")),
        ("Content-Type".into(), "application/json".into()),
    ]
}

fn assemble_zcode_config(jwt: &str, api_key: &str) -> Value {
    let key = api_key.trim();
    let entry = |name: &str, key: &str, base: &str| {
        let options = if key.is_empty() {
            json!({ "apiKey": "", "apiKeyRequired": true, "baseURL": base })
        } else {
            json!({ "apiKey": key, "baseURL": base })
        };
        json!({
            "name": name,
            "kind": "anthropic",
            "options": options,
            "enabled": !key.is_empty(),
            "source": "custom",
        })
    };
    json!({
        "provider": {
            "builtin:bigmodel": entry("Bigmodel - API Key", key, BIGMODEL_ANTHROPIC),
            "builtin:bigmodel-coding-plan": entry("BigModel - Coding Plan", key, BIGMODEL_ANTHROPIC),
            "builtin:bigmodel-start-plan": entry("BigModel- Coding Plan", jwt, START_PLAN_ANTHROPIC),
        }
    })
}

/// 拿 access token 换一个 anthropic 用的 apiKey（`key.secret` 形态）。
/// 换不到就返回 None——账号仍然可用（查额度/签到走 JWT），只是切到客户端要重登。
async fn resolve_biz_api_key(base: &str, access: &str, require_secret: bool) -> Option<String> {
    let http = RealHttp::new();
    let customer = http
        .send({
            let mut req = HttpRequest::get(format!("{base}/api/biz/customer/getCustomerInfo"));
            req.headers = vec![("Authorization".into(), access.into())];
            req
        })
        .await
        .ok()?;
    let customer = customer.json().ok()?;
    let root = customer.get("data").unwrap_or(&customer);
    let orgs = root.get("organizations")?.as_array()?;
    let mut chosen: Option<(&Value, String, Vec<Value>)> = None;
    for org in orgs {
        let Some(org_id) = org.get("organizationId").and_then(|v| v.as_str()) else {
            continue;
        };
        let projects: Vec<Value> = org
            .get("projects")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter(|p| p.get("projectType").and_then(Value::as_str) != Some("2"))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if projects.is_empty() {
            continue;
        }
        chosen = Some((org, org_id.to_string(), projects));
        if org
            .get("organizationName")
            .and_then(Value::as_str)
            .map(|n| n.contains("默认机构"))
            .unwrap_or(false)
        {
            break;
        }
    }
    let (_, org_id, projects) = chosen?;
    let project_id = projects
        .iter()
        .find(|p| {
            p.get("projectName")
                .and_then(Value::as_str)
                .map(|n| n.contains("默认项目"))
                .unwrap_or(false)
        })
        .or_else(|| projects.first())?
        .get("projectId")?
        .as_str()?
        .to_string();

    let keys_url =
        format!("{base}/api/biz/v1/organization/{org_id}/projects/{project_id}/api_keys");
    let listing = http
        .send({
            let mut req = HttpRequest::get(&keys_url);
            req.headers = vec![("Authorization".into(), access.into())];
            req
        })
        .await
        .ok()?
        .json()
        .ok()?;
    let rows = listing
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| listing.as_array().cloned())
        .unwrap_or_default();
    let existing = rows
        .iter()
        .find(|item| item.get("name").and_then(Value::as_str) == Some(ZCODE_KEY_NAME));
    let created;
    let found = if let Some(item) = existing {
        item.clone()
    } else {
        let mut req = HttpRequest::post(&keys_url);
        req.headers = vec![("Authorization".into(), access.into())];
        req.body = Some(json!({ "name": ZCODE_KEY_NAME }).to_string());
        created = http.send(req).await.ok()?.json().ok()?;
        created
    };
    let key = found
        .get("apiKey")
        .and_then(Value::as_str)?
        .trim()
        .to_string();
    if key.is_empty() {
        return None;
    }
    let detail = http
        .send({
            let mut req = HttpRequest::get(format!("{keys_url}/copy/{}", urlencode(&key)));
            req.headers = vec![("Authorization".into(), access.into())];
            req
        })
        .await
        .ok()?
        .json()
        .ok()?;
    let secret = detail
        .get("secretKey")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if secret.is_empty() {
        return if require_secret { None } else { Some(key) };
    }
    Some(format!("{key}.{secret}"))
}

// ------------------------------------------------------------------ 公共

fn save(
    db: &Db,
    identity: AccountIdentity,
    credential: &Credential,
    expires_at: Option<i64>,
    nickname: &str,
) -> Result<LoginOutcome> {
    let iid = identity.iid.clone();
    repo::upsert_identity(db, &identity, now_ms())?;
    repo::set_secret(db, &iid, &credential.to_bytes()?, expires_at)?;
    Ok(LoginOutcome {
        ok: true,
        message: format!("已添加 {nickname}"),
        iid,
    })
}

fn str_of(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn expiry_ms(data: &Value) -> Option<i64> {
    if let Some(ms) = data.get("expiresAt").and_then(Value::as_i64) {
        if ms > 0 {
            return Some(ms);
        }
    }
    if let Some(sec) = data.get("expiresIn").and_then(Value::as_i64) {
        if sec > 0 {
            return Some(now_ms() + sec * 1000);
        }
    }
    None
}

fn device_expiry(data: &Value) -> Option<i64> {
    if let Some(ms) = data.get("expires_at").and_then(Value::as_i64) {
        if ms > 1_000_000_000_000 {
            return Some(ms);
        }
        if ms > 1_000_000_000 {
            return Some(ms * 1000);
        }
    }
    data.get("expires_in")
        .and_then(Value::as_i64)
        .map(|sec| now_ms() + sec * 1000)
}

fn iso_of(ms: Option<i64>) -> String {
    ms.and_then(|v| Local.timestamp_millis_opt(v).single())
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

fn random_bytes(len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    if getrandom::getrandom(&mut buf).is_err() {
        // 极端情况下退回时间戳，至少不 panic
        let seed = now_ms().to_le_bytes();
        for (i, byte) in buf.iter_mut().enumerate() {
            *byte = seed[i % seed.len()];
        }
    }
    buf
}

fn random_hex(len: usize) -> String {
    random_bytes(len)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn uuid_like() -> String {
    let h = random_hex(16);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// query 里的值只可能是 UUID / base64url，做一次最小转义即可
fn urlencode(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            other => format!("%{:02X}", other as u32 & 0xff),
        })
        .collect()
}

async fn sleep(ms: u64) {
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_entries_follow_key_presence() {
        let with = assemble_zcode_config("jwt", "key.secret");
        let entry = &with["provider"]["builtin:bigmodel"];
        assert_eq!(entry["options"]["apiKey"], "key.secret");
        assert_eq!(entry["enabled"], true);

        let without = assemble_zcode_config("jwt", "");
        let entry = &without["provider"]["builtin:bigmodel"];
        assert_eq!(entry["enabled"], false);
        assert_eq!(entry["options"]["apiKeyRequired"], true);
    }

    #[test]
    fn uuid_like_has_five_groups() {
        let id = uuid_like();
        assert_eq!(id.split('-').count(), 5);
        assert_eq!(id.len(), 36);
    }
}
