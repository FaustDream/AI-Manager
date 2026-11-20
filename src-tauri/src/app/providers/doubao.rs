//! 豆包（doubao.com）：**只读积分查询**，不做签到、不领取额度。
//!
//! 四条前提决定了这个适配器的形状，别照着其它平台的样子改：
//!
//!  1. **不做签到/领取**。领取接口（`campaign/landing/checkin`）实测会被
//!     风控，代价远大于每天 200 额度；用户要签到请去豆包客户端里点。所以本模块只保留
//!     只读的面板查询，`Provider::checkin` 保持默认「不支持」。
//!  2. **没有本地登录态可读**。豆包电脑版是 Electron，登录态在 Chromium 的加密
//!     cookie 库里（DPAPI + v10/v20 信封），格式随客户端版本变；网页版走字节 SSO，
//!     没有可回调的授权码。所以账号只能由用户**粘贴 Cookie** 添加
//!     （`accounts::cookie`），`scan` 永远返回 0，`current_id` 永远 None。
//!  3. **鉴权就是 doubao.com 的 Cookie**：没有 a_bogus / msToken 签名，
//!     `device_id` 留空也能过（见 techysy/CreditDaddy#32）。
//!  4. **平台不下发额度到期日**。官方口径是「赠送额度自发放起 30 天内有效」，但
//!     面板只回累计额度与今日发放额，没有批次也没有到期时间。这里**不推算**：
//!     推算出来的日期会被当成平台数据，比空着更容易误导。
//!
//! 唯一在用的接口（口径与 dsh-checkin-all 的 `lib/providers/doubao.js`（MIT）一致）：
//!   POST /alice/ug/campaign/v3/detail/        活动面板（累计额度 + 今日发放）
//!
//! 工作版（豆包工作）与飞书企业账号的面板里没有 `daily_signin` 任务，额度口径也可能
//! 不同。缺任务不算错误——额度照读，只是没有「今日发放」这一项。

use crate::app::accounts::credential::Credential;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::{Http, HttpRequest};
use crate::app::providers::{Provider, Sink};
use crate::types::{trim_f64, CreditSnapshot};
use async_trait::async_trait;
use serde_json::{json, Value};

const DETAIL_URL: &str = "https://www.doubao.com/alice/ug/campaign/v3/detail/";
/// 活动组：面板查询要带，否则服务端不知道问的是哪个活动
const CAMPAIGN_GROUP: &str = "pc_incentive_task_mvp_v3";
/// Cookie 失效（真的登录过期）
const LOGIN_INVALID: i64 = 710012001;
/// Cookie 有效，但这个账号没有可用的额度活动面板（工作版 / 飞书企业账号）
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

    /// 活动面板接口：一次查询返回整块任务、奖励与示例内容（30KB+），
    /// 和别家「一个轻量余额 GET」不是一回事。5 分钟一轮（288 次/天）太像脚本，
    /// 定时轮询压到 30 分钟；手动「刷新积分」不受这个限制。
    fn credits_min_interval_minutes(&self) -> u64 {
        30
    }

    fn credits_interval_note(&self) -> &'static str {
        "活动面板接口：一次查询返回整块活动数据，定时刷新最快 30 分钟一次；手动「刷新积分」不受限"
    }

    /// 豆包不走签到：领取接口会被风控（见模块注释）。界面据此不画「签到」按钮，
    /// 定时轮询也不会把豆包排进签到批次。
    fn requires_checkin(&self) -> bool {
        false
    }

    async fn credits(&self, cred: &Credential, http: &dyn Http) -> Result<CreditSnapshot> {
        let data = self.detail(cred, http).await?;
        let panel = parse_panel(&data);
        Ok(CreditSnapshot {
            summary: panel.summary(),
            error: None,
            balance: panel.total,
            // 平台不给批次：到期日空着，界面显示「平台未提供」，不编造
            lots: None,
            quota: None,
        })
    }
}

// ------------------------------------------------------------------ 纯函数

/// 面板里跟「积分」有关的那几个数。
///
/// `total` 是平台口径的累计额度（不是剩余可用），字段名就叫 total_amount；
/// 「今日发放」挂在 `daily_signin` 任务上，工作版 / 飞书企业账号没有这个任务。
#[derive(Debug, Clone, PartialEq)]
struct Panel {
    /// 累计已得额度（平台口径，不是「剩余」）
    total: Option<f64>,
    /// 今天平台发放的额度；任务缺失时为 None
    today: Option<f64>,
}

impl Panel {
    fn summary(&self) -> String {
        let mut text = String::new();
        if let Some(total) = self.total {
            text.push_str(&format!("累计体验额度 {}", trim_f64(total)));
        }
        if let Some(today) = self.today.filter(|v| *v > 0.0) {
            if !text.is_empty() {
                text.push_str(" · ");
            }
            text.push_str(&format!("今日 +{}", trim_f64(today)));
        }
        if text.is_empty() {
            text.push_str("豆包暂无额度数据");
        }
        text
    }
}

/// 从 `detail` 的 `data` 里取额度。
///
/// 不要求 `daily_signin` 任务存在：工作版 / 飞书企业账号没有签到任务，但额度面板
/// 一样有值；把「缺任务」当错误会让这些账号连余额都查不到。
fn parse_panel(data: &Value) -> Panel {
    let signin = data
        .get("tasks")
        .and_then(Value::as_array)
        .and_then(|list| {
            list.iter()
                .find(|task| task.get("task_kind").and_then(Value::as_str) == Some("daily_signin"))
        })
        .and_then(|task| task.get("daily_checkin"));
    Panel {
        total: data.pointer("/progress/total_amount").and_then(as_f64),
        today: signin
            .and_then(|checkin| checkin.pointer("/reward/reward_amount"))
            .and_then(as_f64),
    }
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
            "这个豆包账号没有可用的额度活动面板（工作版 / 飞书企业账号不参与该活动）",
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
    fn panel_reads_total_and_today_grant() {
        let panel = parse_panel(&panel_json());
        assert_eq!(panel.total, Some(1400.0));
        assert_eq!(panel.today, Some(200.0));
        assert!(panel.summary().contains("累计体验额度 1400"));
        assert!(panel.summary().contains("今日 +200"));
    }

    #[test]
    fn panel_without_signin_task_still_reads_total() {
        // 工作版 / 飞书企业账号没有 daily_signin：额度照读，只是没有「今日发放」
        let data = json!({
            "tasks": [{ "task_kind": "chat_activity" }],
            "progress": { "total_amount": 900 }
        });
        let panel = parse_panel(&data);
        assert_eq!(panel.total, Some(900.0));
        assert_eq!(panel.today, None);
        assert!(panel.summary().contains("累计体验额度 900"));
        assert!(!panel.summary().contains("今日"), "{}", panel.summary());
    }

    #[test]
    fn panel_without_progress_says_no_data() {
        let panel = parse_panel(&json!({ "tasks": [] }));
        assert_eq!(panel.total, None);
        assert_eq!(panel.summary(), "豆包暂无额度数据");
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

    #[tokio::test]
    async fn doubao_never_checks_in() {
        // 领取接口会被风控：豆包必须对签到说「不支持」，界面与定时轮询据此排除它
        let provider = DoubaoProvider::new();
        assert!(!provider.requires_checkin());
        let http = MockHttp::new();
        let err = provider.checkin(&cred(), &http).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::Unsupported);
        assert!(http.calls().is_empty(), "签到被禁用后不该发任何请求");
    }

    #[tokio::test]
    async fn credits_reads_total_and_sends_cookie() {
        let http = MockHttp::new();
        http.on(
            &detail_url(),
            200,
            &json!({ "code": 0, "data": panel_json() }).to_string(),
        );
        let snapshot = DoubaoProvider::new().credits(&cred(), &http).await.unwrap();
        assert_eq!(snapshot.balance, Some(1400.0));
        assert!(snapshot.lots.is_none(), "平台不给批次，别编造到期日");
        assert!(
            snapshot.summary.contains("累计体验额度 1400"),
            "{}",
            snapshot.summary
        );
        // Cookie 必须原样带上，否则服务端只会回「登录已过期」
        let cookie = http
            .last_header(&detail_url(), "Cookie")
            .expect("没带 Cookie 头");
        assert_eq!(cookie, "sessionid=test; uid=1");
    }

    #[tokio::test]
    async fn credits_still_work_for_accounts_without_signin_task() {
        // 工作版账号没有「签到任务」就报错的话，连余额都查不到
        let http = MockHttp::new();
        http.on(
            &detail_url(),
            200,
            &json!({ "code": 0, "data": {
                "tasks": [{ "task_kind": "chat_activity" }],
                "progress": { "total_amount": 900 }
            }})
            .to_string(),
        );
        let snapshot = DoubaoProvider::new().credits(&cred(), &http).await.unwrap();
        assert_eq!(snapshot.balance, Some(900.0));
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
            .credits(&cred(), &http)
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::AuthExpired);
        assert!(err.message.contains("Cookie"), "{}", err.message);
    }

    #[tokio::test]
    async fn missing_cookie_fails_without_touching_network() {
        let http = MockHttp::new();
        let err = DoubaoProvider::new()
            .credits(&Credential::default(), &http)
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::AuthExpired);
        assert!(http.calls().is_empty(), "没有 Cookie 时不该发请求");
    }
}
