//! 账号视图：**一行一个 `AccountView`**。
//!
//! v1 的 `account_center.py` 有十来个按 iid 并行的字典（`live_lots` / `_balances` /
//! `_quota` / `_expiry` / `_status_checked` …），每个都要单独写「重扫时清理、
//! 刷新时更新、渲染时查找」三处——加一个展示维度就是加一个字典改三处，这才是它
//! 膨胀到 2080 行的机制。
//!
//! 这里合成一个结构体，并靠 `apply_identity` **只覆盖身份字段**，
//! 让契约 21（重扫不得清空已获取的余额与状态）变成结构性保证，而不是纪律。

use serde::Serialize;
use std::collections::BTreeMap;

/// 扫描得到的身份信息。**只含身份，不含任何异步获取的展示数据。**
#[derive(Debug, Clone, Default)]
pub struct AccountIdentity {
    pub iid: String,
    pub platform: String,
    pub platform_label: String,
    pub account_id: String,
    pub name: String,
    pub expires_at: Option<i64>,
    pub source: String,
    pub requires_checkin: bool,
    pub has_secret: bool,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct AccountView {
    // ---- 身份（重扫覆盖）----
    pub iid: String,
    pub platform: String,
    pub platform_label: String,
    /// 原始昵称。**不要直接显示**——用 `display_name()`，有些平台的昵称就是手机号。
    pub name: String,
    pub source: String,
    pub requires_checkin: bool,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub expires_at: Option<i64>,
    pub has_secret: bool,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub sort_order: i64,

    // ---- 状态（异步补齐，重扫不得清空）----
    pub checked_in: Option<bool>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub checked_at: Option<i64>,
    pub balance: Option<f64>,
    pub quota: BTreeMap<String, f64>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub next_at: Option<i64>,
    /// 最近一次查询失败的原因。**有值不代表整行不可用**——余额查不到不该挡住别名。
    pub error: Option<String>,
}

impl AccountView {
    /// 展示用昵称。**某些平台的昵称本身就是完整手机号**（实测 ZCode 有账号如此），
    /// 而 AGENTS §6 只写了「界面显示昵称、不显示 uid」——没人想到昵称可能就是 uid。
    /// 这里只改展示，不动库里的原值：脱敏是显示规则，不是数据规则。
    pub fn display_name(&self) -> String {
        mask_phone(&self.name)
    }

    pub fn new(identity: AccountIdentity, sort_order: i64) -> Self {
        Self {
            iid: identity.iid,
            platform: identity.platform,
            platform_label: identity.platform_label,
            name: identity.name,
            source: identity.source,
            requires_checkin: identity.requires_checkin,
            expires_at: identity.expires_at,
            has_secret: identity.has_secret,
            sort_order,
            checked_in: None,
            checked_at: None,
            balance: None,
            quota: BTreeMap::new(),
            next_at: None,
            error: None,
        }
    }

    /// **契约 21 的全部实现**：只覆盖身份字段。
    /// 免签到的账号（WorkBuddy 国际版）不参与状态查询，缓存一清就永远停在「—」。
    pub fn apply_identity(&mut self, identity: AccountIdentity) {
        self.platform = identity.platform;
        self.platform_label = identity.platform_label;
        self.name = identity.name;
        self.source = identity.source;
        self.requires_checkin = identity.requires_checkin;
        self.expires_at = identity.expires_at;
        self.has_secret = identity.has_secret;
        // 注意：checked_in / balance / quota / next_at / error 一个都不动
    }
}

/// 账号表：一个 `Vec` 取代 v1 的十来个 HashMap。
///
/// 用 `Vec` 而不是 `HashMap` 的理由：账号数 < 100，线性查找比哈希更快，
/// 而且**顺序本身就是展示顺序**（`sort_order`），一举两得。
#[derive(Default)]
pub struct AccountTable {
    rows: Vec<AccountView>,
}

impl AccountTable {
    pub fn merge_scan(&mut self, scanned: Vec<AccountIdentity>) {
        for identity in scanned {
            match self.rows.iter_mut().find(|row| row.iid == identity.iid) {
                Some(row) => row.apply_identity(identity),
                None => {
                    let order = self.rows.len() as i64;
                    self.rows.push(AccountView::new(identity, order));
                }
            }
        }
    }

    /// v1 每次重扫都会 `{k: v for k, v in ... if k in alive}` 地清十来个字典。
    /// 这里一个都不用清：**不在本次扫描结果里的行保留原样**。
    /// 清掉就意味着「客户端暂时没读到」= 账号消失，那是数据丢失，不是刷新。
    pub fn rows(&self) -> &[AccountView] {
        &self.rows
    }

    pub fn into_sorted(mut self) -> Vec<AccountView> {
        self.rows.sort_by(|a, b| {
            a.sort_order
                .cmp(&b.sort_order)
                .then_with(|| a.iid.cmp(&b.iid))
        });
        self.rows
    }

    pub fn find_mut(&mut self, iid: &str) -> Option<&mut AccountView> {
        self.rows.iter_mut().find(|row| row.iid == iid)
    }
}

/// 纯 11 位手机号中间四位打码。其它形态（含邮箱、昵称）原样返回。
pub fn mask_phone(text: &str) -> String {
    let trimmed = text.trim();
    let is_mobile = trimmed.len() == 11
        && trimmed.starts_with('1')
        && trimmed.chars().all(|c| c.is_ascii_digit());
    if is_mobile {
        format!("{}****{}", &trimmed[..3], &trimmed[7..])
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(iid: &str, name: &str) -> AccountIdentity {
        let (platform, account_id) = iid.split_once(':').unwrap();
        AccountIdentity {
            iid: iid.to_string(),
            platform: platform.to_string(),
            platform_label: platform.to_string(),
            account_id: account_id.to_string(),
            name: name.to_string(),
            requires_checkin: true,
            ..Default::default()
        }
    }

    #[test]
    fn rescan_keeps_async_fields() {
        let mut table = AccountTable::default();
        table.merge_scan(vec![identity("workbuddy:a", "旧名")]);
        {
            let row = table.find_mut("workbuddy:a").unwrap();
            row.balance = Some(12.5);
            row.checked_in = Some(true);
            row.quota.insert("daily".into(), 80.0);
        }

        // 第二次扫描：昵称变了（用户在客户端改了名）
        table.merge_scan(vec![identity("workbuddy:a", "新名")]);
        let row = &table.rows()[0];
        assert_eq!(row.name, "新名", "身份字段必须更新");
        assert_eq!(row.balance, Some(12.5), "契约 21：重扫不得清空余额");
        assert_eq!(row.checked_in, Some(true), "契约 21：重扫不得清空签到态");
        assert_eq!(row.quota.get("daily"), Some(&80.0));
    }

    #[test]
    fn scan_adds_new_rows_in_order() {
        let mut table = AccountTable::default();
        table.merge_scan(vec![identity("qoder:1", "A"), identity("trae:2", "B")]);
        let rows = table.into_sorted();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].iid, "qoder:1");
        assert_eq!(rows[1].sort_order, 1);
    }

    #[test]
    fn phone_number_nickname_is_masked() {
        // 实测：ZCode 有个账号的昵称直接就是完整手机号
        assert_eq!(mask_phone("17679130626"), "176****0626");
        assert_eq!(
            mask_phone("1767913062"),
            "1767913062",
            "10 位不是手机号，原样返回"
        );
        assert_eq!(mask_phone("Faust Dream"), "Faust Dream");
        assert_eq!(mask_phone("935411629@qq.com"), "935411629@qq.com");
        assert_eq!(
            mask_phone("159****0626"),
            "159****0626",
            "已脱敏的不要二次处理"
        );
    }
}
