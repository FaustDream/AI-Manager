//! 云端优先的凭据校验：**先问平台，再读本机**。
//!
//! 「这个账号还能不能用」不再只靠读客户端文件推断。拿本地保存的凭据直接打一次
//! 平台上最轻的只读接口（各家的 `credits`，也就是余额/配额查询）：
//!
//!   - 能通 → 凭据有效，云端这条路成立；
//!   - 401/403 → 凭据被平台拒绝，只能重新授权（比「本地看着还没过期」可信）；
//!   - 没网 / 平台没有可查接口 → **回退本机读取**，结论里写明是兜底。
//!
//! **不做 token 续期**：各家的 refresh 协议都没有公开文档，自己拼一个请求等于把
//! 账号交给猜出来的端点。云端能回答的只有「有效 / 失效」；有效期本身仍然来自凭据
//! （Qoder 那一份取 refresh token 的到期时间，见 `providers/qoder.rs`）。
//!
//! 与 `refresh.rs` / `credential_refresh.rs` 的分工：本模块只回答「云端怎么说」，
//! 写库与用户话术留在调用方，避免两个模块对同一个账号各写一次。

use super::credential::Credential;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::{Http, RealHttp};
use crate::app::providers::{self, Provider};
use crate::types::CreditSnapshot;

/// 云端对「这份凭据还能不能用」的回答。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudVerdict {
    /// 平台认这份凭据
    Valid,
    /// 平台明确拒绝（401/403）：凭据废了，只能重新授权
    Expired { reason: String },
    /// 云端这条路走不通（没网、平台没有可查接口）：回退本机读取
    Unavailable { reason: String },
}

/// 用保存的凭据问一次云端。会发网络请求，调用方负责放到阻塞线程里。
pub fn probe(iid: &str, cred: &Credential) -> CloudVerdict {
    probe_with(iid, cred, &RealHttp::new())
}

/// 同 `probe`，但允许注入 HTTP 实现（测试用）。
pub fn probe_with(iid: &str, cred: &Credential, http: &dyn Http) -> CloudVerdict {
    let Some(provider) = providers::for_iid(iid) else {
        return CloudVerdict::Unavailable {
            reason: "这个平台没有云端查询实现".into(),
        };
    };
    verdict_of(run_credits(provider.as_ref(), cred, http))
}

/// 结论映射：独立成纯函数，测试不必真的发请求。
fn verdict_of(result: Result<CreditSnapshot>) -> CloudVerdict {
    match result {
        Ok(_) => CloudVerdict::Valid,
        // 只有 AuthExpired 代表「平台说这凭据不行」；其余错误都是「这次问不出来」
        Err(e) if matches!(e.kind, ErrorKind::AuthExpired) => {
            CloudVerdict::Expired { reason: e.message }
        }
        Err(e) => CloudVerdict::Unavailable { reason: e.message },
    }
}

/// 同步上下文里跑一次只读查询。与 `login.rs` 同一条路子：临时建一个单线程运行时，
/// 不接管调用方已有的 tokio 上下文（刷新走的是阻塞线程池，那里没有运行时）。
fn run_credits(
    provider: &dyn Provider,
    cred: &Credential,
    http: &dyn Http,
) -> Result<CreditSnapshot> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::new(ErrorKind::Io, format!("建云端校验运行时失败：{e}")))?;
    runtime.block_on(provider.credits(cred, http))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> CreditSnapshot {
        CreditSnapshot {
            summary: "剩余 1 积分".into(),
            error: None,
            balance: Some(1.0),
            lots: None,
            quota: None,
            checked_in: None,
        }
    }

    #[test]
    fn accepted_credential_is_valid() {
        assert_eq!(verdict_of(Ok(snapshot())), CloudVerdict::Valid);
    }

    #[test]
    fn auth_expired_is_expired_not_unavailable() {
        let err = AppError::new(ErrorKind::AuthExpired, "token 已失效");
        assert!(matches!(verdict_of(Err(err)), CloudVerdict::Expired { .. }));
    }

    /// 平台没有可查接口 ≠ 凭据失效：必须回退本机，不能误判成要重新授权。
    #[test]
    fn unsupported_provider_falls_back_to_local() {
        let err = AppError::new(ErrorKind::Unsupported, "暂不支持查询积分");
        match verdict_of(Err(err)) {
            CloudVerdict::Unavailable { reason } => assert!(reason.contains("暂不支持")),
            other => panic!("应为 Unavailable，实际 {other:?}"),
        }
    }

    #[test]
    fn unknown_platform_is_unavailable() {
        let cred = Credential::default();
        assert!(matches!(
            probe("unknown:u1", &cred),
            CloudVerdict::Unavailable { .. }
        ));
    }
}
