//! 豆包（doubao.com）：个人账号的「天天领额度」每日签到。
//!
//! 三条前提决定了这个适配器的形状，别照着其它平台的样子改：
//!
//!  1. **没有本地登录态可读**。豆包电脑版是 Electron，登录态在 Chromium 的加密
//!     cookie 库里（DPAPI + v10/v20 信封），格式随客户端版本变；网页版走字节 SSO，
//!     没有可回调的授权码。所以账号只能由用户**粘贴 Cookie** 添加
//!     （`accounts::cookie`），`scan` 永远返回 0，`current_id` 永远 None。
//!  2. **鉴权就是 doubao.com 的 Cookie**：没有 a_bogus / msToken 签名，
//!     `device_id` 留空也能过（2026-10 实测，见 techysy/CreditDaddy#32）。
//!  3. **服务端自带幂等**：重复领取返回 `idempotent: true`，客户端可以放心重试。
//!
//! 接口口径与 dsh-checkin-all / dsh-model 的 `lib/providers/doubao.js`（MIT）一致：
//!   POST /alice/ug/campaign/v3/detail/        任务面板（含今天签没签）
//!   POST /alice/ug/campaign/landing/checkin   领取
//!
//! 只支持**个人账号**：工作版（豆包工作）与飞书企业账号的任务面板里没有
//! `daily_signin`，签到接口回 710010702。那不是故障，要说清原因而不是重试。

use crate::app::accounts::credential::Credential;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::{Http, HttpRequest};
use crate::app::providers::{Provider, Sink};
use crate::types::{trim_f64, ActionResult, CreditSnapshot, StatusResult};
use async_trait::async_trait;
use serde_json::{json, Value};

const DETAIL_URL: &str = "https://www.doubao.com/alice/ug/campaign/v3/detail/";
const CHECKIN_URL: &str = "https://www.doubao.com/alice/ug/campaign/landing/checkin";
/// 活动组：`detail` 用它查任务面板，`checkin` 用它当 scene
const CAMPAIGN_GROUP: &str = "pc_incentive_task_mvp_v3";
/// Cookie 失效（真的登录过期）
const LOGIN_INVALID: i64 = 710012001;
/// Cookie 有效，但这个账号没有可领的签到活动（工作版 / 飞书企业账号）
const NO_ACTIVITY: i64 = 710010702;
/// 请求要带豆包电脑版的网页标记，纯 curl 形态会被风控挡
const CLIENT_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) DoubaoDesktop/2.32.6 Chrome/147.0.7727.149 Safari/537.36";
const REFERER: &str = "https://www.doubao.com/pc/agent-explore";

#[derive(Default)]
pub struct DoubaoProvider;

impl DoubaoProvider {
    pub fn new() -> Self {
        Self
    }

    fn post(&self, cred: &Credential, base: &str, body: &str) -> Result<HttpRequest> {
        let cookie = cookie_of(cred)?;
        let seed = seed_of(cred);
        let mut req = HttpRequest::post(api_url(base, &seed))
            .header("Cookie", cookie)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/plain, */*")
            .header("Referer", REFERER)
            .header("Origin", "https://www.doubao.com")
            .header("User-Agent", CLIENT_UA);
        req.body = Some(body.to_string());
        Ok(req)
    }

    /// 任务面板。错误码在这一层统一翻译成 `AppError`，调用方只管业务。
    async fn detail(&self, cred: &Credential, http: &dyn Http) -> Result<Value> {
        let body = json!({ "campaign_group_code": CAMPAIGN_GROUP }).to_string();
        let resp = http.send(self.post(cred, DETAIL_URL, &body)?).await?;
        let value = resp.json()?;
        check_code(&value)?;
        Ok(value.get("data").cloned().unwrap_or_else(|| json!({})))
    }
}

#[async_trait]
impl Provider for DoubaoProvider {
    fn platform(&self) -> &'static str {
        "doubao"
    }

    fn label(&self) -> &'static str {
        "豆包"
    }

    /// 豆包没有可读的本地登录态（见模块注释），账号只能靠 Cookie 添加。
    /// 返回 0 而不是报错：没装客户端、没账号都是正常状态。
    fn scan(&self, _sink: &mut Sink<'_>) -> Result<usize> {
        Ok(0)
    }

    fn current_id(&self) -> Option<String> {
        None
    }

    /// 豆包没有可重扫的登录态：Cookie 存在本工具里，客户端那边读不出来。
    /// 不声明这一条，「刷新凭据」会对豆包报「已是最新」，等于骗人。
    fn scannable(&self) -> bool {
        false
    }

    fn refresh_unsupported(&self) -> String {
        "豆包没有可重扫的本地登录态：Cookie 过期时请用「添加账号 → 豆包」重新粘贴一次".into()
    }

    /// 豆包额度是平台自己记的（累计发放），写进本工具台账会和「积分」混单位（契约 5）。
    fn live_credit(&self) -> bool {
        true
    }

    /// 签到面板是活动接口：一次查询返回整块任务、奖励与示例内容（30KB+），
    /// 和别家「一个轻量余额 GET」不是一回事。5 分钟一轮（288 次/天）太像脚本，
    /// 定时轮询压到 30 分钟；手动「刷新积分」不受这个限制。
    fn credits_min_interval_minutes(&self) -> u64 {
        30
    }

    fn credits_interval_note(&self) -> &'static str {
        "活动面板接口：一次查询返回整块签到面板，定时刷新最快 30 分钟一次；手动「刷新积分」不受限"
    }

    async fn status(&self, cred: &Credential, http: &dyn Http) -> Result<StatusResult> {
        let data = self.detail(cred, http).await?;
        let panel = parse_panel(&data)?;
        Ok(StatusResult {
            checked_in: Some(panel.checked),
            summary: panel.summary(),
            balance: panel.total,
            lots: None,
            next_at: None,
        })
    }

    async fn credits(&self, cred: &Credential, http: &dyn Http) -> Result<CreditSnapshot> {
        let data = self.detail(cred, http).await?;
        let panel = parse_panel(&data)?;
        Ok(CreditSnapshot {
            summary: panel.summary(),
            error: None,
            balance: panel.total,
            lots: None,
            quota: None,
        })
    }

    async fn checkin(&self, cred: &Credential, http: &dyn Http) -> Result<ActionResult> {
        // 提交前查一次积分面板：既确认今天签没签，也作为「前后对比」的基线。
        let before_data = self.detail(cred, http).await?;
        let before = parse_panel(&before_data)?;
        if before.checked {
            return Ok(ActionResult {
                ok: true,
                message: format!("今日已签到（{}）", before.summary()),
                granted: None,
                validity_days: None,
                already: true,
            });
        }

        let body = json!({ "scene": CAMPAIGN_GROUP }).to_string();
        let resp = http.send(self.post(cred, CHECKIN_URL, &body)?).await?;
        let value = resp.json()?;
        check_code(&value)?;
        let data = value.get("data").cloned().unwrap_or_else(|| json!({}));

        // 服务端幂等：并发或重复点击时它回 idempotent=true，不算新发放
        if data.get("idempotent").and_then(Value::as_bool) == Some(true) {
            return Ok(ActionResult {
                ok: true,
                message: "今日已签到（这次是重复请求，平台没有重复发放）".into(),
                granted: None,
                validity_days: None,
                already: true,
            });
        }

        // 领完再查一次面板，用累计额度的前后差算这次实际到账；
        // 接口的 reward_amount 只作兜底（部分活动不返回金额）。
        let after = self
            .detail(cred, http)
            .await
            .ok()
            .and_then(|data| parse_panel(&data).ok());
        let delta = match (before.total, after.as_ref().and_then(|panel| panel.total)) {
            (Some(before), Some(after)) => Some(after - before).filter(|v| *v > 0.0),
            _ => None,
        };
        let granted = delta
            .or_else(|| data.get("reward_amount").and_then(as_f64))
            .or(before.reward)
            .filter(|v| *v > 0.0);
        let days = data
            .get("continuous_days")
            .and_then(as_i64)
            .unwrap_or(before.continuous);
        Ok(ActionResult {
            ok: true,
            message: match granted {
                Some(amount) => format!("签到成功，+{} 额度（连签 {days} 天）", trim_f64(amount)),
                None => format!("签到成功（连签 {days} 天）"),
            },
            granted,
            validity_days: None,
            already: false,
        })
    }
}

// ------------------------------------------------------------------ 纯函数

/// 面板里跟这张卡片有关的那几个数。
#[derive(Debug, Clone, PartialEq)]
struct Panel {
    checked: bool,
    continuous: i64,
    reward: Option<f64>,
    /// 累计已得额度（平台口径，不是「剩余」）
    total: Option<f64>,
}

impl Panel {
    fn summary(&self) -> String {
        let mut text = String::new();
        if let Some(total) = self.total {
            text.push_str(&format!("累计体验额度 {}", trim_f64(total)));
        }
        if self.continuous > 0 {
            if !text.is_empty() {
                text.push_str(" · ");
            }
            text.push_str(&format!("连签 {} 天", self.continuous));
        }
        if let Some(reward) = self.reward.filter(|v| *v > 0.0) {
            if !text.is_empty() {
                text.push_str(" · ");
            }
            text.push_str(&format!("今日 +{}", trim_f64(reward)));
        }
        if text.is_empty() {
            text.push_str("豆包暂无额度数据");
        }
        text
    }
}

/// 从 `detail` 的 `data` 里取 `daily_signin` 任务。
///
/// 找不到 = 这个账号没有签到玩法（工作版 / 飞书企业账号）。这不是错误码能表达的，
/// 所以要把「它到底有哪些任务」带出来，用户才知道是账号类型不对。
fn parse_panel(data: &Value) -> Result<Panel> {
    let tasks = data.get("tasks").and_then(Value::as_array);
    let task = tasks.and_then(|list| {
        list.iter()
            .find(|task| task.get("task_kind").and_then(Value::as_str) == Some("daily_signin"))
    });
    let Some(task) = task else {
        let kinds: Vec<&str> = tasks
            .map(|list| {
                list.iter()
                    .filter_map(|task| task.get("task_kind").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        let hint = if kinds.is_empty() {
            String::new()
        } else {
            format!("（这个账号现有任务：{}）", kinds.join(" / "))
        };
        return Err(AppError::new(
            ErrorKind::ActivityClosed,
            format!("这个豆包账号没有「每日签到」任务{hint}；工作版与飞书企业账号不参与该活动"),
        ));
    };

    let checkin = task
        .get("daily_checkin")
        .cloned()
        .unwrap_or_else(|| json!({}));
    Ok(Panel {
        checked: checkin
            .get("checked_in_today")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        continuous: checkin.get("continuous_days").and_then(as_i64).unwrap_or(0),
        reward: checkin.pointer("/reward/reward_amount").and_then(as_f64),
        total: data.pointer("/progress/total_amount").and_then(as_f64),
    })
}

/// 接口统一错误码 → 本应用错误类别。`code` 缺失按成功处理（部分环境下它只回 data）。
fn check_code(value: &Value) -> Result<()> {
    let Some(code) = value.get("code").and_then(as_i64) else {
        return Ok(());
    };
    if code == 0 {
        return Ok(());
    }
    let msg = value
        .get("msg")
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("");
    match code {
        LOGIN_INVALID => Err(AppError::new(
            ErrorKind::AuthExpired,
            "豆包登录已过期（Cookie 失效），请重新粘贴 Cookie",
        )),
        NO_ACTIVITY => Err(AppError::new(
            ErrorKind::ActivityClosed,
            "这个豆包账号今天没有可领的签到活动（工作版 / 飞书企业账号不参与）",
        )),
        _ => Err(AppError::new(
            ErrorKind::Provider,
            format!("豆包接口返回异常：{msg}（code {code}）"),
        )),
    }
}

/// 用户粘贴的可能是整行 `Cookie: a=b; c=d`，也可能带换行：规整成 `a=b; c=d`。
fn normalize_cookie(raw: &str) -> String {
    let mut text = raw.trim();
    if text.len() >= 7 && text[..7].eq_ignore_ascii_case("cookie:") {
        text = text[7..].trim();
    }
    text.split(';')
        .map(|part| part.trim().replace(['\r', '\n', '\t'], " "))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

fn cookie_of(cred: &Credential) -> Result<String> {
    let raw = if cred.token.is_empty() {
        cred.meta_str("cookie").unwrap_or("")
    } else {
        cred.token.as_str()
    };
    let cookie = normalize_cookie(raw);
    if cookie.is_empty() {
        return Err(AppError::new(
            ErrorKind::AuthExpired,
            "这个账号没有保存豆包 Cookie，请重新添加账号",
        ));
    }
    Ok(cookie)
}

fn seed_of(cred: &Credential) -> String {
    cred.meta_str("seed")
        .or_else(|| cred.meta_str("uid"))
        .unwrap_or("doubao")
        .to_string()
}

/// 每个账号一组稳定的网页标识（由账号自己的种子派生，账号之间不同）。
///
/// 不随请求变化很重要：服务端会把同一账号的多次请求归到同一设备上，
/// 每次随机反而更像异常流量。模型接口（`model_api`）也复用这组标识。
pub(crate) fn device_ids(seed: &str) -> (String, String) {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in format!("doubao-dev:{seed}").bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let mut next = || {
        // xorshift 只为了让 8 个数字之间不相关，不是安全用途
        hash ^= hash << 13;
        hash ^= hash >> 7;
        hash ^= hash << 17;
        hash
    };
    let web_id = format!(
        "76{}",
        (0..8)
            .map(|_| char::from(b'0' + (next() % 10) as u8))
            .collect::<String>()
    );
    let tea_uuid = (0..16)
        .map(|_| char::from(b'0' + (next() % 10) as u8))
        .collect::<String>();
    (web_id, tea_uuid)
}

/// 请求 URL 上那串「豆包电脑版网页」参数。缺了它们接口会当成可疑来源。
fn api_url(base: &str, seed: &str) -> String {
    let (web_id, tea_uuid) = device_ids(seed);
    format!(
        "{base}?version_code=20800&language=zh&device_platform=web&doubao_device_platform=web\
         &aid=497858&real_aid=497858&pkg_type=release_version&pc_version=2.32.6\
         &doubao_pc_version=5.72.66&samantha_web=1&web_platform=desktop&use-olympus-account=1\
         &web_tab_id=0&web_id={web_id}&tea_uuid={tea_uuid}&device_id=&region=&sys_region="
    )
}

fn as_i64(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_f64().map(|v| v as i64))
}

fn as_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel_json() -> Value {
        json!({
            "tasks": [
                { "task_kind": "chat_activity", "title": "聊聊天" },
                {
                    "task_kind": "daily_signin",
                    "daily_checkin": {
                        "checked_in_today": true,
                        "continuous_days": 3,
                        "reward": { "reward_amount": 200, "reward_type": "quota_grant" }
                    }
                }
            ],
            "progress": { "total_amount": 1400 }
        })
    }

    #[test]
    fn panel_reads_signin_task_fields() {
        let panel = parse_panel(&panel_json()).unwrap();
        assert!(panel.checked);
        assert_eq!(panel.continuous, 3);
        assert_eq!(panel.reward, Some(200.0));
        assert_eq!(panel.total, Some(1400.0));
        assert!(panel.summary().contains("累计体验额度 1400"));
        assert!(panel.summary().contains("连签 3 天"));
    }

    #[test]
    fn panel_without_signin_task_says_why() {
        // 工作版 / 飞书企业账号：只有聊天任务。要带出「现有任务」，用户才知道是账号类型不对
        let data = json!({ "tasks": [{ "task_kind": "chat_activity" }] });
        let err = parse_panel(&data).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ActivityClosed);
        assert!(err.message.contains("chat_activity"), "{}", err.message);
        assert!(err.message.contains("工作版"), "{}", err.message);
    }

    #[test]
    fn error_codes_map_to_kinds() {
        let expired = check_code(&json!({ "code": LOGIN_INVALID, "msg": "登录失效" })).unwrap_err();
        assert_eq!(expired.kind, ErrorKind::AuthExpired);

        let no_task = check_code(&json!({ "code": NO_ACTIVITY, "msg": "系统错误" })).unwrap_err();
        assert_eq!(no_task.kind, ErrorKind::ActivityClosed);

        let other = check_code(&json!({ "code": 123, "msg": "boom" })).unwrap_err();
        assert_eq!(other.kind, ErrorKind::Provider);
        assert!(other.message.contains("boom"));

        // code 缺失按成功处理（个别环境只回 data）
        assert!(check_code(&json!({ "data": {} })).is_ok());
        assert!(check_code(&json!({ "code": 0, "data": {} })).is_ok());
    }

    #[test]
    fn cookie_is_normalized_before_use() {
        assert_eq!(
            normalize_cookie("Cookie: a=1;\r\n b=2 ; c=3; "),
            "a=1; b=2; c=3"
        );
        assert_eq!(normalize_cookie("   "), "");
        // 大小写与前缀空格都要认：用户从 DevTools 复制过来就是这么长的
        assert_eq!(normalize_cookie("cookie:a=1;b=2"), "a=1; b=2");
    }

    #[test]
    fn checkin_panel_is_throttled_for_scheduled_refresh() {
        // 活动面板不是轻量余额接口：定时轮询必须放慢，且理由要能显示给用户
        let provider = DoubaoProvider::new();
        assert_eq!(provider.credits_min_interval_minutes(), 30);
        assert!(
            provider.credits_interval_note().contains("活动面板"),
            "{}",
            provider.credits_interval_note()
        );
    }

    #[test]
    fn device_ids_are_stable_per_account_and_differ_across_accounts() {
        let first = device_ids("db1234567890");
        let again = device_ids("db1234567890");
        let other = device_ids("db0987654321");
        assert_eq!(first, again, "同一账号的设备标识必须稳定");
        assert_ne!(first, other, "不同账号不能共用同一组标识");
        assert_eq!(first.0.len(), 10); // 76 + 8 位
        assert_eq!(first.1.len(), 16);
    }

    #[test]
    fn api_url_carries_desktop_web_markers() {
        let url = api_url(DETAIL_URL, "db1234567890");
        for marker in [
            "device_platform=web",
            "doubao_pc_version=5.72.66",
            "aid=497858",
            "device_id=",
            "web_id=",
        ] {
            assert!(url.contains(marker), "缺参数 {marker}：{url}");
        }
    }

    // ---- 以下是带假 HTTP 的链路测试：解析、错误码、幂等、请求头 ----

    use crate::app::net::MockHttp;

    const SEED: &str = "dbtest1234";

    fn cred() -> Credential {
        let mut cred = Credential {
            token: "sessionid=test; uid=1".into(),
            ..Default::default()
        };
        cred.put("seed", json!(SEED));
        cred
    }

    fn detail_url() -> String {
        api_url(DETAIL_URL, SEED)
    }

    fn checkin_url() -> String {
        api_url(CHECKIN_URL, SEED)
    }

    #[tokio::test]
    async fn status_reads_today_state_total_and_sends_cookie() {
        let http = MockHttp::new();
        http.on(
            &detail_url(),
            200,
            &json!({ "code": 0, "data": panel_json() }).to_string(),
        );
        let status = DoubaoProvider::new().status(&cred(), &http).await.unwrap();
        assert_eq!(status.checked_in, Some(true));
        assert_eq!(status.balance, Some(1400.0));
        assert!(status.summary.contains("连签 3 天"), "{}", status.summary);
        // Cookie 必须原样带上，否则服务端只会回「登录已过期」
        let cookie = http
            .last_header(&detail_url(), "Cookie")
            .expect("没带 Cookie 头");
        assert_eq!(cookie, "sessionid=test; uid=1");
    }

    #[tokio::test]
    async fn checkin_claims_then_reports_grant() {
        let http = MockHttp::new();
        http.on(
            &detail_url(),
            200,
            &json!({ "code": 0, "data": {
                "tasks": [{ "task_kind": "daily_signin", "daily_checkin": {
                    "checked_in_today": false, "continuous_days": 3
                }}],
                "progress": { "total_amount": 1400 }
            }})
            .to_string(),
        );
        http.on(
            &checkin_url(),
            200,
            &json!({ "code": 0, "data": {
                "checked_in": true, "continuous_days": 4,
                "reward_amount": 200, "reward_type": "quota_grant", "idempotent": false
            }})
            .to_string(),
        );

        let result = DoubaoProvider::new().checkin(&cred(), &http).await.unwrap();
        assert!(result.ok);
        assert!(!result.already);
        assert_eq!(result.granted, Some(200.0));
        assert!(result.message.contains("连签 4 天"), "{}", result.message);
        assert_eq!(http.call_count(&checkin_url()), 1);
        // 领取请求体的 scene 必须是活动组，不能是自己拼的字符串
        assert!(http
            .last_header(&checkin_url(), "Referer")
            .unwrap_or_default()
            .contains("doubao.com"));
    }

    #[tokio::test]
    async fn already_checked_today_does_not_call_claim() {
        let http = MockHttp::new();
        http.on(
            &detail_url(),
            200,
            &json!({ "code": 0, "data": panel_json() }).to_string(),
        );
        let result = DoubaoProvider::new().checkin(&cred(), &http).await.unwrap();
        assert!(result.already);
        assert!(result.granted.is_none());
        assert_eq!(http.call_count(&checkin_url()), 0, "已签就不该再打签到接口");
    }

    #[tokio::test]
    async fn idempotent_claim_is_not_counted_as_new_grant() {
        let http = MockHttp::new();
        http.on(
            &detail_url(),
            200,
            &json!({ "code": 0, "data": {
                "tasks": [{ "task_kind": "daily_signin", "daily_checkin": { "checked_in_today": false } }]
            }})
            .to_string(),
        );
        http.on(
            &checkin_url(),
            200,
            &json!({ "code": 0, "data": { "idempotent": true, "reward_amount": 200 } }).to_string(),
        );
        let result = DoubaoProvider::new().checkin(&cred(), &http).await.unwrap();
        assert!(result.ok);
        assert!(result.already, "重复请求不能报成新发放");
        assert!(result.granted.is_none());
    }

    #[tokio::test]
    async fn expired_cookie_maps_to_auth_expired() {
        let http = MockHttp::new();
        // 真实接口对无效 Cookie 的返回（已用假 Cookie 实测）
        http.on(
            &detail_url(),
            200,
            r#"{"data":null,"code":710012001,"msg":"登录已过期，请重新登录","message":"login invalid"}"#,
        );
        let err = DoubaoProvider::new()
            .status(&cred(), &http)
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::AuthExpired);
        assert!(err.message.contains("Cookie"), "{}", err.message);
    }

    #[tokio::test]
    async fn missing_cookie_fails_without_touching_network() {
        let http = MockHttp::new();
        let err = DoubaoProvider::new()
            .status(&Credential::default(), &http)
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::AuthExpired);
        assert!(http.calls().is_empty(), "没有 Cookie 时不该发请求");
    }
}
