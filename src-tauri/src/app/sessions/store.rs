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

/// 标题的统一兜底：各来源扫不出标题时都写这一个字面量，列表的「未命名」筛选
/// 也按它匹配（SQL 与扫描器共用一份常量，改了这处两边同步）。
pub const UNNAMED_TITLE: &str = "（未命名会话）";

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
/// 列表筛选。下拉与快筛叠加使用，空值 = 不限。
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct Filter {
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    /// 标题（含本地命名）/ 来源 / 项目路径的模糊搜索；空值 = 不限
    #[serde(default)]
    pub q: Option<String>,
    /// 运行形态：`desktop` / `ide-plugin`。IDEA 插件标签用这个维度聚合各软件插件会话。
    #[serde(default)]
    pub origin: Option<String>,
    /// 只看「未命名」会话（生效标题是占位文案 UNNAMED_TITLE 或空）。false = 不限。
    #[serde(default)]
    pub unnamed: bool,
    /// 只看置顶会话。
    #[serde(default)]
    pub pinned: bool,
    /// 只看「本地隐藏」的会话（Trae 家族的墓碑也在里面，可从这里取消隐藏）。
    #[serde(default)]
    pub hidden: bool,
    /// 正文全文搜索（FTS5）。空值 = 不限；与 `q`（标题级）相互独立。
    #[serde(default)]
    pub body: Option<String>,
    /// 标签多选（命中任一即通过）。空 = 不限。
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub sort: Sort,
}

impl Filter {
    fn clean(value: &Option<String>) -> Option<&str> {
        value.as_deref().map(str::trim).filter(|s| !s.is_empty())
    }

    /// 「未命名」筛选的 SQL 片段：开了就只留占位标题与空标题的行。
    /// 字面量是编译期常量且不含引号，静态拼进去没有注入面。
    /// `title` 是外层结果集的**生效标题**（本地命名已覆盖），三个查询入口口径一致。
    fn unnamed_clause(unnamed: bool) -> String {
        if unnamed {
            format!("AND (x.title = '{UNNAMED_TITLE}' OR TRIM(x.title) = '')")
        } else {
            String::new()
        }
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
    pub tags: Vec<FacetItem>,
    /// 「未命名」会话数（与活跃列表同口径：不含已归档 / 已隐藏），给筛选 chip 当角标。
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub unnamed: usize,
    /// 置顶数（活跃口径），给「只看置顶」chip 当角标。
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub pinned: usize,
    /// 本地隐藏数（活跃口径），给「已隐藏」chip 当角标。
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub hidden: usize,
    /// 正文全文索引的进度：界面提示「索引建立中 x/y」。
    pub fts: FtsStatus,
    /// 已归档总数（活跃页「已归档（N）」入口的角标）。
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub archived: usize,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total: usize,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct FtsStatus {
    /// 已建好正文索引的会话数
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub indexed: usize,
    /// 可索引的会话总数（有本地正文的来源）
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

    /// 游标分页。`(置顶, 排序键值, session_id)` 是排序键 + 唯一键，缺一个都会翻页重复。
    /// 置顶行永远排在最前，所以排序键里带上了置顶位（行值比较保证跨页不错位）。
    pub fn page(
        &self,
        filter: &Filter,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Page<SessionRow>> {
        let limit = limit.clamp(1, 500) as i64;
        let (cur_sort, cur_pinned, cur_value, cur_sid) = match cursor {
            Some(raw) => decode_cursor(raw)?,
            // 首页从最大值起步：置顶=1 才能把置顶行也算进「小于起点」的集合
            None => (filter.sort, true, i64::MAX, String::new()),
        };
        // 游标里的档位与请求不一致（用户翻页中途换了排序）：从头开始，避免跨档位错位
        let resume = cur_sort == filter.sort;
        let (sort, cur_value, cur_pinned) = if resume {
            (filter.sort, Some(cur_value), Some(cur_pinned))
        } else {
            (filter.sort, None, None)
        };

        let (where_sql, mut params, body_phrase) = filter_where_parts(filter);

        let col = sort.column();
        // 位置参数按 SQL 文本里出现的顺序排：筛选条件（见 filter_where_parts）→
        // 游标值 ×2（IS NULL 判断 + 行值比较）→ 游标置顶位 → 游标 sid → LIMIT。
        // 注意一个逻辑参数可能对应多个 `?` 占位（`? IS NULL OR col = ?`），
        // filter_where_parts 已经把参数按占位数复制好了，这里只追加 page 独有的部分。
        params.push(cur_value.into());
        params.push((cur_pinned.unwrap_or(false) as i64).into());
        params.push(cur_value.into());
        params.push(cur_sid.into());
        params.push((limit + 1).into());

        // 生效状态与生效标题都只有一份口径（见 effective_state_sql）；本地隐藏的行
        // 只在「只看已隐藏」视图出现。外层固定起别名 x：标签/正文的相关子查询里
        // 引用外层列必须带前缀，否则 SQLite 先解析到子查询自己的同名列，恒真。
        // ORDER BY 列名只能静态拼——三档都是白名单枚举。
        let sql = format!(
            "SELECT kind, source_key, source_label, account, session_id, title,
                    title_override, pinned, last_at, msgs, size, state, has_body, slug, cwd, origin
             FROM ({}) AS x
             {}
               AND (? IS NULL OR (x.pinned, x.{col}, x.session_id) < (?, ?, ?))
             ORDER BY x.pinned DESC, x.{col} DESC, x.session_id DESC
             LIMIT ?",
            effective_state_sql(filter.hidden),
            where_sql,
        );

        self.db.read(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |row| {
                let slug: String = row.get(13)?;
                let cwd: String = row.get(14)?;
                let title_override: Option<String> = row.get(6)?;
                Ok(SessionRow {
                    kind: row.get(0)?,
                    source_key: row.get(1)?,
                    source_label: row.get(2)?,
                    account: row.get(3)?,
                    // 昵称由 Service 层按账号库补（store 只管索引表）
                    account_label: String::new(),
                    session_id: row.get(4)?,
                    // 生效标题：本地命名覆盖客户端标题（renamed 由调用方区分）
                    title: row.get(5)?,
                    last_at: ms_to_dt(row.get::<_, i64>(8)?),
                    msgs: row.get(9)?,
                    // 0 = 没有本地正文可管理（云端占位 / 无正文 / ZCode 这类
                    // 不逐条算体积的来源），显示为「—」而不是误导性的「0 B」
                    size: match row.get::<_, i64>(10)? {
                        0 => None,
                        n => Some(n),
                    },
                    state: state_of(row.get::<_, Option<String>>(11)?.as_deref()),
                    has_body: row.get::<_, Option<i64>>(12)?.unwrap_or(0) != 0,
                    pinned: row.get::<_, i64>(7)? != 0,
                    renamed: title_override.is_some(),
                    // 正文命中摘要由 page 尾部补（FTS 查询，标题搜索时为 None）
                    snippet: None,
                    project: project_name(&slug, &cwd),
                    cwd,
                    origin: row.get(15)?,
                })
            })?;
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
                    encode_cursor(sort, row.pinned, value, &row.session_id)
                })
            } else {
                None
            };
            // 正文搜索：命中行补摘要（FTS 按行查，一页最多几十次微查询）
            if let Some(phrase) = &body_phrase {
                for row in &mut items {
                    row.snippet = snippet_of(conn, phrase, &row.source_key, &row.session_id);
                }
            }
            let total = count_with(conn, filter)?;
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
                effective_state_sql(false)
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

            // 「未命名」角标：与活跃列表同口径（不含已归档 / 已隐藏）
            let sql = format!(
                "SELECT COUNT(*) FROM ({})
                 WHERE state <> 'archived' AND (title = ?1 OR TRIM(title) = '')",
                effective_state_sql(false)
            );
            facets.unnamed =
                conn.query_row(&sql, [UNNAMED_TITLE], |r| r.get::<_, i64>(0))? as usize;

            // 置顶 / 已隐藏角标：同样按活跃口径（不含已归档）
            let sql = format!(
                "SELECT COUNT(*) FROM ({}) WHERE state <> 'archived' AND pinned = 1",
                effective_state_sql(false)
            );
            facets.pinned = conn.query_row(&sql, [], |r| r.get::<_, i64>(0))? as usize;
            let sql = format!(
                "SELECT COUNT(*) FROM ({}) WHERE state <> 'archived'",
                effective_state_sql(true)
            );
            facets.hidden = conn.query_row(&sql, [], |r| r.get::<_, i64>(0))? as usize;

            // 标签计数：不含已隐藏的墓碑（那些行在两个列表里都看不到）
            let mut stmt = conn.prepare(
                "SELECT t.tag, COUNT(*) FROM session_tag t
                 JOIN session_index s
                   ON s.source_key = t.source_key AND s.session_id = t.session_id
                 LEFT JOIN session_marks m
                   ON m.source_key = s.source_key AND m.session_id = s.session_id
                 WHERE m.hidden_at IS NULL
                 GROUP BY t.tag ORDER BY COUNT(*) DESC, t.tag",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(FacetItem {
                    value: row.get(0)?,
                    label: row.get(0)?,
                    count: row.get::<_, i64>(1)? as usize,
                })
            })?;
            for item in rows {
                facets.tags.push(item?);
            }

            facets.fts = fts_status(conn)?;
            facets.archived = conn.query_row(
                "SELECT COUNT(*) FROM session_index s
                 LEFT JOIN session_marks m
                   ON m.source_key = s.source_key AND m.session_id = s.session_id
                 WHERE m.hidden_at IS NULL AND m.archived_at IS NOT NULL",
                [],
                |r| r.get::<_, i64>(0),
            )? as usize;
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
            effective_state_sql(false)
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
    /// 条件必须与 `page` 完全同构（含正文/标签/置顶/隐藏），否则「全选」
    /// 选中集会和列表展示对不上。
    pub fn keys(&self, filter: &Filter) -> Result<Vec<String>> {
        let (where_sql, params, _) = filter_where_parts(filter);
        let sql = format!(
            "SELECT kind, source_key, account, session_id FROM ({}) AS x {}",
            effective_state_sql(filter.hidden),
            where_sql,
        );
        self.db.read(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |row| {
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

/// 项目名（stats.rs 也用）。三种来源各有各的形态，统一成「人能看懂的一句话」：
///   - CodeBuddy 的 slug 是工作区目录的哈希（32 位十六进制），对人没意义，忽略；
///   - 其余来源只有 cwd，取最后一段；
///   - WorkBuddy 给「非项目会话」会自动建 `<...>/WorkBuddy/<时间戳>` 的临时工作区，
///     那不是项目，标成「临时」比显示一串日期有用。
///
/// 什么都拿不到就返回空串，界面显示「—」。
pub(crate) fn project_name(slug: &str, cwd: &str) -> String {
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

/// 「生效状态」与「生效标题」只有一份口径：
///   - 状态：已删除优先（客户端已经删了，归档与否没意义），其次本地归档标记，
///     最后沿用客户端给的 state；
///   - 标题：本地命名（`title_override`）覆盖客户端标题——列表、搜索、「未命名」
///     判断、归档计数全部走同一份，改名后哪条路都一致；
///   - 置顶位随行带出，`page` 用它做「置顶优先」排序，`keys`/`count` 用它复现筛选。
///
/// `hidden_only = false`：本地隐藏的行整行不出现（默认口径）。
/// `hidden_only = true`：**只**出本地隐藏的行（「已隐藏」chip 的专属视图）。
fn effective_state_sql(hidden_only: bool) -> String {
    format!(
        "SELECT s.kind, s.source_key, s.source_label, s.account, s.session_id,
                COALESCE(NULLIF(m.title_override, ''), s.title) AS title,
                m.title_override AS title_override,
                (m.pinned_at IS NOT NULL) AS pinned,
                s.last_at, s.msgs, s.size, s.has_body, s.slug, s.cwd, s.origin,
                {} AS state
         FROM session_index s
         LEFT JOIN session_marks m
           ON m.source_key = s.source_key AND m.session_id = s.session_id
         WHERE m.hidden_at {} NULL",
        EFFECTIVE_STATE_CASE,
        if hidden_only { "IS NOT" } else { "IS" }
    )
}

const EFFECTIVE_STATE_CASE: &str = "CASE
        WHEN COALESCE(s.state, 'visible') = 'deleted' THEN 'deleted'
        WHEN m.archived_at IS NOT NULL THEN 'archived'
        ELSE COALESCE(s.state, 'visible')
    END";

/// 标签筛选片段：命中任一标签即通过。占位符用位置参数 `?`，
/// 参数顺序与 SQL 文本组装顺序一致（见 `filter_where`）。
fn tags_clause(tags: &[String]) -> String {
    if tags.is_empty() {
        return String::new();
    }
    let marks = vec!["?"; tags.len()].join(", ");
    format!(
        " AND EXISTS (SELECT 1 FROM session_tag tg
              WHERE tg.source_key = x.source_key AND tg.session_id = x.session_id
                AND tg.tag IN ({marks}))"
    )
}

/// trigram 分词器的最小查询长度：少于 3 个字符 trigram 建不出索引，
/// 短查询走 `instr` 匹配兜底（session_text 单会话有 200KB 硬顶，量级可控）。
const TRIGRAM_MIN_CHARS: usize = 3;

/// 正文搜索片段。FTS5 MATCH 的左侧必须是 FTS 表名本身（别名不认）；
/// external-content 表不带业务列，按 rowid 关联 `session_text` 再比对会话标识。
/// 中文要过 trigram 分词才能做子串匹配（默认 unicode61 会把整段 CJK 当成一个
/// token，「部署脚本」永远搜不到）；不足 3 字的查询参数化走 `instr` 兜底。
fn body_clause(raw: Option<&str>) -> String {
    let Some(raw) = raw else {
        return String::new();
    };
    if raw.chars().count() < TRIGRAM_MIN_CHARS {
        return " AND EXISTS (SELECT 1 FROM session_text st
          WHERE st.source_key = x.source_key AND st.session_id = x.session_id
            AND instr(st.body, ?) > 0)"
            .to_string();
    }
    " AND EXISTS (SELECT 1 FROM session_fts
          JOIN session_text st ON st.rowid = session_fts.rowid
          WHERE session_fts MATCH ?
            AND st.source_key = x.source_key AND st.session_id = x.session_id)"
        .to_string()
}

/// `page` / `keys` / `count_with` 三条路共用的 WHERE 与参数。
///
/// 一个逻辑参数可能对应多个 `?` 占位（`? IS NULL OR col = ?` 这种「空值放行」
/// 惯用法每条两个占位），参数必须按**占位个数**复制，顺序与 SQL 文本一致——
/// 三处共用一份，口径不会漂。
///
/// 返回 `(WHERE 片段, 按序参数, 正文短语)`；短语给 `page` 补摘要用。
fn filter_where_parts(filter: &Filter) -> (String, Vec<rusqlite::types::Value>, Option<String>) {
    let source = Filter::clean(&filter.source).map(str::to_string);
    let account = Filter::clean(&filter.account).map(str::to_string);
    let state = Filter::clean(&filter.state).map(str::to_string);
    let q = Filter::clean(&filter.q).map(|s| format!("%{s}%"));
    let origin = Filter::clean(&filter.origin).map(str::to_string);
    // 正文搜索参数：≥3 字走 FTS 短语（加引号壳），短查询走 instr 原文。
    // 两个分支的占位都是一个 `?`，绑定值按分支给。
    let body_raw = Filter::clean(&filter.body).map(str::to_string);
    let body_param = body_raw.as_ref().map(|raw| {
        if raw.chars().count() < TRIGRAM_MIN_CHARS {
            raw.clone()
        } else {
            fts_phrase(raw)
        }
    });
    let body = body_raw;
    let tags = clean_tags(&filter.tags);

    let mut params: Vec<rusqlite::types::Value> = Vec::new();
    // 参数顺序必须与 SQL 文本里 `?` 的出现顺序一致（一个都不能错位——
    // 错一位，MATCH 就会绑到 NULL，报「syntax error near ""」这种没头没尾的错）：
    // source×2 → account×2 → state×2 → q×4 → origin×2 → tags×n → body → pinned
    // → 最后是 WHERE 末尾归档放行子句的 state×1
    push_opt(&mut params, source, 2);
    push_opt(&mut params, account, 2);
    push_opt(&mut params, state.clone(), 2);
    push_opt(&mut params, q, 4);
    push_opt(&mut params, origin, 2);
    for tag in &tags {
        params.push(tag.clone().into());
    }
    // body 只在开启正文搜索时有占位（body_clause 为空时不能多绑一个参数）
    if let Some(param) = &body_param {
        params.push(param.clone().into());
    }
    params.push((filter.pinned as i64).into());
    push_opt(&mut params, state, 1);

    let sql = format!(
        "WHERE (? IS NULL OR x.source_key = ?)
               AND (? IS NULL OR x.account = ?)
               AND (? IS NULL OR x.state = ?)
               AND (? IS NULL OR x.title LIKE ? OR x.source_label LIKE ? OR x.cwd LIKE ?)
               AND (? IS NULL OR x.origin = ?)
               {}
               {}
               AND (? = 0 OR x.pinned = 1)
               AND (? IS NOT NULL OR x.state <> 'archived')
               {}",
        tags_clause(&tags),
        body_clause(body.as_deref()),
        Filter::unnamed_clause(filter.unnamed),
    );
    (sql, params, body)
}

/// 一个逻辑值绑 N 个占位（None → NULL）。
fn push_opt(params: &mut Vec<rusqlite::types::Value>, value: Option<String>, times: usize) {
    let value = match value {
        Some(v) => rusqlite::types::Value::from(v),
        None => rusqlite::types::Value::Null,
    };
    for _ in 0..times {
        params.push(value.clone());
    }
}

/// 计数：与 `page` / `keys` 同一套筛选条件，保证「共 N 条」「全选」与列表对得上。
fn count_with(conn: &rusqlite::Connection, filter: &Filter) -> Result<usize> {
    let (where_sql, params, _) = filter_where_parts(filter);
    let sql = format!(
        "SELECT COUNT(*) FROM ({}) AS x {}",
        effective_state_sql(filter.hidden),
        where_sql,
    );
    let total: i64 = conn.query_row(&sql, rusqlite::params_from_iter(params.iter()), |row| {
        row.get(0)
    })?;
    Ok(total as usize)
}

/// 正文搜索词 → FTS5 短语：整段加双引号当**字面短语**查。
/// 不这么包的话，用户随手输入的 `-`、`(`、`AND` 都会被 FTS 当查询语法，
/// 轻则查不出东西，重则整条查询报错。
fn fts_phrase(raw: &str) -> String {
    format!("\"{}\"", raw.replace('"', "\"\""))
}

/// 标签筛选值清洗：去空白、去空项。
fn clean_tags(tags: &[String]) -> Vec<String> {
    tags.iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

/// 单行的正文命中摘要。查不到（会话没被索引 / 命中的只是其它行）返回 None。
/// FTS 命中用 `snippet()`；短查询（instr 模式）从 body 现算一个窗口。
fn snippet_of(
    conn: &rusqlite::Connection,
    raw: &str,
    source_key: &str,
    session_id: &str,
) -> Option<String> {
    if raw.chars().count() < TRIGRAM_MIN_CHARS {
        let body: String = conn
            .query_row(
                "SELECT body FROM session_text
                 WHERE source_key = ?1 AND session_id = ?2",
                rusqlite::params![source_key, session_id],
                |row| row.get(0),
            )
            .ok()?;
        let at = body.find(raw)?;
        // 窗口是字节偏移：回退 40 字节可能落在多字节字符中间，
        // 两端都要对齐 char boundary（在这里 panic 过一次，见 crash 日志）
        let mut start = (at as i64 - 40).max(0) as usize;
        while start > 0 && !body.is_char_boundary(start) {
            start += 1;
        }
        let mut end = (at + raw.len() + 60).min(body.len());
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        let mut text = String::new();
        if start > 0 {
            text.push('…');
        }
        text.push_str(&body[start..at]);
        text.push('『');
        text.push_str(&body[at..end.max(at)]);
        text.push('』');
        return Some(text);
    }
    conn.query_row(
        "SELECT snippet(session_fts, 0, '『', '』', '…', 16)
         FROM session_fts
         WHERE session_fts MATCH ?1
           AND rowid = (SELECT rowid FROM session_text
                        WHERE source_key = ?2 AND session_id = ?3)",
        rusqlite::params![fts_phrase(raw), source_key, session_id],
        |row| row.get::<_, String>(0),
    )
    .ok()
}

/// FTS 索引进度（facets 用）：indexed = 已有正文的行数，total = 可索引行数
/// （有本地正文的来源；Trae 云端占位这类没有正文的行不算）。
pub(crate) fn fts_status(conn: &rusqlite::Connection) -> Result<FtsStatus> {
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM session_index WHERE has_body = 1",
        [],
        |r| r.get(0),
    )?;
    let indexed: i64 = conn.query_row("SELECT COUNT(*) FROM session_text", [], |r| r.get(0))?;
    Ok(FtsStatus {
        indexed: indexed as usize,
        total: total as usize,
    })
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

/// 游标里必须同时有排序档位、置顶位、排序键与唯一键，否则数据增删会让翻页错位。
/// 旧格式（无档位前缀）按时间档解读，兼容升级前发出的游标。
pub fn encode_cursor(sort: Sort, pinned: bool, value: i64, session_id: &str) -> String {
    B64.encode(format!(
        "{}{}|{value}:{session_id}",
        sort.tag(),
        if pinned { "1" } else { "0" }
    ))
}

pub fn decode_cursor(raw: &str) -> Result<(Sort, bool, i64, String)> {
    let text = B64
        .decode(raw)
        .map_err(|_| AppError::new(ErrorKind::Invalid, "游标格式不对"))?;
    let text =
        String::from_utf8(text).map_err(|_| AppError::new(ErrorKind::Invalid, "游标不是 UTF-8"))?;
    let (tag, rest) = match text.split_once('|') {
        Some((tag, rest)) => (tag, rest),
        // 旧格式：last_at:session_id（无档位、无置顶位）
        None => ("", text.as_str()),
    };
    // 旧格式只有 "t"，v3 起带置顶位："t1" / "t0"
    let (sort, pinned) = match tag.chars().next() {
        Some(c) => {
            let sort = Sort::from_tag(&c.to_string());
            let pinned = tag.chars().nth(1).map(|f| f == '1').unwrap_or(false);
            (sort, pinned)
        }
        None => (Sort::LastAt, false),
    };
    let (value, sid) = rest
        .split_once(':')
        .ok_or_else(|| AppError::new(ErrorKind::Invalid, "游标缺少分隔符"))?;
    let value = value
        .parse::<i64>()
        .map_err(|_| AppError::new(ErrorKind::Invalid, "游标里的排序值不是整数"))?;
    Ok((sort, pinned, value, sid.to_string()))
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

    /// 「未命名」筛选：占位标题与空标题都算，正常标题被滤掉；page / keys / facets
    /// 三条路共用同一份 SQL 条件，口径必须一致。
    #[test]
    fn unnamed_filter_matches_placeholder_and_empty_titles() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let store = SqliteSessions::new(&db);

        let row = |sid: &str, title: &str, last_at: i64| {
            let mut row = RawRow::empty("qoder", "qoder_cn", "Qoder CN");
            row.session_id = sid.into();
            row.title = title.into();
            row.last_at = last_at;
            row
        };
        store
            .replace_source(
                "qoder_cn",
                &[
                    row("s1", "正常标题", 3),
                    row("s2", UNNAMED_TITLE, 2),
                    row("s3", "", 1),
                ],
            )
            .unwrap();

        assert_eq!(
            store
                .page(&Filter::default(), None, 100)
                .unwrap()
                .items
                .len(),
            3
        );

        let unnamed = Filter {
            unnamed: true,
            ..Default::default()
        };
        let page = store.page(&unnamed, None, 100).unwrap();
        let ids = page
            .items
            .iter()
            .map(|row| row.session_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["s2", "s3"]);
        assert_eq!(page.total, 2);
        assert_eq!(store.keys(&unnamed).unwrap().len(), 2);
        assert_eq!(store.facets().unwrap().unnamed, 2);
    }

    /// v3 筛选矩阵：置顶优先排序（含游标跨页）、只看置顶、本地命名参与搜索、
    /// 标签多选、已隐藏视图。page / keys / facets / count 四条路必须同口径。
    #[test]
    fn v3_filters_pinned_rename_tags_hidden() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let store = SqliteSessions::new(&db);

        let row = |sid: &str, title: &str, last_at: i64| {
            let mut row = RawRow::empty("qoder", "qoder_cn", "Qoder CN");
            row.session_id = sid.into();
            row.title = title.into();
            row.last_at = last_at;
            row
        };
        store
            .replace_source(
                "qoder_cn",
                &[
                    row("s1", "alpha", 5),
                    row("s2", "beta", 4),
                    row("s3", "gamma", 3),
                ],
            )
            .unwrap();

        // 置顶最旧的 s3：仍按 last_at 排序，但置顶行跳到最前
        let key_s1 = "qoder|qoder_cn||s1".to_string();
        let key_s2 = "qoder|qoder_cn||s2".to_string();
        let key_s3 = "qoder|qoder_cn||s3".to_string();
        crate::app::sessions::marks::pin(&db, std::slice::from_ref(&key_s3), true).unwrap();
        let ids = |page: &Page<SessionRow>| -> Vec<String> {
            page.items.iter().map(|r| r.session_id.clone()).collect()
        };
        let page = store.page(&Filter::default(), None, 100).unwrap();
        assert_eq!(ids(&page), ["s3", "s1", "s2"]);

        // 游标跨页：从置顶行之后继续，剩下的按 last_at 排
        let cur = encode_cursor(Sort::LastAt, true, 3, "s3");
        let page = store.page(&Filter::default(), Some(&cur), 100).unwrap();
        assert_eq!(ids(&page), ["s1", "s2"]);

        // 只看置顶
        let pinned = Filter {
            pinned: true,
            ..Default::default()
        };
        let page = store.page(&pinned, None, 100).unwrap();
        assert_eq!(ids(&page), ["s3"]);
        assert_eq!(page.total, 1);
        assert_eq!(store.keys(&pinned).unwrap().len(), 1);
        assert_eq!(store.facets().unwrap().pinned, 1);

        // 本地命名：生效标题覆盖、renamed 标记、标题搜索命中新名
        crate::app::sessions::marks::rename(&db, &key_s2, Some("我的会话")).unwrap();
        let page = store.page(&Filter::default(), None, 100).unwrap();
        let s2 = page.items.iter().find(|r| r.session_id == "s2").unwrap();
        assert_eq!(s2.title, "我的会话");
        assert!(s2.renamed);
        let q = Filter {
            q: Some("我的会话".into()),
            ..Default::default()
        };
        let page = store.page(&q, None, 100).unwrap();
        assert_eq!(ids(&page), ["s2"]);
        // 恢复原标题
        crate::app::sessions::marks::rename(&db, &key_s2, None).unwrap();
        let page = store.page(&Filter::default(), None, 100).unwrap();
        let s2 = page.items.iter().find(|r| r.session_id == "s2").unwrap();
        assert_eq!(s2.title, "beta");
        assert!(!s2.renamed);

        // 标签：命中任一标签即通过
        crate::app::sessions::marks::tags_set(
            &db,
            &key_s1,
            &["工作".to_string(), "重要".to_string()],
        )
        .unwrap();
        crate::app::sessions::marks::tags_set(&db, &key_s2, &["重要".to_string()]).unwrap();
        let work = Filter {
            tags: vec!["工作".into()],
            ..Default::default()
        };
        let page = store.page(&work, None, 100).unwrap();
        assert_eq!(ids(&page), ["s1"]);
        let both = Filter {
            tags: vec!["工作".into(), "重要".into()],
            ..Default::default()
        };
        assert_eq!(store.keys(&both).unwrap().len(), 2);
        let facets = store.facets().unwrap();
        assert_eq!(facets.tags.len(), 2);

        // 已隐藏视图：默认页不含隐藏行，「已隐藏」chip 只出隐藏行
        crate::app::sessions::marks::hide(&db, std::slice::from_ref(&key_s1)).unwrap();
        let page = store.page(&Filter::default(), None, 100).unwrap();
        assert_eq!(ids(&page), ["s3", "s2"], "隐藏行从默认列表消失");
        let hidden = Filter {
            hidden: true,
            ..Default::default()
        };
        let page = store.page(&hidden, None, 100).unwrap();
        assert_eq!(ids(&page), ["s1"]);
        assert_eq!(page.total, 1);
        assert_eq!(store.facets().unwrap().hidden, 1);
    }

    /// 正文搜索：FTS 命中返回行 + 摘要；没索引的正文查不到；keys 与 page 同口径。
    #[test]
    fn body_search_hits_fts_with_snippet() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let store = SqliteSessions::new(&db);
        let mut row = RawRow::empty("qoder", "qoder_cn", "Qoder CN");
        row.session_id = "s1".into();
        row.title = "alpha".into();
        row.last_at = 5;
        row.has_body = true;
        store.replace_source("qoder_cn", &[row]).unwrap();

        // 直接写 session_text 并同步 FTS（与 fts.rs 的写入纪律一致）
        let body = "讨论了部署脚本的幂等性问题";
        db.write(|conn| {
            conn.execute(
                "INSERT INTO session_text(source_key, session_id, fp, truncated, body)
                 VALUES ('qoder_cn', 's1', '5:1', 0, ?1)",
                rusqlite::params![body],
            )?;
            let rowid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO session_fts(rowid, body) VALUES (?1, ?2)",
                rusqlite::params![rowid, body],
            )?;
            Ok(())
        })
        .unwrap();

        let filter = Filter {
            body: Some("部署脚本".into()),
            ..Default::default()
        };
        let page = store.page(&filter, None, 100).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].session_id, "s1");
        let snippet = page.items[0].snippet.as_deref().unwrap_or_default();
        assert!(snippet.contains("部署脚本"), "摘要应含命中词：{snippet}");
        assert_eq!(store.keys(&filter).unwrap().len(), 1);
        assert_eq!(page.total, 1);

        // 没写的词查不到
        let miss = Filter {
            body: Some("不存在的词组".into()),
            ..Default::default()
        };
        assert_eq!(store.page(&miss, None, 100).unwrap().items.len(), 0);
        // 索引进度口径：has_body=1 的行
        assert_eq!(store.facets().unwrap().fts.indexed, 1);
    }

    /// 短查询（instr 模式）的摘要窗口：回退 40 **字节**可能落在 CJK 字符中间，
    /// 必须对齐 char boundary——这里真实 panic 过一次（crash 日志 store.rs:862）。
    #[test]
    fn short_query_snippet_respects_char_boundaries() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let store = SqliteSessions::new(&db);
        let mut row = RawRow::empty("qoder", "qoder_cn", "Qoder CN");
        row.session_id = "s1".into();
        row.title = "t".into();
        row.last_at = 5;
        row.has_body = true;
        store.replace_source("qoder_cn", &[row]).unwrap();
        // 命中词前面铺一串三字节字符：回退窗口必然切在字符中间
        let body = format!("{}登录成功", "那".repeat(100));
        db.write(|conn| {
            conn.execute(
                "INSERT INTO session_text(source_key, session_id, fp, truncated, body)
                 VALUES ('qoder_cn', 's1', '1:1', 0, ?1)",
                rusqlite::params![body],
            )?;
            Ok(())
        })
        .unwrap();

        let filter = Filter {
            body: Some("登录".into()),
            ..Default::default()
        };
        let page = store.page(&filter, None, 100).unwrap();
        assert_eq!(page.items.len(), 1);
        let snippet = page.items[0].snippet.as_deref().unwrap_or_default();
        // 高亮窗向后扩 60 字节，可能把命中词所在的整句包进来
        assert!(
            snippet.contains('『') && snippet.contains("登录"),
            "摘要应含高亮：{snippet}"
        );
        assert!(snippet.starts_with('…'), "命中位置远超窗口，前面应是省略号");
    }
}
