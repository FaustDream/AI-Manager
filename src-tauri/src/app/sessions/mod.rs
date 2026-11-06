//! 会话中心的数据层：索引构建（增量）+ 游标分页 + 筛选。
//!
//! 两条铁律（`docs/phase-4-sessions.md` §2）：
//!   1. **列表只读 `app.db`**，永远不碰客户端库——客户端正在写的时候列表也不能转圈；
//!   2. 派生字段（消息数 / 体积 / 状态）由扫描补齐，扫到哪补到哪。
//!
//! 增量靠源指纹：`sources()` 只 stat 元数据，指纹没变就整源复用索引。

pub mod backend;
pub mod delete;
pub mod marks;
pub mod preview;
pub mod restore;
pub mod sources;
pub mod store;

pub use preview::Preview;
pub use store::{Facets, Filter, RawRow, SqliteSessions};

use crate::app::accounts::{is_intl_account, uid_of};
use crate::app::clients::Roots;
use crate::app::db::Db;
use crate::app::error::{ErrorKind, Result};
use crate::app::runtime::blocking;
use crate::app::sessions::sources::MsgCache;
use crate::contracts::{RestoreReport, SessionStore};
use crate::types::{Page, SessionRow, SessionState};
use serde::Serialize;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone, Default, Serialize)]
pub struct ScanReport {
    /// 真去扫了的来源数
    pub scanned: usize,
    /// 因指纹没变被跳过的来源数
    pub skipped: usize,
    /// 写进索引的行数
    pub rows: usize,
    /// 本轮到点自动归档的条数
    pub archived: usize,
    /// 客户端没装/没登录过的来源
    pub missing: usize,
    pub ms: u64,
    pub errors: Vec<String>,
}

impl ScanReport {
    pub fn summary(&self) -> String {
        let mut text = format!(
            "扫描 {} 个来源，跳过 {} 个，写入 {} 条，用时 {:.1}s",
            self.scanned,
            self.skipped,
            self.rows,
            self.ms as f64 / 1000.0
        );
        if self.missing > 0 {
            text.push_str(&format!("，{} 个来源未安装", self.missing));
        }
        if self.archived > 0 {
            text.push_str(&format!("，自动归档 {}", self.archived));
        }
        if !self.errors.is_empty() {
            text.push_str(&format!("，{} 个来源报错", self.errors.len()));
        }
        text
    }
}

#[derive(Clone)]
pub struct Service {
    db: Arc<Db>,
    roots: Roots,
}

impl Service {
    pub fn new(db: Arc<Db>, roots: Roots) -> Self {
        Self { db, roots }
    }

    pub fn roots(&self) -> &Roots {
        &self.roots
    }

    /// 增量扫描：指纹没变的来源整源跳过，变了的重扫并整源替换。
    pub async fn scan(&self) -> Result<ScanReport> {
        let db = self.db.clone();
        let roots = self.roots.clone();
        blocking(move || {
            let started = Instant::now();
            let mut report = ScanReport::default();
            let store = SqliteSessions::new(&db);
            let infos = sources::sources(&roots);
            let mut seen = BTreeSet::new();
            // 消息计数缓存放 kv：一次读入、一次写回，不为每个文件建行
            let mut cache: MsgCache = db
                .kv_get("sess_msg_cache")
                .ok()
                .flatten()
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or_default();

            for info in &infos {
                seen.insert(info.key.clone());
                if !info.present {
                    report.missing += 1;
                    continue;
                }
                let kv_key = format!("sess_fp:{}", info.key);
                if db.kv_get(&kv_key).ok().flatten().as_deref() == Some(info.fp.as_str()) {
                    report.skipped += 1;
                    continue;
                }
                let rows = sources::scan_one(&roots, &info.key, &mut cache, &mut report.errors)?;
                report.rows += store.replace_source(&info.key, &rows)?;
                report.scanned += 1;
                // 写库失败时不要把指纹落下去，否则这个来源永远不会再被扫
                db.kv_set(&kv_key, &info.fp)?;
            }

            // 客户端卸载了：索引里留着的行会让页面出现点不动的幽灵行
            for facet in store.facets()?.sources {
                if !seen.contains(&facet.value) {
                    store.drop_source(&facet.value)?;
                    let _ = db.kv_delete(&format!("sess_fp:{}", facet.value));
                }
            }

            if let Ok(text) = serde_json::to_string(&cache) {
                let _ = db.kv_set("sess_msg_cache", &text);
            }

            // 到点自动归档：索引刚重建完，这时候算「谁过期了」最准。
            // 归档只写本地标记，不碰客户端库——后台动作不该去关用户的客户端。
            match marks::auto_archive(&db, marks::retention_days(&db)) {
                Ok(0) => {}
                Ok(count) => report.archived = count,
                Err(e) => report.errors.push(format!("自动归档失败：{}", e.message)),
            }

            report.ms = started.elapsed().as_millis() as u64;
            Ok(report)
        })
        .await?
    }

    pub async fn page(
        &self,
        filter: Filter,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<Page<SessionRow>> {
        let db = self.db.clone();
        blocking(move || {
            // 「到时间就归档」不能只在扫描时算：软件可能开着好几天不扫。
            // 列表是最常走的路，这里顺手检查一次，10 分钟内不重复。
            maybe_auto_archive(&db);
            let mut page = SqliteSessions::new(&db).page(&filter, cursor.as_deref(), limit)?;
            // 账号列显示昵称（界面不出现 uid）；uid 原文仍留给行 key 与筛选
            let names = account_names(&db);
            for row in &mut page.items {
                row.account_label = account_label(&row.account, &names);
            }
            Ok(page)
        })
        .await?
    }

    /// 归档 / 取消归档（只写本地标记，不动客户端）。
    pub async fn archive(&self, keys: Vec<String>, on: bool) -> Result<usize> {
        let db = self.db.clone();
        blocking(move || marks::archive(&db, &keys, on)).await?
    }

    /// 彻底删除：**可见会话必须先归档**；已归档 / 已删除 / 云端占位 / 旧账号
    /// 可以直接删。门槛放在后端而不是只靠界面置灰：绕过界面直接调命令也不行。
    pub async fn purge(&self, keys: Vec<String>) -> Result<delete::PurgeReport> {
        self.purge_with(keys, true).await
    }

    /// `with_guard=false` 只给测试用：不碰真实客户端进程。
    #[doc(hidden)]
    pub async fn purge_with(
        &self,
        keys: Vec<String>,
        with_guard: bool,
    ) -> Result<delete::PurgeReport> {
        let db = self.db.clone();
        let roots = self.roots.clone();
        blocking(move || {
            let store = SqliteSessions::new(&db);
            let mut allowed = Vec::new();
            let mut refused = 0usize;
            for key in &keys {
                let target = preview::parse_key(key)?;
                let state = store.state_of(&target.source_key, &target.session_id)?;
                match state {
                    Some(SessionState::Visible) | None => refused += 1,
                    Some(_) => allowed.push(key.clone()),
                }
            }
            if allowed.is_empty() {
                return Err(crate::app::error::AppError::new(
                    ErrorKind::Invalid,
                    "可见会话不能直接删除；请先归档，再在归档列表中彻底删除",
                ));
            }
            let mut report = delete::purge(&roots, &db, &allowed, with_guard)?;
            report.refused = refused;
            Ok(report)
        })
        .await?
    }

    /// 当前筛选下的全部行 key（不分页），给「选中全部」用。
    pub async fn keys(&self, filter: Filter) -> Result<Vec<String>> {
        let db = self.db.clone();
        blocking(move || SqliteSessions::new(&db).keys(&filter)).await?
    }

    /// 自动归档的保留天数（0 = 关闭）。
    pub async fn retention_days(&self) -> u32 {
        let db = self.db.clone();
        blocking(move || marks::retention_days(&db))
            .await
            .unwrap_or(0)
    }

    pub async fn set_retention_days(&self, days: u32) -> Result<u32> {
        let db = self.db.clone();
        blocking(move || {
            marks::set_retention_days(&db, days)?;
            // 改档位立刻按新档位算一次：用户调成 3 天后马上能看到效果
            let _ = marks::auto_archive(&db, days);
            Ok(marks::retention_days(&db))
        })
        .await?
    }

    /// 正文预览：按行 key 找正文，三种来源各自的解析在 `preview.rs`。
    pub async fn preview(&self, key: String) -> Result<Preview> {
        let roots = self.roots.clone();
        blocking(move || {
            let target = preview::parse_key(&key)?;
            Ok(preview::load(&roots, &target))
        })
        .await?
    }

    pub async fn facets(&self) -> Result<Facets> {
        let db = self.db.clone();
        blocking(move || {
            let mut facets = SqliteSessions::new(&db).facets()?;
            // 账号列显示昵称：库里有账号就补名字，没有的（Qoder / ZCode / Trae）显示「本机」
            let names = account_names(&db);
            for item in &mut facets.accounts {
                item.label = account_label(&item.value, &names);
            }
            Ok(facets)
        })
        .await?
    }
}

/// 上次跑自动归档的秒级时间戳。进程内节流：列表翻页很密，不能每页都算一遍。
static LAST_AUTO_ARCHIVE: AtomicI64 = AtomicI64::new(0);
const AUTO_ARCHIVE_INTERVAL_SECS: i64 = 600;

/// 到点自动归档的「时间触发器」：只在列表被打开时检查，10 分钟一次。
/// 归档只写本地标记，不碰客户端库，所以这里可以放心在读写路径上顺手做。
fn maybe_auto_archive(db: &Db) {
    let now = chrono::Local::now().timestamp();
    let last = LAST_AUTO_ARCHIVE.load(Ordering::Relaxed);
    if now - last < AUTO_ARCHIVE_INTERVAL_SECS {
        return;
    }
    LAST_AUTO_ARCHIVE.store(now, Ordering::Relaxed);
    let _ = marks::auto_archive(db, marks::retention_days(db));
}

/// 账号库的 (platform, iid, name) 列表，给账号列昵称映射用。
fn account_names(db: &Db) -> Vec<(String, String, String)> {
    crate::app::accounts::repo::list(db)
        .map(|list| {
            list.into_iter()
                .map(|view| (view.platform.clone(), view.iid.clone(), view.name.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// 会话行的账号列取值是客户端里的 uid；账号库里的 iid 是 `平台:uid`（国际版带 `@intl`）。
/// 匹配不上（Qoder / ZCode / Trae 的库根本没有账号字段）就显示「本机」。
fn account_label(uid: &str, names: &[(String, String, String)]) -> String {
    if uid.is_empty() {
        return "本机".into();
    }
    for (platform, iid, name) in names {
        let Some((_, account_id)) = iid.split_once(':') else {
            continue;
        };
        if account_id == uid || uid_of(account_id) == uid {
            let tag = if is_intl_account(account_id) {
                "国际版"
            } else {
                ""
            };
            let _ = platform;
            return if tag.is_empty() {
                name.clone()
            } else {
                format!("{name}（{tag}）")
            };
        }
    }
    if uid == "default" {
        "未登录".into()
    } else {
        "未知账号".into()
    }
}

/// 契约接口的同步实现：`page` 走本地索引，`restore_account` 走客户端库。
/// 命令层用异步的 `Service`，契约测试用这个——两者共用同一份 SQL 与同一个恢复函数。
pub struct Store {
    db: Arc<Db>,
    roots: Roots,
}

impl Store {
    pub fn new(db: Arc<Db>, roots: Roots) -> Self {
        Self { db, roots }
    }
}

impl SessionStore for Store {
    fn page(
        &self,
        cursor: Option<&str>,
        limit: usize,
        state: Option<&str>,
    ) -> Result<Page<SessionRow>> {
        let filter = Filter {
            state: state.map(str::to_string),
            ..Default::default()
        };
        SqliteSessions::new(&self.db).page(&filter, cursor, limit)
    }

    fn restore_account(&self, edition: &str, uid: &str) -> Result<RestoreReport> {
        // 主体名：库里查得到就用昵称，查不到退回 uid（执行记录要能看出是谁）
        let subject = crate::app::accounts::repo::list(&self.db)
            .ok()
            .and_then(|list| {
                list.into_iter()
                    .find(|a| crate::app::accounts::uid_of(&a.iid) == uid)
            })
            .map(|a| a.name)
            .unwrap_or_else(|| uid.to_string());
        restore::account(&self.roots, edition, uid, &subject)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_labels() {
        let names = vec![
            (
                "workbuddy".to_string(),
                "workbuddy:u1".to_string(),
                "慎静绛思".to_string(),
            ),
            (
                "workbuddy".to_string(),
                "workbuddy:u2@intl".to_string(),
                "Faust".to_string(),
            ),
        ];
        assert_eq!(account_label("u1", &names), "慎静绛思");
        assert_eq!(account_label("u2", &names), "Faust（国际版）");
        assert_eq!(account_label("", &names), "本机");
        assert_eq!(account_label("nobody", &names), "未知账号");
        assert_eq!(account_label("default", &[]), "未登录");
    }
}
