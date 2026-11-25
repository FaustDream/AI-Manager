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
//! 在用的两个只读接口：
//!   POST /alice/ug/campaign/v3/detail/                     活动面板（累计获得 + 今日发放）
//!   POST /alice/commerce/sale/subscription/quota/summary/  额度汇总（`quota_remaining_amount`）
//!
//! 活动面板只有「累计获得」，不能当余额展示；额度汇总直接返回
//! `quota_remaining_amount`，这才是可用余额。该字段缺失就报错，
//! 不再用累计获得或 total - used 推算。
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
/// 额度汇总：活动面板只给累计获得，这里才有扣减后的可用额度
const QUOTA_SUMMARY_URL: &str =
    "https://www.doubao.com/alice/commerce/sale/subscription/quota/summary/";
const PRODUCT_LINE: &str = "membership";
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
/// 额度汇总网页端从聊天页发起，Referer 跟着页面走，减少风控误判
const CHAT_REFERER: &str = "https://www.doubao.com/chat/";

#[derive(Default)]
pub struct DoubaoProvider;

impl DoubaoProvider {
    pub fn new() -> Self {
        Self
    }

    fn post(&self, cred: &Credential, base: &str, body: &str) -> Result<HttpRequest> {
        let seed = seed_of(cred);
        self.post_url(cred, api_url(base, &seed), body)
    }

    /// 同 `post`，但调用方已经拼好完整 URL（额度汇总的参数与活动面板不同）。
    fn post_url(&self, cred: &Credential, url: String, body: &str) -> Result<HttpRequest> {
        self.post_url_with_referer(cred, url, body, REFERER)
    }

    fn post_url_with_referer(
        &self,
        cred: &Credential,
        url: String,
        body: &str,
        referer: &str,
    ) -> Result<HttpRequest> {
        let cookie = cookie_of(cred)?;
        let mut req = HttpRequest::post(url)
            .header("Cookie", cookie)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/plain, */*")
            .header("Referer", referer)
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

    /// 额度汇总：只读平台直接给的 `quota_remaining_amount`。
    /// 缺失或请求失败都由 `credits` 报错，不再退回累计口径。
    async fn quota_summary(&self, cred: &Credential, http: &dyn Http) -> Result<Value> {
        let body = json!({ "product_line": PRODUCT_LINE }).to_string();
        let seed = seed_of(cred);
        let url = quota_summary_url(QUOTA_SUMMARY_URL, &seed);
        let resp = http
            .send(self.post_url_with_referer(cred, url, &body, CHAT_REFERER)?)
            .await?;
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
        // 余额只认额度汇总的 quota_remaining_amount；拿不到就报错，
        // 不再退回累计获得（累计值不是可用余额，会把用户引到错误数字上）。
        let data = self.quota_summary(cred, http).await?;
        let Some(available) = parse_available_quota(&data) else {
            return Err(AppError::new(
                ErrorKind::Provider,
                "豆包额度汇总没有返回 quota_remaining_amount，无法确定可用额度",
            ));
        };
        // 活动面板只用来补充「累计获得 / 今日发放」，失败不影响余额。
        let summary = match self.detail(cred, http).await.ok() {
            Some(data) => parse_panel(&data).summary_with_available(available),
            None => format!("可用体验额度 {}", trim_f64(available)),
        };
        Ok(CreditSnapshot {
            summary,
            error: None,
            balance: Some(available),
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
    /// 拿到可用额度后，summary 先说可用值；累计值只作为对照，避免和余额混淆。
    fn summary_with_available(&self, available: f64) -> String {
        let mut text = format!("可用体验额度 {}", trim_f64(available));
        if let Some(total) = self.total.filter(|total| *total > available) {
            text.push_str(&format!(" · 累计获得 {}", trim_f64(total)));
        }
        if let Some(today) = self.today.filter(|v| *v > 0.0) {
            text.push_str(&format!(" · 今日 +{}", trim_f64(today)));
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

/// 从额度汇总里取平台直接给的可用额度。
///
/// 只认 `quota_remaining_amount`，不拿累计获得或 total - used 推算：
/// 该字段缺失时返回 None，由调用方报错，避免把总额当余额。
fn parse_available_quota(data: &Value) -> Option<f64> {
    for path in [
        "/quota_remaining_amount",
        "/quota_summary/quota_remaining_amount",
        "/quota_balance/quota_remaining_amount",
        "/incentive_quota_section/quota_remaining_amount",
    ] {
        if let Some(remaining) = data.pointer(path).and_then(as_f64) {
            return Some(remaining.max(0.0));
        }
    }
    // 已知路径都没有时再递归找一次：字段名足够独特，不依赖具体包一层还是两层
    find_number(data, "quota_remaining_amount").map(|remaining| remaining.max(0.0))
}

/// 在任意层级找第一个数字字段。只在已知路径都落空时兜底使用。
fn find_number(value: &Value, key: &str) -> Option<f64> {
    match value {
        Value::Object(map) => {
            if let Some(found) = map.get(key).and_then(as_f64) {
                return Some(found);
            }
            map.values().find_map(|item| find_number(item, key))
        }
        Value::Array(items) => items.iter().find_map(|item| find_number(item, key)),
        _ => None,
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

/// 额度汇总接口的网页端参数要求 19 位数字设备标识（`device_id` 与 `web_id`
/// 不同，`tea_uuid` 与 `web_id` 相同）。活动面板用的是另一组短标识，
/// 这里单独派生，避免改动已经工作的活动面板请求。
fn quota_ids(seed: &str) -> (String, String, String) {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in format!("doubao-quota:{seed}").bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let mut next = || {
        hash ^= hash << 13;
        hash ^= hash >> 7;
        hash ^= hash << 17;
        format!("77{:017}", hash % 100_000_000_000_000_000)
    };
    let device_id = next();
    let web_id = next();
    (device_id, web_id.clone(), web_id)
}

/// 网页端额度汇总要带一个 tab UUID；按账号种子派生，稳定且账号间不同。
fn tab_id(seed: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in format!("doubao-tab:{seed}").bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let hex = format!("{hash:016x}{:016x}", hash.rotate_left(21));
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// 额度汇总的参数与活动面板不同：网页端用 browser，带 os/region，
/// device_id 不能为空。设备标识仍按账号种子派生，不同账号不共用一套。
///
/// 不复制浏览器 URL 里的 msToken/a_bogus：那是某个账号当次会话的签名，
/// 不能跨账号复用；这里只带账号自己的 Cookie 和派生设备标识。
fn quota_summary_url(base: &str, seed: &str) -> String {
    let (device_id, web_id, tea_uuid) = quota_ids(seed);
    let tab = tab_id(seed);
    format!(
        "{base}?version_code=20800&language=zh&device_platform=web&doubao_device_platform=web\
         &aid=497858&os=windows&real_aid=497858&pkg_type=release_version\
         &device_id={device_id}&pc_version=3.47.0&doubao_pc_version=3.47.0\
         &web_id={web_id}&tea_uuid={tea_uuid}&region=CN&sys_region=CN\
         &samantha_web=1&web_platform=browser&use-olympus-account=1&web_tab_id={tab}"
    )
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
    value.as_f64().or_else(|| {
        // 平台会把余额格式化成 "1,450" 这样的千分位字符串，直接 parse 会失败
        let text = value.as_str()?.trim();
        let cleaned: String = text
            .chars()
            .filter(|ch| !ch.is_whitespace() && *ch != ',')
            .collect();
        cleaned.parse().ok()
    })
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
    }

    #[test]
    fn panel_without_progress_has_no_total() {
        let panel = parse_panel(&json!({ "tasks": [] }));
        assert_eq!(panel.total, None);
    }

    fn quota_summary_json() -> Value {
        json!({ "quota_remaining_amount": "600" })
    }

    #[test]
    fn available_quota_reads_remaining_field() {
        assert_eq!(
            parse_available_quota(&json!({ "quota_remaining_amount": 500 })),
            Some(500.0)
        );
        // 字符串数字也要认；平台实际返回的是带千分位的 "1,450"
        assert_eq!(
            parse_available_quota(&json!({ "quota_remaining_amount": "600" })),
            Some(600.0)
        );
        assert_eq!(
            parse_available_quota(&json!({ "quota_remaining_amount": "1,450" })),
            Some(1450.0)
        );
        // 字段被包一层时也要能找到
        assert_eq!(
            parse_available_quota(&json!({
                "quota_summary": { "quota_remaining_amount": 700 }
            })),
            Some(700.0)
        );
        // 未知层级时的递归兜底
        assert_eq!(
            parse_available_quota(&json!({
                "outer": { "inner": { "quota_remaining_amount": 800 } }
            })),
            Some(800.0)
        );
    }

    #[test]
    fn available_quota_clamps_at_zero() {
        assert_eq!(
            parse_available_quota(&json!({ "quota_remaining_amount": -5 })),
            Some(0.0)
        );
    }

    #[test]
    fn available_quota_missing_field_is_none() {
        assert_eq!(parse_available_quota(&json!({})), None);
        assert_eq!(
            parse_available_quota(&json!({ "incentive_quota_section": {} })),
            None
        );
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

    #[test]
    fn quota_summary_url_carries_browser_markers_and_account_ids() {
        let url = quota_summary_url(QUOTA_SUMMARY_URL, "db1234567890");
        for marker in [
            "web_platform=browser",
            "os=windows",
            "region=CN",
            "sys_region=CN",
            "pc_version=3.47.0",
            "device_id=",
            "web_tab_id=",
        ] {
            assert!(url.contains(marker), "缺参数 {marker}：{url}");
        }
        // 设备标识按账号派生：不同账号不能共用同一组参数
        assert_ne!(url, quota_summary_url(QUOTA_SUMMARY_URL, "db0987654321"));
        let (device_id, web_id, tea_uuid) = quota_ids("db1234567890");
        assert_eq!(device_id.len(), 19);
        assert_eq!(web_id.len(), 19);
        assert_eq!(tea_uuid, web_id);
        assert_ne!(device_id, web_id);
        let tab = tab_id("db1234567890");
        assert_eq!(tab.len(), 36);
        assert_eq!(tab.matches('-').count(), 4);
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

    fn summary_url() -> String {
        quota_summary_url(QUOTA_SUMMARY_URL, SEED)
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
    async fn credits_errors_without_remaining_amount() {
        // 汇总返回成功但没有可用额度字段：宁可报错，也不能退回累计获得
        let http = MockHttp::new();
        http.on(
            &summary_url(),
            200,
            &json!({ "code": 0, "data": {} }).to_string(),
        );
        let err = DoubaoProvider::new()
            .credits(&cred(), &http)
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Provider);
        assert!(
            err.message.contains("quota_remaining_amount"),
            "{}",
            err.message
        );
        assert_eq!(http.call_count(&detail_url()), 0, "不应退回活动面板");
    }

    #[tokio::test]
    async fn credits_uses_available_quota_from_summary() {
        let http = MockHttp::new();
        http.on(
            &detail_url(),
            200,
            &json!({ "code": 0, "data": panel_json() }).to_string(),
        );
        http.on(
            &summary_url(),
            200,
            &json!({ "code": 0, "data": quota_summary_json() }).to_string(),
        );
        let snapshot = DoubaoProvider::new().credits(&cred(), &http).await.unwrap();
        // 余额必须取自汇总的 600，累计 1400 只作为摘要里的对照
        assert_eq!(snapshot.balance, Some(600.0));
        assert!(
            snapshot.summary.contains("可用体验额度 600"),
            "{}",
            snapshot.summary
        );
        assert!(
            snapshot.summary.contains("累计获得 1400"),
            "{}",
            snapshot.summary
        );
        assert!(
            snapshot.summary.contains("今日 +200"),
            "{}",
            snapshot.summary
        );
        // 两个只读接口都要带上同一份 Cookie，汇总请求的 Referer 跟聊天页一致
        assert_eq!(
            http.last_header(&summary_url(), "Cookie").as_deref(),
            Some("sessionid=test; uid=1")
        );
        assert_eq!(
            http.last_header(&summary_url(), "Referer").as_deref(),
            Some(CHAT_REFERER)
        );
    }

    #[tokio::test]
    async fn credits_still_work_for_accounts_without_signin_task() {
        // 工作版账号没有「签到任务」：余额仍取自额度汇总，只是没有「今日发放」
        let http = MockHttp::new();
        http.on(
            &summary_url(),
            200,
            &json!({ "code": 0, "data": { "quota_remaining_amount": "900" } }).to_string(),
        );
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
        assert!(snapshot.summary.contains("可用体验额度 900"));
        assert!(!snapshot.summary.contains("今日"), "{}", snapshot.summary);
    }

    #[tokio::test]
    async fn expired_cookie_maps_to_auth_expired() {
        let http = MockHttp::new();
        // 余额主路径是额度汇总，鉴权错误必须在这一层就映射出来
        http.on(
            &summary_url(),
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
