//! 登录态的**唯一计算点**。
//!
//! 各平台的凭据形态由 `Provider::credential_shape` 自己声明（档案包 / 粘贴 Cookie /
//! 设备绑定只读 / 短期令牌），这里把「形态 + 本机有没有凭据 + 到期 + 健康 + 是否当前登录」
//! 合成一个结论。界面只渲染 `AccountLoginState`，不再自己拼日期与猜测。
//!
//! 两条边界：
//!   - 「当前登录」只认 `switch::current_iids()`（即 `Provider::current_id()`），不靠日期推断；
//!   - 「可切换」要形态支持 **且** 通过 `switch::switchable_iids` 的预检，能力表仍是权威。

use crate::app::accounts::view::AccountView;
use crate::app::db::Db;
use crate::app::error::Result;
use crate::app::providers;
use crate::app::runtime::blocking;
use crate::types::{AccountLoginState, CredentialShape, LoginStateKind};
use chrono::TimeZone;
use std::collections::HashSet;
use std::sync::Arc;

/// 环境性失败：网络不通 / 平台限流。跟凭据没关系，不参与「这个号坏了」的判定。
/// 与前端 `columns.ts` 的 TRANSIENT_FAIL 同口径。
const TRANSIENT_FAIL: [&str; 2] = ["io", "rate_limited"];

/// 3 天内到期开始提示。
const SOON_MS: i64 = 3 * 24 * 60 * 60 * 1000;

fn day(ms: i64) -> String {
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// 纯函数：形态 + 事实 → 结论。不做 I/O，便于逐平台事实上单测。
pub fn compute(
    shape: &CredentialShape,
    has_secret: bool,
    expires_at: Option<i64>,
    last_fail_kind: Option<&str>,
    current: bool,
    switchable: bool,
    now_ms: i64,
) -> AccountLoginState {
    // 没凭据就没有可写回的东西，形态支持也切不了
    let can_switch = has_secret && shape.switchable && switchable;
    let state = |kind: LoginStateKind, label: String, detail: String| AccountLoginState {
        kind,
        label,
        detail,
        shape: shape.clone(),
        current,
        switchable: can_switch,
        expires_at,
    };

    if !has_secret {
        return state(
            LoginStateKind::NotSaved,
            "未保存凭据".into(),
            format!(
                "本机账号库里没有这个账号的{}：不能一键切换，也不参与自动签到。{}",
                shape.label, shape.note
            ),
        );
    }

    // 凭据类失败（登录失效等）优先于日期：日期还没到但平台已经拒绝，说日期是骗人
    if let Some(kind) = last_fail_kind.filter(|k| !TRANSIENT_FAIL.contains(k)) {
        return state(
            LoginStateKind::Failed,
            "登录可能失效".into(),
            format!(
                "最近一次查询失败（{kind}），凭据可能已失效；到期时间仅供参考。{}",
                shape.note
            ),
        );
    }

    if let Some(at) = expires_at {
        let when = day(at);
        if at <= now_ms {
            return state(
                LoginStateKind::Expired,
                format!("{when} 已过期"),
                format!(
                    "登录凭据已于 {when} 过期，需要在客户端重新登录一次，下次启动自动扫描会补存。{}",
                    shape.note
                ),
            );
        }
        if at - now_ms <= SOON_MS {
            return state(
                LoginStateKind::ExpiringSoon,
                format!("{when} 到期"),
                format!("登录凭据 3 天内到期（{when}），建议尽快在客户端续一次。{}", shape.note),
            );
        }
        if current {
            return state(
                LoginStateKind::Current,
                "当前登录".into(),
                format!("这就是客户端现在登录的账号，凭据有效至 {when}。{}", shape.note),
            );
        }
        return state(
            LoginStateKind::Saved,
            if can_switch { "可切换" } else { "凭据已保存" }.into(),
            format!(
                "凭据有效至 {when}（不是积分到期）。{}",
                if can_switch {
                    "可一键切换到客户端。"
                } else {
                    "本工具不写回这个平台的登录态。"
                }
            ),
        );
    }

    // 平台不下发到期时间：说「未知」，不说「没登录」
    if current {
        return state(
            LoginStateKind::Current,
            "当前登录".into(),
            format!("这就是客户端现在登录的账号。{}", shape.note),
        );
    }
    state(
        LoginStateKind::UnknownExpiry,
        if shape.expiry_known {
            "有效期未知"
        } else if can_switch {
            "可切换"
        } else {
            "凭据已保存"
        }
        .into(),
        format!(
            "凭据已加密存在本机，但平台不提供到期时间。{}",
            shape.note
        ),
    )
}

/// 给一批账号视图补上登录态。一次性取当前登录与可切换集合，不逐行读客户端文件。
pub async fn decorate(db: Arc<Db>, rows: &mut [AccountView]) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let (current, switchable) = blocking(move || {
        let current: HashSet<String> = super::switch::current_iids().into_iter().collect();
        let switchable: HashSet<String> = super::switch::switchable_iids(&db)?.into_iter().collect();
        Ok::<_, crate::app::error::AppError>((current, switchable))
    })
    .await??;
    let now = chrono::Local::now().timestamp_millis();
    for row in rows.iter_mut() {
        let Some(provider) = providers::for_iid(&row.iid) else {
            // 平台键不在登记表里（历史遗留行）：留 None，界面回落旧日期渲染
            continue;
        };
        row.login = Some(compute(
            &provider.credential_shape(),
            row.has_secret,
            row.expires_at,
            row.last_fail_kind.as_deref(),
            current.contains(&row.iid),
            switchable.contains(&row.iid),
            now,
        ));
    }
    Ok(())
}

/// 单条账号的登录态。实时事件推送一行时用它，口径与整表一致。
pub async fn decorate_one(db: Arc<Db>, row: &mut AccountView) -> Result<()> {
    decorate(db, std::slice::from_mut(row)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CredentialKind;

    const NOW: i64 = 1_800_000_000_000;
    const DAY: i64 = 24 * 60 * 60 * 1000;

    fn shape(kind: CredentialKind, switchable: bool, expiry_known: bool) -> CredentialShape {
        CredentialShape {
            kind,
            label: "测试形态".into(),
            multi_account: false,
            switchable,
            expiry_known,
            note: "口径说明".into(),
        }
    }

    #[test]
    fn missing_secret_wins_over_everything() {
        let s = shape(CredentialKind::ProfilePack, true, true);
        let got = compute(&s, false, Some(NOW + 30 * DAY), Some("auth_expired"), true, true, NOW);
        assert_eq!(got.kind, LoginStateKind::NotSaved);
        assert!(!got.switchable, "没凭据不可能切换");
    }

    #[test]
    fn credential_failure_outranks_a_future_expiry() {
        let s = shape(CredentialKind::ClientToken, false, true);
        let got = compute(&s, true, Some(NOW + 30 * DAY), Some("auth_expired"), false, false, NOW);
        assert_eq!(got.kind, LoginStateKind::Failed);
    }

    #[test]
    fn transient_failure_does_not_condemn_the_credential() {
        let s = shape(CredentialKind::ClientToken, false, true);
        for kind in ["io", "rate_limited"] {
            let got = compute(&s, true, Some(NOW + 30 * DAY), Some(kind), false, false, NOW);
            assert_eq!(got.kind, LoginStateKind::Saved, "{kind} 与凭据无关");
        }
    }

    #[test]
    fn expired_and_expiring_soon_are_distinct() {
        let s = shape(CredentialKind::ProfilePack, true, true);
        assert_eq!(
            compute(&s, true, Some(NOW - DAY), None, false, true, NOW).kind,
            LoginStateKind::Expired
        );
        assert_eq!(
            compute(&s, true, Some(NOW + DAY), None, false, true, NOW).kind,
            LoginStateKind::ExpiringSoon
        );
    }

    #[test]
    fn current_login_is_reported_even_with_long_expiry() {
        let s = shape(CredentialKind::ProfilePack, true, true);
        let got = compute(&s, true, Some(NOW + 30 * DAY), None, true, true, NOW);
        assert_eq!(got.kind, LoginStateKind::Current);
        assert!(got.current);
        assert!(got.switchable);
    }

    #[test]
    fn zcode_without_exp_says_unknown_not_missing() {
        let s = shape(CredentialKind::ClientToken, true, false);
        let got = compute(&s, true, None, None, false, true, NOW);
        assert_eq!(got.kind, LoginStateKind::UnknownExpiry);
        assert!(got.switchable, "ZCode 可切换");
        assert!(got.label.contains("可切换"));
    }

    #[test]
    fn pasted_cookie_is_saved_but_never_switchable() {
        let s = shape(CredentialKind::PastedCookie, false, false);
        // 即使预检误判为可切换，形态不支持也不能显示可切换
        let got = compute(&s, true, None, None, false, true, NOW);
        assert_eq!(got.kind, LoginStateKind::UnknownExpiry);
        assert!(!got.switchable);
    }

    #[test]
    fn device_bound_trae_is_read_only() {
        let s = shape(CredentialKind::DeviceBound, false, true);
        let got = compute(&s, true, Some(NOW + 30 * DAY), None, false, true, NOW);
        assert!(!got.switchable, "Trae 写回会登出客户端");
        assert_eq!(got.kind, LoginStateKind::Saved);
    }

    #[test]
    fn ima_current_login_without_expiry_is_still_current() {
        let s = shape(CredentialKind::RefreshCookie, false, false);
        let got = compute(&s, true, None, None, true, false, NOW);
        assert_eq!(got.kind, LoginStateKind::Current);
        assert!(!got.switchable);
    }
}