//! 会话索引表（`session_index`）的读写。
//!
//! 表结构与 `docs/phase-4-sessions.md` §4 一致：索引字段建列，派生字段（msgs/size/state）
//! 可空、由扫描补齐。**列表页只读这张表，永远不碰客户端库**——这是 §2.2 的核心约定。

use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::types::{Page, SessionRow, SessionState};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chrono::{Local, TimeZone};

/// 会话行的内部形态：时间用毫秒，状态用枚举。落表前才转字符串。
#[derive(Debug, Clone)]
pub struct RawRow {
    pub kind: String,
    pub source_key: String,
    pub source_label: String,
    pub account: String,
    pub session_id: String,
    pub title: String,
    /// 工作区/项目目录名。CodeBuddy 靠它定位正文目录，WorkBuddy 用 cwd
    pub slug: String,
    pub cwd: String,
    pub origin: String,
    pub last_at: i64,
    pub msgs: i64,
    pub size: i64,
    pub state: SessionState,
    pub has_body: bool,
}

impl RawRow {
    pub fn empty(kind: &str, source_key: &str, source_label: &str) -> Self {
        Self {
            kind: kind.into(),
            source_key: source_key.into(),
            source_label: source_label.into(),
            account: String::new(),
            session_id: String::new(),
            title: String::new(),
            slug: String::new(),
            cwd: String::new(),
            origin: String::new(),
            last_at: 0,
            msgs: 0,
            size: 0,
            state: SessionState::Visible,
            has_body: false,
        }
    }
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
/// 列表排序档位（STAGES §6.1：时间 / 大小 / 消息数）。全部倒序，游标里带档位。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    #[default]
    LastAt,
    Size,
    Msgs,
}

impl Sort {
    fn column(self) -> &'static str {
        match self {
            Sort::LastAt => "last_at",
            Sort::Size => "size",
            Sort::Msgs => "msgs",
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Sort::LastAt => "t",
            Sort::Size => "s",
            Sort::Msgs => "m",
        }
    }

    fn from_tag(tag: &str) -> Sort {
        match tag {
            "s" => Sort::Size,
            "m" => Sort::Msgs,
            _ => Sort::LastAt,
        }
    }
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
/// 列表筛选。三个下拉对应 §7 的草图，空值 = 不限。
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct Filter {
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub sort: Sort,
}

impl Filter {
    fn clean(value: &Option<String>) -> Option<&str> {
        value.as_deref().map(str::trim).filter(|s| !s.is_empty())
    }
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, serde::Serialize)]
pub struct FacetItem {
    pub value: String,
    pub label: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub count: usize,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Facets {
    pub sources: Vec<FacetItem>,
    pub accounts: Vec<FacetItem>,
    pub states: Vec<FacetItem>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total: usize,
}

pub struct SqliteSessions<'a> {
    db: &'a Db,
}

impl<'a> SqliteSessions<'a> {
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    /// 整源替换：扫描完一个来源就把它写回去。
    /// 先删后插而不是逐行 upsert——来源内的会话可能被客户端删掉，旧的留着就是幽灵行。
    pub fn replace_source(&self, source_key: &str, rows: &[RawRow]) -> Result<usize> {
        // seen_at = 这行是什么时候写进来的，由这一层打，扫描器不用关心时间
        let seen_at = Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        self.db.write(|conn| {
            let tx = conn.unchecked_transaction()?;
            tx.execute(
                "DELETE FROM session_index WHERE source_key = ?1",
                [source_key],
            )?;
            {
                let mut stmt = tx.prepare(
                    "INSERT OR REPLACE INTO session_index
                     (kind, source_key, source_label, account, session_id, title, slug, cwd,
                      origin, refs, last_at, seen_at, msgs, size, state, has_body)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'[]',?10,?11,?12,?13,?14,?15)",
                )?;
                for row in rows {
                    stmt.execute(rusqlite::params![
                        row.kind,
                        row.source_key,
                        row.source_label,
                        row.account,
                        row.session_id,
                        row.title,
                        row.slug,
                        row.cwd,
                        row.origin,
                        row.last_at,
                        seen_at,
                        row.msgs,
                        row.size,
                        state_text(row.state),
                        if row.has_body { 1 } else { 0 },
                    ])?;
                }
            }
            tx.commit()?;
            Ok(rows.len())
        })
    }

    pub fn drop_source(&self, source_key: &str) -> Result<()> {
        self.db.write(|conn| {
            conn.execute(
                "DELETE FROM session_index WHERE source_key = ?1",
                [source_key],
            )?;
            Ok(())
        })
    }

    /// 精确摘掉这几行索引（删除 / 彻底删除之后调用）。
    ///
    /// 不用 `drop_source`：那会把**整个来源**的列表清空，用户得手动重扫才看得见
    /// 别的会话——「删了一条，结果整页都没了」就是它干的。四段主键一起给，
    /// 少一段就可能误删同 id 的另一份会话（不同账号 / 不同版本共用 id）。
    pub fn forget(&self, rows: &[(String, String, String, String)]) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }
        self.db.write(|conn| {
            let mut stmt = conn.prepare(
                "DELETE FROM session_index
                 WHERE kind = ?1 AND source_key = ?2 AND account = ?3 AND session_id = ?4",
            )?;
            let mut removed = 0;
            for (kind, source, account, id) in rows {
                removed += stmt.execute(rusqlite::params![kind, source, account, id])?;
            }
            Ok(removed)
        })
    }

    /// 按 `(行 key, target)` 取 session_index 的标题（删除前取标题、迁移校验共用）。
    /// 一条连接循环查——每个 key 各开一次事务太浪费；查不到的行不返回。
    pub fn titles_of(
        &self,
        targets: &[(String, super::preview::Target)],
    ) -> Result<std::collections::HashMap<String, String>> {
        self.db.read(|conn| {
            let mut out = std::collections::HashMap::new();
            let Ok(mut stmt) = conn.prepare(
                "SELECT title FROM session_index
                 WHERE kind = ?1 AND source_key = ?2 AND account = ?3 AND session_id = ?4",
            ) else {
                return Ok(out);
            };
            for (key, target) in targets {
                let title: Option<String> = stmt
                    .query_row(
                        rusqlite::params![
                            target.kind,
                            target.source_key,
                            target.account,
                            target.session_id
                        ],
                        |row| row.get::<_, String>(0),
                    )
                    .ok();
                if let Some(title) = title {
                    out.insert(key.clone(), title);
                }
            }
            Ok(out)
        })
    }

    /// 迁移校验：`kind='workbuddy'` 的行数与会话 id 全集。
    pub fn workbuddy_keys(&self) -> Result<(u32, Vec<String>)> {
        self.db.read(|conn| {
            let rows: i64 = conn.query_row(
                "SELECT COUNT(*) FROM session_index WHERE kind = 'workbuddy'",
                [],
                |r| r.get(0),
            )?;
            let mut stmt =
                conn.prepare("SELECT session_id FROM session_index WHERE kind = 'workbuddy'")?;
            let ids = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((rows as u32, ids))
        })
    }

    /// 游标分页。`(排序键值, session_id)` 是排序键 + 唯一键，缺一个都会翻页重复。
    pub fn page(
        &self,
        filter: &Filter,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Page<SessionRow>> {
        let limit = limit.clamp(1, 500) as i64;
        let (cur_sort, cur_value, cur_sid) = match cursor {
            Some(raw) => decode_cursor(raw)?,
            None => (filter.sort, i64::MAX, String::new()),
        };
        // 游标里的档位与请求不一致（用户翻页中途换了排序）：从头开始，避免跨档位错位
        let (sort, cur) = if cur_sort == filter.sort {
            (filter.sort, cursor.map(|_| cur_value))
        } else {
            (filter.sort, None)
        };
        let source = Filter::clean(&filter.source);
        let account = Filter::clean(&filter.account);
        let state = Filter::clean(&filter.state);
        let col = sort.column();
        // 生效状态：已删除优先（客户端已经删了，归档与否没意义），其次看本地归档标记，
        // 其余沿用客户端给的 state。本地隐藏的行整行不出现。
        // ORDER BY 列名只能静态拼——三档都是白名单枚举，不存在注入面
        let sql = format!(
            "SELECT kind, source_key, source_label, account, session_id, title,
                    last_at, msgs, size, state, has_body, slug, cwd
             FROM (
                 SELECT s.kind, s.source_key, s.source_label, s.account, s.session_id, s.title,
                        s.last_at, s.msgs, s.size, s.has_body, s.slug, s.cwd,
                        {} AS state
                 {}
             )
             WHERE (?1 IS NULL OR source_key = ?1)
               AND (?2 IS NULL OR account = ?2)
               AND (?3 IS NULL OR state = ?3)
               AND (?3 IS NOT NULL OR state <> 'archived')
               AND (?4 IS NULL OR {col} < ?4 OR ({col} = ?4 AND session_id < ?5))
             ORDER BY {col} DESC, session_id DESC
             LIMIT ?6",
            EFFECTIVE_STATE_CASE, EFFECTIVE_STATE_FROM
        );

        self.db.read(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(
                rusqlite::params![source, account, state, cur, cur_sid, limit + 1,],
                |row| {
                    let slug: String = row.get(11)?;
                    let cwd: String = row.get(12)?;
                    Ok(SessionRow {
                        kind: row.get(0)?,
                        source_key: row.get(1)?,
                        source_label: row.get(2)?,
                        account: row.get(3)?,
                        // 昵称由 Service 层按账号库补（store 只管索引表）
                        account_label: String::new(),
                        session_id: row.get(4)?,
                        title: row.get(5)?,
                        last_at: ms_to_dt(row.get::<_, i64>(6)?),
                        msgs: row.get(7)?,
                        // 0 = 没有本地正文可管理（云端占位 / 无正文 / ZCode 这类
                        // 不逐条算体积的来源），显示为「—」而不是误导性的「0 B」
                        size: match row.get::<_, i64>(8)? {
                            0 => None,
                            n => Some(n),
                        },
                        state: state_of(row.get::<_, Option<String>>(9)?.as_deref()),
                        has_body: row.get::<_, Option<i64>>(10)?.unwrap_or(0) != 0,
                        project: project_name(&slug, &cwd),
                        cwd,
                    })
                },
            )?;
            let mut items = Vec::new();
            for item in rows {
                items.push(item?);
            }
            // 多取一条用来判断「还有没有下一页」，不额外 count
            let next_cursor = if items.len() as i64 > limit {
                items.truncate(limit as usize);
                items.last().map(|row| {
                    let value = match sort {
                        Sort::LastAt => row.last_at.map(|d| d.timestamp_millis()).unwrap_or(0),
                        Sort::Size => row.size.unwrap_or(0),
                        Sort::Msgs => row.msgs.unwrap_or(0),
                    };
                    encode_cursor(sort, value, &row.session_id)
                })
            } else {
                None
            };
            let total = count_with(conn, source, account, state)?;
            Ok(Page {
                items,
                next_cursor,
                total,
            })
        })
    }

    pub fn facets(&self) -> Result<Facets> {
        self.db.read(|conn| {
            let mut facets = Facets::default();
            let mut stmt = conn.prepare(
                "SELECT s.source_key, s.source_label, COUNT(*) FROM session_index s
                 LEFT JOIN session_marks m
                   ON m.source_key = s.source_key AND m.session_id = s.session_id
                 WHERE m.hidden_at IS NULL AND m.archived_at IS NULL
                 GROUP BY s.source_key, s.source_label ORDER BY s.source_label",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(FacetItem {
                    value: row.get(0)?,
                    label: row.get(1)?,
                    count: row.get::<_, i64>(2)? as usize,
                })
            })?;
            for item in rows {
                facets.sources.push(item?);
            }

            let mut stmt = conn.prepare(
                "SELECT s.account, COUNT(*) FROM session_index s
                 LEFT JOIN session_marks m
                   ON m.source_key = s.source_key AND m.session_id = s.session_id
                 WHERE m.hidden_at IS NULL AND m.archived_at IS NULL
                 GROUP BY s.account ORDER BY s.account",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(FacetItem {
                    value: row.get::<_, String>(0)?,
                    label: String::new(),
                    count: row.get::<_, i64>(1)? as usize,
                })
            })?;
            for item in rows {
                facets.accounts.push(item?);
            }

            // 状态计数与活跃列表同一口径：已归档单独成列表，不混进这里
            let sql = format!(
                "SELECT state, COUNT(*) FROM ({}) WHERE state <> 'archived' GROUP BY state",
                effective_state_sql()
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| {
                let value: String = row.get(0)?;
                Ok(FacetItem {
                    label: state_label(state_of(Some(&value))).to_string(),
                    value,
                    count: row.get::<_, i64>(1)? as usize,
                })
            })?;
            for item in rows {
                facets.states.push(item?);
            }

            facets.total = facets.sources.iter().map(|s| s.count).sum();
            Ok(facets)
        })
    }

    pub fn count(&self) -> Result<usize> {
        self.db.read(|conn| {
            let total: i64 =
                conn.query_row("SELECT COUNT(*) FROM session_index", [], |r| r.get(0))?;
            Ok(total as usize)
        })
    }

    /// 单行的**生效状态**（含归档合并）。行不存在返回 `None`。
    /// 彻底删除前用它判断「这条能不能删」，与列表显示的口径完全一致。
    pub fn state_of(&self, source_key: &str, session_id: &str) -> Result<Option<SessionState>> {
        let sql = format!(
            "SELECT state FROM ({})
             WHERE source_key = ?1 AND session_id = ?2",
            effective_state_sql()
        );
        self.db.read(|conn| {
            let text: Option<String> = conn
                .query_row(&sql, rusqlite::params![source_key, session_id], |row| {
                    row.get(0)
                })
                .ok();
            Ok(text.as_deref().map(|value| state_of(Some(value))))
        })
    }

    /// 当前筛选下的**全部行 key**（不分页），给「选中全部」用。
    /// key 的拼法与界面一致：`kind|source_key|account|session_id`。
    pub fn keys(&self, filter: &Filter) -> Result<Vec<String>> {
        let source = Filter::clean(&filter.source);
        let account = Filter::clean(&filter.account);
        let state = Filter::clean(&filter.state);
        let sql = format!(
            "SELECT kind, source_key, account, session_id FROM ({})
             WHERE (?1 IS NULL OR source_key = ?1)
               AND (?2 IS NULL OR account = ?2)
               AND (?3 IS NULL OR state = ?3)
               AND (?3 IS NOT NULL OR state <> 'archived')",
            effective_state_sql()
        );
        self.db.read(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params![source, account, state], |row| {
                let kind: String = row.get(0)?;
                let source_key: String = row.get(1)?;
                let account: String = row.get(2)?;
                let session_id: String = row.get(3)?;
                Ok(format!("{kind}|{source_key}|{account}|{session_id}"))
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }
}

/// 项目名。三种来源各有各的形态，统一成「人能看懂的一句话」：
///   - CodeBuddy 的 slug 是工作区目录的哈希（32 位十六进制），对人没意义，忽略；
///   - 其余来源只有 cwd，取最后一段；
///   - WorkBuddy 给「非项目会话」会自动建 `<...>/WorkBuddy/<时间戳>` 的临时工作区，
///     那不是项目，标成「临时」比显示一串日期有用。
///
/// 什么都拿不到就返回空串，界面显示「—」。
fn project_name(slug: &str, cwd: &str) -> String {
    let slug = slug.trim();
    if !slug.is_empty() && !looks_like_hash(slug) {
        return slug.to_string();
    }
    let path = cwd.trim().trim_end_matches(['/', '\\']);
    if path.is_empty() {
        return String::new();
    }
    let last = path.rsplit(['/', '\\']).next().unwrap_or("");
    if is_scratch_workspace(path, last) {
        return "临时".to_string();
    }
    last.to_string()
}

fn looks_like_hash(text: &str) -> bool {
    text.len() >= 16 && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// `2026-11-02-08-51-01` 这种「时间戳目录」= WorkBuddy 的临时工作区
fn is_scratch_workspace(path: &str, last: &str) -> bool {
    let timestamp_like = last.len() == 19
        && last.chars().enumerate().all(|(i, c)| {
            if matches!(i, 4 | 7 | 10 | 13 | 16) {
                c == '-'
            } else {
                c.is_ascii_digit()
            }
        });
    timestamp_like && path.contains("WorkBuddy")
}

/// 「生效状态」只有一份口径：已删除优先，其次本地归档，最后沿用客户端状态。
const EFFECTIVE_STATE_CASE: &str = "CASE
        WHEN COALESCE(s.state, 'visible') = 'deleted' THEN 'deleted'
        WHEN m.archived_at IS NOT NULL THEN 'archived'
        ELSE COALESCE(s.state, 'visible')
    END";

/// 本地隐藏的行整行不出现。
const EFFECTIVE_STATE_FROM: &str = "FROM session_index s
    LEFT JOIN session_marks m
      ON m.source_key = s.source_key AND m.session_id = s.session_id
    WHERE m.hidden_at IS NULL";

/// 计数 / 筛选 / 单行状态共用同一份生效状态结果集，口径与 `page` 完全一致。
fn effective_state_sql() -> String {
    format!(
        "SELECT s.kind, s.source_key, s.session_id, s.account, s.last_at,
                {} AS state
         {}",
        EFFECTIVE_STATE_CASE, EFFECTIVE_STATE_FROM
    )
}

fn count_with(
    conn: &rusqlite::Connection,
    source: Option<&str>,
    account: Option<&str>,
    state: Option<&str>,
) -> Result<usize> {
    let sql = format!(
        "SELECT COUNT(*) FROM ({})
         WHERE (?1 IS NULL OR source_key = ?1)
           AND (?2 IS NULL OR account = ?2)
           AND (?3 IS NULL OR state = ?3)
           AND (?3 IS NOT NULL OR state <> 'archived')",
        effective_state_sql()
    );
    let total: i64 = conn.query_row(&sql, rusqlite::params![source, account, state], |row| {
        row.get(0)
    })?;
    Ok(total as usize)
}

pub fn state_text(state: SessionState) -> &'static str {
    match state {
        SessionState::Visible => "visible",
        SessionState::Deleted => "deleted",
        SessionState::OldAccount => "old_account",
        SessionState::Cloud => "cloud",
        SessionState::Archived => "archived",
        SessionState::Orphan => "orphan",
    }
}

pub fn state_label(state: SessionState) -> &'static str {
    match state {
        SessionState::Visible => "可见",
        SessionState::Deleted => "已删除",
        SessionState::OldAccount => "旧账号",
        SessionState::Cloud => "云端",
        SessionState::Archived => "已归档",
        SessionState::Orphan => "孤立",
    }
}

fn state_of(value: Option<&str>) -> SessionState {
    match value {
        Some("deleted") => SessionState::Deleted,
        Some("old_account") => SessionState::OldAccount,
        Some("cloud") => SessionState::Cloud,
        Some("archived") => SessionState::Archived,
        Some("orphan") => SessionState::Orphan,
        _ => SessionState::Visible,
    }
}

fn ms_to_dt(ms: i64) -> Option<chrono::DateTime<Local>> {
    if ms <= 0 {
        return None;
    }
    Local.timestamp_millis_opt(ms).single()
}

/// 游标里必须同时有排序档位、排序键与唯一键，否则数据增删会让翻页错位。
/// 旧格式（无档位前缀）按时间档解读，兼容升级前发出的游标。
pub fn encode_cursor(sort: Sort, value: i64, session_id: &str) -> String {
    B64.encode(format!("{}|{value}:{session_id}", sort.tag()))
}

pub fn decode_cursor(raw: &str) -> Result<(Sort, i64, String)> {
    let text = B64
        .decode(raw)
        .map_err(|_| AppError::new(ErrorKind::Invalid, "游标格式不对"))?;
    let text =
        String::from_utf8(text).map_err(|_| AppError::new(ErrorKind::Invalid, "游标不是 UTF-8"))?;
    let (tag, rest) = match text.split_once('|') {
        Some((tag, rest)) => (Sort::from_tag(tag), rest),
        // 旧格式：last_at:session_id
        None => (Sort::LastAt, text.as_str()),
    };
    let (value, sid) = rest
        .split_once(':')
        .ok_or_else(|| AppError::new(ErrorKind::Invalid, "游标缺少分隔符"))?;
    let value = value
        .parse::<i64>()
        .map_err(|_| AppError::new(ErrorKind::Invalid, "游标里的排序值不是整数"))?;
    Ok((tag, value, sid.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_name_prefers_slug_then_last_segment() {
        // CodeBuddy 有工作区名，直接用
        assert_eq!(project_name("AI Manager", "D:/whatever"), "AI Manager");
        // 其余来源只有 cwd：取最后一段，正反斜杠都要认
        assert_eq!(project_name("", "D:/AI Manager"), "AI Manager");
        assert_eq!(project_name("", r"D:/AI Manager/"), "AI Manager");
        assert_eq!(project_name("  ", "/home/lynn/work/"), "work");
        // 都没有就留空，界面显示「—」
        assert_eq!(project_name("", ""), "");
        assert_eq!(project_name("", "   "), "");
    }
}
