//! 各平台账号扫描与登录态写回。一家一个文件，扫描产物统一是 `ScannedAccount`。
//!
//! 共同约定（三条都是真金白银换来的）：
//!   - 找不到客户端目录 = 返回 `Ok(0)`，**不是错误**（没装就该没账号）
//!   - `source` 只留文件名，绝不落绝对路径（里面有 Windows 用户名）
//!   - 凭据走回调立刻入库，**不收集成集合**：内存里同时存在的明文 token 永远只有一个
//!
//! P0 曾在 `app/mod.rs` 定过一个 `Provider::scan(&self, ctx) -> Vec<Account>` 的签名，
//! P2 实现时确认它不可用——`Account` 按契约 17 不含 token，返回 `Vec<Account>`
//! 就带不出凭据；改成 `Vec<ScannedAccount>` 又会把全部明文攒在内存里。
//! 所以扫描改成回调式，签名在这里重定；`status` / `checkin` 留给 P3 在同一个模块里定。

use crate::app::accounts::credential::Credential;
use crate::app::accounts::AccountIdentity;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::Http;
use crate::types::{ActionResult, CreditSnapshot, StatusResult};
use async_trait::async_trait;

pub mod codebuddy;
pub mod qoder;
pub mod trae;
pub mod workbuddy;
pub mod zclaim;
pub mod zcode;

pub use qoder::QoderProvider;
pub use trae::TraeProvider;
pub use workbuddy::WorkBuddyProvider;
pub use zcode::ZCodeProvider;

#[derive(Debug)]
/// 切换登录的结果。`backup` 是写前备份的位置，界面要能告诉用户「后悔了去哪找」。
pub struct SwitchOutcome {
    pub message: String,
    pub backup: Option<std::path::PathBuf>,
}

/// 扫描产物：身份信息 + 已序列化的凭据包。
/// **凭据必须在回调里立刻加密入库**，不要收集成集合。
pub struct ScannedAccount {
    pub identity: AccountIdentity,
    pub secret: Vec<u8>,
}

impl ScannedAccount {
    pub fn new(identity: AccountIdentity, credential: &Credential) -> Result<Self> {
        Ok(Self {
            identity,
            secret: credential.to_bytes()?,
        })
    }
}

/// 扫描回调。用 `&mut dyn` 而不是泛型：`scan_all` 要能遍历一个异构 Provider 列表。
pub type Sink<'a> = dyn FnMut(ScannedAccount) -> Result<()> + 'a;

#[async_trait]
pub trait Provider: Send + Sync {
    /// 平台键，与 `accounts.platform` 一致
    fn platform(&self) -> &'static str;
    fn label(&self) -> &'static str;
    /// 扫描并逐个交出账号，返回条数。缺客户端返回 0。
    fn scan(&self, sink: &mut Sink<'_>) -> Result<usize>;
    /// 客户端**当前**登录的 account_id；读不到返回 None（客户端没装/没登录）。
    fn current_id(&self) -> Option<String>;

    // ---- 以下是 P3 加的接口：默认实现一律「不支持」，各家按需覆盖 ----
    /// 平台本身有没有签到玩法。WorkBuddy 国际版与 ZCode 都没有，只查积分。
    fn requires_checkin(&self) -> bool {
        true
    }
    /// 平台自己会返回「发放批次」（Trae / WorkBuddy）。这类平台**不写台账**（契约 5）。
    fn live_credit(&self) -> bool {
        false
    }
    /// 契约 4 的冷却口径：`0` = 自然日（次日 0 点可签），`24` = 上次签到 + 24h（Qoder）。
    /// 活动制平台的下次可签时刻由接口下发，这个值只做接口没给时的本地推算兜底。
    fn cooldown_hours(&self) -> u32 {
        0
    }
    /// 查签到状态。会顺带读余额。
    async fn status(&self, _cred: &Credential, _http: &dyn Http) -> Result<StatusResult> {
        Err(AppError::new(
            ErrorKind::Unsupported,
            format!("{} 暂不支持查询签到状态", self.label()),
        ))
    }
    /// 只查积分，**不碰签到状态**（高频刷新走这条，避免无意义地打签到接口）。
    async fn credits(&self, _cred: &Credential, _http: &dyn Http) -> Result<CreditSnapshot> {
        Err(AppError::new(
            ErrorKind::Unsupported,
            format!("{} 暂不支持查询积分", self.label()),
        ))
    }
    /// 真正调用签到接口。调用方负责幂等（当日去重表）。
    async fn checkin(&self, _cred: &Credential, _http: &dyn Http) -> Result<ActionResult> {
        Err(AppError::new(
            ErrorKind::Unsupported,
            format!("{} 暂不支持签到", self.label()),
        ))
    }
}

/// 平台登记表（架构评审 C4）：**客户端平台键** → Provider 构造，全库唯一分支点。
/// `all` / `for_iid` / `for_platform` 共用；新增平台只改这一处，三边不会再漂移。
fn make(client_key: &str) -> Option<Box<dyn Provider>> {
    match client_key {
        "qoder" => Some(Box::new(QoderProvider::new(false))),
        "qoder_intl" => Some(Box::new(QoderProvider::new(true))),
        "workbuddy" => Some(Box::new(WorkBuddyProvider::new(false))),
        "workbuddy_intl" => Some(Box::new(WorkBuddyProvider::new(true))),
        "zcode" => Some(Box::new(ZCodeProvider::new())),
        "trae" => Some(Box::new(TraeProvider::new())),
        _ => None,
    }
}

/// 全部 Provider。顺序 = 扫描顺序 = 新账号首次入表时的排序。
pub fn all() -> Vec<Box<dyn Provider>> {
    [
        "qoder",
        "qoder_intl",
        "workbuddy",
        "workbuddy_intl",
        "zcode",
        "trae",
    ]
    .iter()
    .filter_map(|key| make(key))
    .collect()
}

/// 按 iid 的平台前缀找回对应 Provider（只读用途：判断「是不是当前账号」等）。
/// WorkBuddy 两版共用 `workbuddy` 前缀，先经 `client_key_of` 换算再查登记表。
pub fn for_iid(iid: &str) -> Option<Box<dyn Provider>> {
    platform_of(iid)?;
    make(client_key_of(iid))
}

/// 按**客户端平台键**找回 Provider（`accounts::add` 用的是这套键：
/// `workbuddy` / `workbuddy_intl` 分开）。
pub fn for_platform(platform: &str) -> Option<Box<dyn Provider>> {
    make(platform)
}

/// iid 的平台前缀。
pub fn platform_of(iid: &str) -> Option<&str> {
    iid.split(':').next().filter(|s| !s.is_empty())
}

/// iid → **客户端**平台键（`accounts::add` 的 `process_names` / `client_exe` 用同一套键）。
///
/// 和 `platform_of` 的区别只有 WorkBuddy：两个版本共用 `workbuddy` 前缀，靠账号 id 的
/// `@intl` 后缀区分。直接拿前缀去关/启客户端会落到国内版——关错版本、启动错版本。
pub fn client_key_of(iid: &str) -> &str {
    match platform_of(iid) {
        Some("workbuddy") if iid.ends_with("@intl") => "workbuddy_intl",
        Some(key) => key,
        None => "",
    }
}

/// **切换登录的统一入口**：按平台分发到各自的实现。
/// 一家一个函数，不做巨型 match——每家的写入面差异太大（信封 / 档案包 / 整份 JSON）。
pub fn switch(
    iid: &str,
    account_id: &str,
    name: &str,
    cred: &crate::app::accounts::credential::Credential,
    backup_dir: &std::path::Path,
) -> Result<SwitchOutcome> {
    match platform_of(iid) {
        Some("qoder") => qoder::switch_to(
            &QoderProvider::new(false),
            account_id,
            name,
            cred,
            backup_dir,
        ),
        Some("qoder_intl") => qoder::switch_to(
            &QoderProvider::new(true),
            account_id,
            name,
            cred,
            backup_dir,
        ),
        Some("workbuddy") => workbuddy::switch_to(
            &WorkBuddyProvider::new(iid.ends_with("@intl")),
            account_id,
            name,
            backup_dir,
        ),
        Some("zcode") => {
            zcode::switch_to(&ZCodeProvider::new(), account_id, name, cred, backup_dir)
        }
        Some("trae") => Err(crate::app::error::AppError::new(
            ErrorKind::Unsupported,
            "Trae 的登录态是设备绑定的加密格式，无法安全写回。请在 Trae 客户端里手动切换账号。",
        )),
        _ => Err(crate::app::error::AppError::new(
            ErrorKind::Unsupported,
            "这个平台不支持切换登录",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iid_routes_to_the_right_variant() {
        assert_eq!(for_iid("qoder_intl:abc").unwrap().platform(), "qoder_intl");
        assert_eq!(
            for_iid("workbuddy:abc@intl").unwrap().label(),
            "WorkBuddy 国际版"
        );
        assert_eq!(
            for_iid("workbuddy:abc").unwrap().label(),
            "WorkBuddy 国内版"
        );
        assert!(for_iid("unknown:abc").is_none());
    }

    #[test]
    fn client_key_keeps_the_edition_apart() {
        // 关/启客户端必须按版本走：只拿平台前缀会把国际版操作到国内版上
        assert_eq!(client_key_of("workbuddy:abc@intl"), "workbuddy_intl");
        assert_eq!(client_key_of("workbuddy:abc"), "workbuddy");
        assert_eq!(client_key_of("qoder_intl:abc"), "qoder_intl");
        assert_eq!(client_key_of(""), "");
    }

    #[test]
    fn trae_refuses_to_switch() {
        let cred = crate::app::accounts::credential::Credential::default();
        let err = switch("trae:u1", "u1", "n", &cred, std::path::Path::new(".")).unwrap_err();
        assert!(matches!(
            err.kind,
            crate::app::error::ErrorKind::Unsupported
        ));
    }
}
