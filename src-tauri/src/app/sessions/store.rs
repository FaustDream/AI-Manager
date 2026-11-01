//! 会话索引表（`session_index`）的读写。
//!
//! 表结构与 `docs/phase-4-sessions.md` §4 一致：索引字段建列，派生字段（msgs/size/state）
//! 可空、由扫描补齐。**列表页只读这张表，永远不碰客户端库**——这是 §2.2 的核心约定。

use crate::app::db::Db;
use crate::app::error::{AppError, Result};
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
        // ORDER BY 列名只能静态拼——三档都是白名单枚举，不存在注入面
        let sql = format!(
            "SELECT kind, source_key, source_label, account, session_id, title,
                    last_at, msgs, size, state, has_body
             FROM session_index
             WHERE (?1 IS NULL OR source_key = ?1)
               AND (?2 IS NULL OR account = ?2)
               AND (?3 IS NULL OR state = ?3)
               AND (?4 IS NULL OR {col} < ?4 OR ({col} = ?4 AND session_id < ?5))
             ORDER BY {col} DESC, session_id DESC
             LIMIT ?6"
        );

        self.db.read(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(
                rusqlite::params![source, account, state, cur, cur_sid, limit + 1],
                |row| {
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
                        size: row.get(8)?,
                        state: state_of(row.get::<_, Option<String>>(9)?.as_deref()),
                        has_body: row.get::<_, Option<i64>>(10)?.unwrap_or(0) != 0,
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
                "SELECT source_key, source_label, COUNT(*) FROM session_index
                 GROUP BY source_key, source_label ORDER BY source_label",
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
                "SELECT account, COUNT(*) FROM session_index GROUP BY account ORDER BY account",
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

            let mut stmt = conn.prepare(
                "SELECT state, COUNT(*) FROM session_index GROUP BY state ORDER BY state",
            )?;
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

            let total: i64 =
                conn.query_row("SELECT COUNT(*) FROM session_index", [], |r| r.get(0))?;
            facets.total = total as usize;
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
}

fn count_with(
    conn: &rusqlite::Connection,
    source: Option<&str>,
    account: Option<&str>,
    state: Option<&str>,
) -> Result<usize> {
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM session_index
         WHERE (?1 IS NULL OR source_key = ?1)
           AND (?2 IS NULL OR account = ?2)
           AND (?3 IS NULL OR state = ?3)",
        rusqlite::params![source, account, state],
        |row| row.get(0),
    )?;
    Ok(total as usize)
}

pub fn state_text(state: SessionState) -> &'static str {
    match state {
        SessionState::Visible => "visible",
        SessionState::Deleted => "deleted",
        SessionState::OldAccount => "old_account",
        SessionState::Cloud => "cloud",
        SessionState::Archived => "archived",
    }
}

pub fn state_label(state: SessionState) -> &'static str {
    match state {
        SessionState::Visible => "可见",
        SessionState::Deleted => "已删除",
        SessionState::OldAccount => "旧账号",
        SessionState::Cloud => "云端",
        SessionState::Archived => "已归档",
    }
}

fn state_of(value: Option<&str>) -> SessionState {
    match value {
        Some("deleted") => SessionState::Deleted,
        Some("old_account") => SessionState::OldAccount,
        Some("cloud") => SessionState::Cloud,
        Some("archived") => SessionState::Archived,
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
        .map_err(|_| AppError::new("invalid", "游标格式不对"))?;
    let text = String::from_utf8(text).map_err(|_| AppError::new("invalid", "游标不是 UTF-8"))?;
    let (tag, rest) = match text.split_once('|') {
        Some((tag, rest)) => (Sort::from_tag(tag), rest),
        // 旧格式：last_at:session_id
        None => (Sort::LastAt, text.as_str()),
    };
    let (value, sid) = rest
        .split_once(':')
        .ok_or_else(|| AppError::new("invalid", "游标缺少分隔符"))?;
    let value = value
        .parse::<i64>()
        .map_err(|_| AppError::new("invalid", "游标里的排序值不是整数"))?;
    Ok((tag, value, sid.to_string()))
}
