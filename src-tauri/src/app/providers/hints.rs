//! 平台错误码 → 人话。
//!
//! 各平台返回的数字码在界面上只是「code=11102」，用户不知道下一步该干什么。
//! 这里只做一层查表：**不改判定逻辑**，只在原有错误信息后面补一句人话。
//!
//! 码值来自各平台实际返回（WorkBuddy / CodeBuddy 系共用一套错误码），
//! 认不出的保持原样——猜错比不猜更糟。

/// 认得的码给一句「是什么 + 下一步」；认不出返回 `None`。
pub fn explain(code: i64) -> Option<&'static str> {
    match code {
        10001 => Some("今日已签到，无需重复操作"),
        11102 => Some("该模型暂不可用：账号未开通或无权限，换模型或换账号"),
        12153 | 401 => Some("登录态已失效，需要重新登录"),
        6004 | 429 => Some("平台限流，稍后再试"),
        402 | 14018 => Some("积分不足，先签到或领取活动"),
        _ => None,
    }
}

/// 在原信息后补一句人话。原文里已经包含这句就不重复追加。
pub fn with_hint(message: impl Into<String>, code: i64) -> String {
    let message = message.into();
    match explain(code) {
        Some(hint) if !message.contains(hint) => format!("{message}（{hint}）"),
        _ => message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_codes_get_a_hint() {
        assert_eq!(
            with_hint("失败 code=11102", 11102),
            "失败 code=11102（该模型暂不可用：账号未开通或无权限，换模型或换账号）"
        );
        assert!(explain(12153).unwrap().contains("重新登录"));
    }

    #[test]
    fn unknown_codes_and_duplicate_hints_stay_untouched() {
        assert_eq!(
            with_hint("接口返回 code=99999", 99999),
            "接口返回 code=99999"
        );
        let once = with_hint("今日已签到，无需重复操作", 10001);
        assert_eq!(once, "今日已签到，无需重复操作", "已有这句就不再追加");
    }
}
