//! 时间解析与当前时刻。**业务代码不得直接 `SystemTime::now()`**，
//! 否则「明天能不能签」「24 小时后」这类边界无法构造（见 P0 注入点设计）。

use chrono::{Local, TimeZone};
use serde_json::Value;

/// 当前 unix 毫秒。
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 各家接口的时间字段 → unix 毫秒。兼容四种形态：
/// 毫秒数、秒数（早于 2286 年都小于 1e10）、RFC3339、本地朴素时间。
pub fn parse_ts(value: &Value) -> Option<i64> {
    if let Some(n) = value.as_i64() {
        return Some(if n < 10_000_000_000 { n * 1000 } else { n });
    }
    let text = value.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(text) {
        return Some(dt.timestamp_millis());
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(text, fmt) {
            if let Some(dt) = Local.from_local_datetime(&naive).single() {
                return Some(dt.timestamp_millis());
            }
        }
    }
    None
}

/// 从 JWT 的 payload 里读 `exp`（转成 unix 毫秒）。**不验签**。
///
/// 用途只有一个：客户端票据里没写 `expiresAt` 时，用 token 自带的过期时间补一个。
/// 它只影响列表上的「凭据到期」与「还剩几天」提醒，不参与任何鉴权判断——
/// 所以这里不校验签名、不校验 alg，解析不出来就返回 `None`（宁可不显示，也不编）。
pub fn jwt_exp_ms(token: &str) -> Option<i64> {
    use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
    use base64::Engine;
    let payload = token.split('.').nth(1)?;
    if payload.is_empty() {
        return None;
    }
    // JWT 按规范去掉了 padding，但个别客户端会留着，两种都试
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| URL_SAFE.decode(payload))
        .ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    parse_ts(value.get("exp")?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_all_four_shapes() {
        assert_eq!(
            parse_ts(&json!(1_700_000_000_000i64)),
            Some(1_700_000_000_000)
        );
        assert_eq!(
            parse_ts(&json!(1_700_000_000i64)),
            Some(1_700_000_000_000),
            "秒级要乘 1000"
        );
        assert_eq!(
            parse_ts(&json!("2026-10-30T11:06:25")),
            parse_ts(&json!("2026-10-30T11:06:25"))
        );
        assert!(parse_ts(&json!("2026-10-30T03:06:25Z")).is_some());
        assert!(parse_ts(&json!("")).is_none());
        assert!(parse_ts(&json!("不是时间")).is_none());
    }

    /// 票据里没写 expiresAt 时，token 自身的 exp 就是唯一可信来源。
    #[test]
    fn jwt_exp_is_read_without_verifying_the_signature() {
        // {"exp":1799999999} = 2027-01-13 前后（秒级）
        let token = "eyJhbGciOiJub25lIn0.eyJleHAiOjE3OTk5OTk5OTl9.sig";
        assert_eq!(jwt_exp_ms(token), Some(1_799_999_999_000));
        // 不是 JWT / 没有 exp：一律 None，由调用方决定沿用旧值
        assert_eq!(jwt_exp_ms("opaque-token"), None);
        assert_eq!(jwt_exp_ms("a.b.c"), None);
        assert_eq!(jwt_exp_ms(""), None);
    }

    #[test]
    fn rfc3339_keeps_timezone() {
        let utc = parse_ts(&json!("2026-10-30T03:00:00Z")).unwrap();
        let cn = parse_ts(&json!("2026-10-30T11:00:00+08:00")).unwrap();
        assert_eq!(utc, cn, "同一时刻的不同写法必须解析成同一个 ms");
    }
}
