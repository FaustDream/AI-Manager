//! 用 Cookie 添加账号：给「没有可借用登录流程」的平台用（目前只有豆包）。
//!
//! 为什么不做浏览器登录 / 借客户端登录：
//!   - 豆包网页登录走字节 SSO，没有可回调的授权码；
//!   - 豆包电脑版的登录态在 Chromium 加密 cookie 库里（DPAPI + v10/v20 信封），
//!     格式随客户端版本变，读它等于跟一个会变的私有格式绑定。
//!
//! 让用户从浏览器 DevTools 复制一次 Cookie，比上面两条路都稳，也不用本工具保管密码。
//!
//! **多账号**：iid 由 Cookie 的哈希派生——
//!   - 同一份 Cookie 重复添加 = 更新同一行（不会越加越多）；
//!   - 不同账号（不同 Cookie）各成一行，互不覆盖。
//!
//! 这也是豆包「可以开几个账号」的落地方式：一个账号一份 Cookie。

use crate::app::accounts::credential::Credential;
use crate::app::accounts::repo;
use crate::app::accounts::AccountIdentity;
use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use serde_json::json;
use sha2::{Digest, Sha256};

/// 支持「粘贴 Cookie」添加的平台。
pub fn supported(platform: &str) -> bool {
    matches!(platform, "doubao")
}

pub fn add(db: &Db, platform: &str, cookie: &str, name: &str) -> Result<AccountIdentity> {
    if !supported(platform) {
        return Err(AppError::new(
            ErrorKind::Unsupported,
            "这个平台不支持用 Cookie 添加账号",
        ));
    }
    let cookie = normalize(cookie);
    if cookie.is_empty() {
        return Err(AppError::new(ErrorKind::Invalid, "Cookie 不能为空"));
    }
    // 豆包的登录态在 sessionid / sessionid_ss 里。没有它，多半是复制了别的域
    // （比如从 doubao.com 的营销页复制的），或者根本没登录成功。
    if !cookie.contains("sessionid") {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "这份 Cookie 里没有 sessionid：请确认已在 doubao.com 登录，并从该站点复制整段 Cookie",
        ));
    }

    let digest = hex_digest(&cookie);
    let account_id = format!("db{}", &digest[..10]);
    let iid = format!("doubao:{account_id}");
    let name = name.trim();
    let name = if name.is_empty() {
        format!("豆包 {}", &account_id[account_id.len() - 4..])
    } else {
        name.to_string()
    };

    let identity = AccountIdentity {
        iid: iid.clone(),
        platform: "doubao".into(),
        platform_label: "豆包".into(),
        account_id: account_id.clone(),
        name,
        // Cookie 的过期由字节控制，平台不告诉我们；空 = 「有效期未知」，
        // 界面按「未知」显示，不编一个假日期出来。
        expires_at: None,
        source: "cookie".into(),
        requires_checkin: true,
        has_secret: true,
    };

    let mut cred = Credential {
        token: cookie,
        ..Default::default()
    };
    // seed 决定「设备标识」：同一账号每次都算出同一组 web_id / tea_uuid
    cred.put("seed", json!(account_id));
    cred.put("uid", json!(account_id));

    let now = chrono::Local::now().timestamp_millis();
    repo::upsert_identity(db, &identity, now)?;
    repo::set_secret(db, &iid, &cred.to_bytes()?, None)?;
    Ok(identity)
}

/// 规整用户粘贴的内容：`Cookie: a=b; c=d`、带换行、结尾多一个分号都要能收。
fn normalize(raw: &str) -> String {
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

/// 前 10 个十六进制字符足够做「这份 Cookie 的短指纹」：碰撞概率 1/16^10，
/// 而它只是本机账号行的主键，不承担任何安全职责。
fn hex_digest(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest
        .iter()
        .take(5)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::db::Db;

    fn temp_db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        (dir, db)
    }

    #[test]
    fn same_cookie_updates_one_row_and_different_cookies_add_rows() {
        let (_dir, db) = temp_db();
        let first = add(&db, "doubao", "sessionid=aaa; uid=1", "主号").unwrap();
        let again = add(&db, "doubao", "sessionid=aaa; uid=1", "主号（改名）").unwrap();
        let second = add(&db, "doubao", "sessionid=bbb; uid=2", "小号").unwrap();

        assert_eq!(first.iid, again.iid, "同一份 Cookie 必须落在同一行");
        assert_ne!(first.iid, second.iid, "不同账号必须是两行");
        assert_eq!(repo::list(&db).unwrap().len(), 2);
        // 重复添加时昵称跟着更新（用户改名是有效操作）
        let rows = repo::list(&db).unwrap();
        let row = rows.iter().find(|r| r.iid == first.iid).unwrap();
        assert_eq!(row.name, "主号（改名）");
    }

    #[test]
    fn cookie_without_sessionid_is_rejected() {
        let (_dir, db) = temp_db();
        let err = add(&db, "doubao", "foo=bar; baz=1", "x").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        assert!(err.message.contains("sessionid"), "{}", err.message);
    }

    #[test]
    fn empty_cookie_and_unknown_platform_are_rejected() {
        let (_dir, db) = temp_db();
        assert_eq!(
            add(&db, "doubao", "   ", "x").unwrap_err().kind,
            ErrorKind::Invalid
        );
        assert_eq!(
            add(&db, "qoder", "sessionid=1", "x").unwrap_err().kind,
            ErrorKind::Unsupported
        );
    }

    #[test]
    fn default_name_falls_back_to_short_id() {
        let (_dir, db) = temp_db();
        let identity = add(&db, "doubao", "sessionid=ccc", "").unwrap();
        assert!(identity.name.starts_with("豆包 "), "{}", identity.name);
        assert_eq!(identity.name.chars().count(), 7); // 「豆包 」3 字 + 4 位短 id
    }

    #[test]
    fn pasted_cookie_block_is_normalized() {
        assert_eq!(normalize("Cookie: a=1;\r\n b=2 ; c=3; "), "a=1; b=2; c=3");
    }
}
