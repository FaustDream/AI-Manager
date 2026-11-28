//! Global media metadata DAL. Source identities, ratings and individual releases are facts;
//! a missing item in a later sync is not a deletion or a change in watching state.

use super::types::*;
use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use rusqlite::{params, params_from_iter, types::Value, Connection, OptionalExtension, Row};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const LEGACY_MARKER: &str = "catalog.media.legacy.v1";
const DEDUPE_MARKER: &str = "catalog.dedupe.v1";
const DETAIL_RELEASE_LIMIT: i64 = 500;

pub(crate) fn bootstrap(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS kv (
            key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS media (
            key TEXT PRIMARY KEY, title TEXT NOT NULL, kind TEXT NOT NULL, status TEXT NOT NULL,
            year INTEGER, premiered TEXT, rating REAL, rating_source TEXT, source TEXT NOT NULL,
            first_seen TEXT NOT NULL, updated_at TEXT NOT NULL, search_text TEXT NOT NULL,
            payload TEXT NOT NULL, authority_json TEXT NOT NULL DEFAULT '{}'
        );
        CREATE INDEX IF NOT EXISTS idx_media_kind_year ON media(kind, year);
        CREATE INDEX IF NOT EXISTS idx_media_status ON media(status);
        CREATE INDEX IF NOT EXISTS idx_media_year ON media(year DESC, key);
        CREATE INDEX IF NOT EXISTS idx_media_rating ON media(rating DESC, key);
        CREATE INDEX IF NOT EXISTS idx_media_title ON media(title COLLATE NOCASE, key);
        CREATE INDEX IF NOT EXISTS idx_media_first_seen ON media(first_seen DESC, key);
        CREATE INDEX IF NOT EXISTS idx_media_updated ON media(updated_at DESC, key);
        CREATE INDEX IF NOT EXISTS idx_media_premiered ON media(premiered, key);
        CREATE TABLE IF NOT EXISTS media_aliases (
            media_key TEXT NOT NULL REFERENCES media(key), alias TEXT NOT NULL,
            PRIMARY KEY(media_key, alias)
        );
        CREATE INDEX IF NOT EXISTS idx_media_alias ON media_aliases(alias, media_key);
        CREATE TABLE IF NOT EXISTS media_external_ids (
            source TEXT NOT NULL, external_id TEXT NOT NULL,
            media_key TEXT NOT NULL REFERENCES media(key), PRIMARY KEY(source, external_id)
        );
        CREATE INDEX IF NOT EXISTS idx_media_external_media ON media_external_ids(media_key);
        CREATE TABLE IF NOT EXISTS media_genres (
            media_key TEXT NOT NULL REFERENCES media(key), genre TEXT NOT NULL,
            PRIMARY KEY(media_key, genre)
        );
        CREATE INDEX IF NOT EXISTS idx_media_genre ON media_genres(genre, media_key);
        CREATE TABLE IF NOT EXISTS media_regions (
            media_key TEXT NOT NULL REFERENCES media(key), region TEXT NOT NULL,
            PRIMARY KEY(media_key, region)
        );
        CREATE INDEX IF NOT EXISTS idx_media_region ON media_regions(region, media_key);
        CREATE TABLE IF NOT EXISTS media_platforms (
            media_key TEXT NOT NULL REFERENCES media(key), platform TEXT NOT NULL,
            PRIMARY KEY(media_key, platform)
        );
        CREATE INDEX IF NOT EXISTS idx_media_platform ON media_platforms(platform, media_key);
        CREATE TABLE IF NOT EXISTS media_people (
            media_key TEXT NOT NULL REFERENCES media(key), name TEXT NOT NULL, role TEXT NOT NULL,
            PRIMARY KEY(media_key, name, role)
        );
        CREATE INDEX IF NOT EXISTS idx_media_people_name ON media_people(name, media_key);
        CREATE TABLE IF NOT EXISTS media_source_info (
            media_key TEXT NOT NULL REFERENCES media(key), source TEXT NOT NULL,
            title TEXT, url TEXT, episode_text TEXT, updated_at TEXT NOT NULL,
            PRIMARY KEY(media_key, source)
        );
        CREATE INDEX IF NOT EXISTS idx_media_source_info_src ON media_source_info(source, media_key);
        CREATE TABLE IF NOT EXISTS media_follows (
            media_key TEXT PRIMARY KEY REFERENCES media(key),
            state TEXT NOT NULL CHECK(state IN ('want','watching','shelve','done')),
            created_at TEXT NOT NULL, updated_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_media_follows_state ON media_follows(state, updated_at DESC);
        CREATE TABLE IF NOT EXISTS media_dupe_candidates (
            key_a TEXT NOT NULL, key_b TEXT NOT NULL, reason TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'open', created_at TEXT NOT NULL,
            PRIMARY KEY(key_a, key_b)
        );
        CREATE INDEX IF NOT EXISTS idx_dupe_status ON media_dupe_candidates(status, key_a);
        CREATE INDEX IF NOT EXISTS idx_dupe_status_b ON media_dupe_candidates(status, key_b);
        CREATE TABLE IF NOT EXISTS media_merge_archive (
            merged_key TEXT PRIMARY KEY, into_key TEXT NOT NULL,
            payload TEXT NOT NULL, merged_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS seasons (
            media_key TEXT NOT NULL REFERENCES media(key), source TEXT NOT NULL,
            external_id TEXT NOT NULL, number INTEGER, title TEXT, episode_count INTEGER,
            premiered TEXT, ended TEXT, payload TEXT NOT NULL, updated_at TEXT NOT NULL,
            PRIMARY KEY(media_key, source, external_id)
        );
        CREATE INDEX IF NOT EXISTS idx_seasons_media ON seasons(media_key, number);
        CREATE TABLE IF NOT EXISTS episodes (
            source TEXT NOT NULL, external_id TEXT NOT NULL,
            media_key TEXT NOT NULL REFERENCES media(key), season INTEGER, number TEXT,
            title TEXT, PRIMARY KEY(source, external_id)
        );
        CREATE INDEX IF NOT EXISTS idx_episodes_media ON episodes(media_key, season, number);
        CREATE TABLE IF NOT EXISTS manga_chapters (
            source TEXT NOT NULL, external_id TEXT NOT NULL,
            media_key TEXT NOT NULL REFERENCES media(key), number TEXT, title TEXT,
            PRIMARY KEY(source, external_id)
        );
        CREATE INDEX IF NOT EXISTS idx_chapters_media ON manga_chapters(media_key, number);
        CREATE TABLE IF NOT EXISTS release_schedules (
            key TEXT PRIMARY KEY, media_key TEXT NOT NULL REFERENCES media(key),
            source TEXT NOT NULL, external_id TEXT NOT NULL, date TEXT NOT NULL,
            local_time TEXT, timezone TEXT, utc_time TEXT, season INTEGER, episode TEXT,
            title TEXT, platform TEXT, release_kind TEXT NOT NULL, url TEXT,
            updated_at TEXT NOT NULL, payload TEXT NOT NULL, UNIQUE(source, external_id)
        );
        CREATE INDEX IF NOT EXISTS idx_releases_date ON release_schedules(date, local_time, key);
        CREATE INDEX IF NOT EXISTS idx_releases_media ON release_schedules(media_key, date DESC);
        CREATE INDEX IF NOT EXISTS idx_releases_platform ON release_schedules(platform, date);
        CREATE TABLE IF NOT EXISTS rating_sources (
            source TEXT PRIMARY KEY, label TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS media_ratings (
            media_key TEXT NOT NULL REFERENCES media(key),
            source TEXT NOT NULL REFERENCES rating_sources(source), category TEXT NOT NULL,
            value REAL, scale REAL NOT NULL, votes INTEGER, reviews INTEGER,
            positive_percent REAL, rank INTEGER, url TEXT, updated_at TEXT NOT NULL,
            payload TEXT NOT NULL, PRIMARY KEY(media_key, source, category)
        );
        CREATE INDEX IF NOT EXISTS idx_ratings_source ON media_ratings(source, category, media_key);
        CREATE TABLE IF NOT EXISTS media_reviews (
            media_key TEXT NOT NULL, source TEXT NOT NULL, category TEXT NOT NULL,
            reviews INTEGER, positive_percent REAL, votes INTEGER, rank INTEGER,
            url TEXT, updated_at TEXT NOT NULL,
            PRIMARY KEY(media_key, source, category),
            FOREIGN KEY(media_key, source, category) REFERENCES media_ratings(media_key, source, category)
        );
        CREATE TABLE IF NOT EXISTS data_sources (
            source TEXT PRIMARY KEY, enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)),
            key_blob BLOB, state_json TEXT, updated_at INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS ingestion_jobs (
            id TEXT PRIMARY KEY, source TEXT NOT NULL REFERENCES data_sources(source),
            started_at TEXT NOT NULL, finished_at TEXT, status TEXT NOT NULL,
            pages INTEGER NOT NULL, payload TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_jobs_recent ON ingestion_jobs(started_at DESC, id DESC);
        CREATE TABLE IF NOT EXISTS ingestion_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id TEXT NOT NULL REFERENCES ingestion_jobs(id),
            source TEXT NOT NULL REFERENCES data_sources(source), page INTEGER NOT NULL,
            status TEXT NOT NULL, http_status INTEGER, failed INTEGER NOT NULL,
            error TEXT, warning TEXT, logged_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_ingestion_logs_job ON ingestion_logs(job_id, id);
        CREATE TABLE IF NOT EXISTS catalog_meta (
            key TEXT PRIMARY KEY, value TEXT NOT NULL
        );",
    )?;
    // norm_title 列：归一化标题，跨源合并的匹配键。catalog bootstrap 每次开库都跑、
    // 无版本护栏，加列必须先查 PRAGMA 再 ALTER（幂等重跑）。
    let has_norm_title: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('media') WHERE name='norm_title')",
        [],
        |r| r.get(0),
    )?;
    if !has_norm_title {
        tx.execute("ALTER TABLE media ADD COLUMN norm_title TEXT", [])?;
    }
    tx.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_media_norm_title ON media(kind, norm_title);
         CREATE INDEX IF NOT EXISTS idx_media_norm ON media(norm_title);",
    )?;
    migrate_legacy(&tx)?;
    tx.commit()?;
    Ok(())
}

/// 标题归一化：全角→半角、小写、去空白与标点，只留字母数字与 CJK。
/// 跨源合并的匹配键——「斗罗大陆」「斗罗大陆 第一季」「Ｄｕｂｅｄ」都要能对上。
pub(crate) fn normalize_title(value: &str) -> String {
    value
        .chars()
        .filter_map(|c| {
            // 全角 ASCII 区（！～）平移回半角
            let c = if (c as u32) >= 0xFF01 && (c as u32) <= 0xFF5E {
                char::from_u32(c as u32 - 0xFEE0).unwrap_or(c)
            } else {
                c
            };
            if c.is_alphanumeric() {
                c.to_lowercase().next()
            } else {
                None
            }
        })
        .collect()
}

fn migrate_legacy(conn: &Connection) -> Result<()> {
    let done: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM catalog_meta WHERE key=?1)",
        [LEGACY_MARKER],
        |r| r.get(0),
    )?;
    if done {
        return Ok(());
    }
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='catalog_items')",
        [],
        |r| r.get(0),
    )?;
    if exists {
        let mut stmt = conn.prepare(
            "SELECT key, platform, kind, title, update_note, year, air_date, finished, url,
                    first_seen, updated_at FROM catalog_items ORDER BY first_seen, key",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let old_key: String = row.get(0)?;
            let source: String = row.get(1)?;
            let external_id = old_key
                .strip_prefix(&format!("{source}:"))
                .unwrap_or(&old_key)
                .to_string();
            let date: Option<String> = row.get(6)?;
            let first_seen: String = row.get(9)?;
            let updated_at: String = row.get(10)?;
            let mut draft = MediaDraft {
                source: source.clone(),
                external_id,
                media: CatalogMedia {
                    title: row.get(3)?,
                    kind: row.get(2)?,
                    year: row.get(5)?,
                    status: if row.get::<_, bool>(7)? {
                        "completed"
                    } else {
                        "unknown"
                    }
                    .into(),
                    platforms: vec![source.clone()],
                    source_url: row.get(8)?,
                    premiered: if matches!(source.as_str(), "tencent" | "iqiyi") {
                        date.clone()
                    } else {
                        None
                    },
                    first_seen,
                    updated_at: updated_at.clone(),
                    ..Default::default()
                },
                ..Default::default()
            };
            // Only Bilibili's timeline was an observed release. Its old show-level row
            // cannot recover an episode ID, so label it as legacy rather than invent one.
            if source == "bilibili" {
                if let Some(date) = date.filter(|date| valid_date(date)) {
                    draft.releases.push(CatalogRelease {
                        source,
                        external_id: format!("legacy:{old_key}"),
                        date,
                        title: row.get(4)?,
                        release_kind: "legacy".into(),
                        url: draft.media.source_url.clone(),
                        updated_at: updated_at.clone(),
                        ..Default::default()
                    });
                }
            }
            ingest(conn, &draft, &updated_at)?;
            let imported_key: String = conn.query_row(
                "SELECT media_key FROM media_external_ids WHERE source=?1 AND external_id=?2",
                params![draft.source, draft.external_id],
                |r| r.get(0),
            )?;
            let (mut media, authorities) = conn.query_row(
                "SELECT payload,authority_json FROM media WHERE key=?1",
                [&imported_key],
                |r| {
                    Ok((
                        json_row::<CatalogMedia>(r, 0)?,
                        json_row::<Authorities>(r, 1)?,
                    ))
                },
            )?;
            // Migration must not make an old item look newly collected or updated.
            if draft.media.first_seen < media.first_seen {
                media.first_seen.clone_from(&draft.media.first_seen);
            }
            if updated_at < media.updated_at {
                media.updated_at.clone_from(&updated_at);
            }
            save_media(conn, &media, &authorities)?;
        }
    }
    // 标记与导入事实同事务提交；原表永不改动。标记放 catalog_meta，不污染通用 kv。
    conn.execute(
        "INSERT INTO catalog_meta(key,value) VALUES (?1,'complete') ON CONFLICT(key) DO NOTHING",
        [LEGACY_MARKER],
    )?;
    Ok(())
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Authority {
    source: String,
    stamp: String,
}
type Authorities = BTreeMap<String, Authority>;

fn invalid(message: &str) -> AppError {
    AppError::new(ErrorKind::Invalid, message)
}
fn conflict() -> AppError {
    AppError::new(
        ErrorKind::Conflict,
        "媒体外部标识指向不同条目，请核对来源映射",
    )
}
pub(crate) fn valid_date(date: &str) -> bool {
    date.len() == 10 && chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok()
}
fn identity(source: &str, id: &str) -> Result<String> {
    if source.trim().is_empty() || source.contains(':') || id.trim().is_empty() {
        return Err(invalid("媒体来源与外部标识不能为空，来源不能含冒号"));
    }
    Ok(format!("{source}:{id}"))
}
fn json_row<T: DeserializeOwned>(row: &Row<'_>, column: usize) -> rusqlite::Result<T> {
    let raw: String = row.get(column)?;
    serde_json::from_str(&raw).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, Box::new(e))
    })
}
fn union(target: &mut Vec<String>, incoming: &[String]) {
    target.extend(incoming.iter().filter(|s| !s.trim().is_empty()).cloned());
    target.retain(|s| !s.trim().is_empty());
    target.sort();
    target.dedup();
}
fn option_text(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}
fn can_apply(authorities: &Authorities, field: &str, source: &str, stamp: &str) -> bool {
    authorities
        .get(field)
        .is_none_or(|a| stamp > a.stamp.as_str() || (stamp == a.stamp && source == a.source))
}
fn mark(authorities: &mut Authorities, field: &str, source: &str, stamp: &str) {
    authorities.insert(
        field.into(),
        Authority {
            source: source.into(),
            stamp: stamp.into(),
        },
    );
}
fn kind_rank(kind: &str) -> u8 {
    match kind {
        "anime" | "manga" | "movie" | "variety" | "documentary" => 2,
        "tv" | "other" => 1,
        _ => 0,
    }
}
fn cjk_count(value: &str) -> usize {
    value
        .chars()
        .filter(|c| {
            matches!(
                c,
                '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}'
            )
        })
        .count()
}
/// 多源合并后选主标题：中文优先，短名优先，避免运营长文案或后来的英文名盖掉中文名。
pub(crate) fn choose_display_title(current: &str, aliases: &[String]) -> String {
    let mut candidates: Vec<&str> = std::iter::once(current)
        .chain(aliases.iter().map(String::as_str))
        .filter(|name| cjk_count(name) >= 2)
        .collect();
    candidates.sort_by(|a, b| {
        a.chars()
            .count()
            .cmp(&b.chars().count())
            .then_with(|| cjk_count(b).cmp(&cjk_count(a)))
            .then_with(|| a.cmp(b))
    });
    candidates
        .first()
        .map(|name| (*name).to_string())
        .unwrap_or_else(|| current.to_string())
}
fn merge_media(
    old: &CatalogMedia,
    incoming: &CatalogMedia,
    source: &str,
    stamp: &str,
    authorities: &mut Authorities,
) -> CatalogMedia {
    let mut media = old.clone();
    macro_rules! text {
        ($($field:ident),+ $(,)?) => { $(
            if !incoming.$field.trim().is_empty() && (media.$field.trim().is_empty()
                || can_apply(authorities, stringify!($field), source, stamp)) {
                media.$field.clone_from(&incoming.$field);
                mark(authorities, stringify!($field), source, stamp);
            }
        )+ };
    }
    macro_rules! optional {
        ($($field:ident),+ $(,)?) => { $(
            if incoming.$field.is_some() && (media.$field.is_none()
                || can_apply(authorities, stringify!($field), source, stamp)) {
                media.$field.clone_from(&incoming.$field);
                mark(authorities, stringify!($field), source, stamp);
            }
        )+ };
    }
    // Keep alternate titles searchable even when a newer authoritative name wins.
    if !old.title.trim().is_empty()
        && !incoming.title.trim().is_empty()
        && incoming.title != old.title
    {
        union(&mut media.aliases, std::slice::from_ref(&incoming.title));
        union(&mut media.aliases, std::slice::from_ref(&old.title));
    }
    for title in [
        &old.original_title,
        &old.english_title,
        &incoming.original_title,
        &incoming.english_title,
    ]
    .into_iter()
    .flatten()
    {
        union(&mut media.aliases, std::slice::from_ref(title));
    }
    text!(title);
    // 具体类型（动漫/漫画/电影/综艺/纪录片）不被通用 tv/other 降级：
    // 跨源合并时 SIMKL 的泛 tv 不应把 TVmaze 已判定的综艺盖掉。
    let old_rank = kind_rank(&media.kind);
    let new_rank = kind_rank(&incoming.kind);
    if new_rank > 0
        && (old_rank == 0 || new_rank > old_rank
            || (new_rank == old_rank && can_apply(authorities, "kind", source, stamp)))
    {
        media.kind.clone_from(&incoming.kind);
        mark(authorities, "kind", source, stamp);
    }
    if !incoming.status.trim().is_empty()
        && (media.status.is_empty()
            || media.status == "unknown"
            || (incoming.status != "unknown" && can_apply(authorities, "status", source, stamp)))
    {
        media.status.clone_from(&incoming.status);
        mark(authorities, "status", source, stamp);
    }
    optional!(
        original_title,
        english_title,
        subkind,
        poster,
        backdrop,
        summary,
        year,
        premiered,
        ended,
        season_count,
        episode_count,
        aired_episodes,
        runtime,
        chapter_count,
        latest_chapter
    );
    // A contributing provider URL is not the originating provider's URL.
    if source == old.source {
        optional!(source_url);
    }
    macro_rules! lists {
        ($($field:ident),+ $(,)?) => { $(union(&mut media.$field, &incoming.$field);)+ };
    }
    lists!(
        aliases,
        regions,
        genres,
        platforms,
        directors,
        writers,
        cast,
        studios,
        distributors
    );
    media.title = choose_display_title(&media.title, &media.aliases);
    media
}
fn clean_media(mut media: CatalogMedia) -> CatalogMedia {
    macro_rules! clean {
        ($($field:ident),+ $(,)?) => { $(media.$field = option_text(media.$field);)+ };
    }
    clean!(
        original_title,
        english_title,
        subkind,
        poster,
        backdrop,
        summary,
        premiered,
        ended,
        latest_chapter,
        source_url
    );
    // 类型在入库前统一成中文：国际源的英文类型与国内源的中文类型落到同一列，
    // 筛选项、列表、详情不会出现「Action / 动作」两套写法。已入库的旧英文值
    // 由 facets 的 genre_label 兜底翻译，不改写历史行。
    let mut localized = Vec::with_capacity(media.genres.len());
    for genre in std::mem::take(&mut media.genres) {
        let value = super::genres::localize_genre(&genre).unwrap_or(genre.as_str());
        if !localized.iter().any(|existing: &String| existing == value) {
            localized.push(value.to_string());
        }
    }
    media.genres = localized;
    media
}
fn search_text(media: &CatalogMedia) -> String {
    let mut parts = vec![media.title.clone()];
    parts.extend(media.original_title.iter().cloned());
    parts.extend(media.english_title.iter().cloned());
    for values in [
        &media.aliases,
        &media.directors,
        &media.writers,
        &media.cast,
        &media.studios,
        &media.distributors,
    ] {
        parts.extend(values.iter().cloned());
    }
    parts.join("\n")
}
fn save_media(conn: &Connection, media: &CatalogMedia, authorities: &Authorities) -> Result<()> {
    // The JSON display value stays on its original scale. Only the indexed SQL
    // value is normalized to /10 for filtering and ordering.
    let filter_rating: Option<f64> = conn
        .query_row(
            "SELECT value*10.0/scale FROM media_ratings WHERE media_key=?1 AND source=?2
            AND lower(category) IN ('user','audience','users','audience_score','user_score')
            AND value IS NOT NULL AND scale>0
         ORDER BY CASE WHEN lower(category)='user' THEN 0 ELSE 1 END,category LIMIT 1",
            params![media.key, media.rating_source],
            |r| r.get(0),
        )
        .optional()?;
    conn.execute(
        "INSERT INTO media(key,title,kind,status,year,premiered,rating,rating_source,source,
                           first_seen,updated_at,search_text,payload,authority_json,norm_title)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
         ON CONFLICT(key) DO UPDATE SET title=excluded.title,kind=excluded.kind,status=excluded.status,
             year=excluded.year,premiered=excluded.premiered,rating=excluded.rating,
             rating_source=excluded.rating_source,first_seen=excluded.first_seen,updated_at=excluded.updated_at,
             search_text=excluded.search_text,payload=excluded.payload,authority_json=excluded.authority_json,
             norm_title=excluded.norm_title",
        params![media.key, media.title, media.kind, media.status, media.year, media.premiered,
            filter_rating, media.rating_source, media.source, media.first_seen, media.updated_at,
            search_text(media), serde_json::to_string(media)?, serde_json::to_string(authorities)?,
            normalize_title(&media.title)],
    )?;
    for (sql, values) in [
        (
            "INSERT INTO media_aliases(media_key,alias) VALUES (?1,?2) ON CONFLICT DO NOTHING",
            &media.aliases,
        ),
        (
            "INSERT INTO media_genres(media_key,genre) VALUES (?1,?2) ON CONFLICT DO NOTHING",
            &media.genres,
        ),
        (
            "INSERT INTO media_regions(media_key,region) VALUES (?1,?2) ON CONFLICT DO NOTHING",
            &media.regions,
        ),
        (
            "INSERT INTO media_platforms(media_key,platform) VALUES (?1,?2) ON CONFLICT DO NOTHING",
            &media.platforms,
        ),
    ] {
        let mut stmt = conn.prepare_cached(sql)?;
        for value in values {
            stmt.execute(params![media.key, value])?;
        }
    }
    // 演员筛选走独立表（media_people），全量重建：payload 变更后人名集合跟着变，
    // 增量diff 的复杂度大于一次 DELETE+INSERT。
    conn.execute("DELETE FROM media_people WHERE media_key=?1", params![media.key])?;
    let mut stmt = conn.prepare_cached(
        "INSERT INTO media_people(media_key,name,role) VALUES (?1,?2,?3) ON CONFLICT DO NOTHING",
    )?;
    for (names, role) in [
        (&media.cast, "cast"),
        (&media.directors, "director"),
        (&media.writers, "writer"),
    ] {
        for name in names.iter().filter(|n| !n.trim().is_empty()) {
            stmt.execute(params![media.key, name.trim(), role])?;
        }
    }
    Ok(())
}

/// 每源落地信息文案：把该源草稿的集数口径拼成一句人话。
fn episode_text_of(media: &CatalogMedia) -> Option<String> {
    let total = media.episode_count;
    let aired = media.aired_episodes;
    let text = match (total, aired) {
        (Some(total), Some(aired)) => Some(format!("共 {total} 集 · 已发行 {aired} 集")),
        (Some(total), None) => Some(format!("共 {total} 集")),
        (None, Some(aired)) => Some(format!("已发行 {aired} 集")),
        (None, None) => media.latest_chapter.as_ref().map(|c| format!("最新章节 {c}")),
    };
    text.filter(|s| !s.trim().is_empty())
}

pub fn upsert(db: &Db, drafts: &[MediaDraft], now: &str) -> Result<IngestionCounts> {
    db.write(|conn| {
        let tx = conn.unchecked_transaction()?;
        let mut counts = IngestionCounts::default();
        for draft in drafts {
            let result = ingest(&tx, draft, now)?;
            counts.added += result.added;
            counts.updated += result.updated;
            counts.deduplicated += result.deduplicated;
            counts.releases += result.releases;
        }
        tx.commit()?;
        Ok(counts)
    })
}

/// 跨源二级匹配的结论：Strong=归一化标题+类型+年份全部一致（自动合并）；
/// Suspect=标题一致但年份缺失或冲突（记入疑似候选，不自动合并）；NoMatch=没有可比标题。
enum IdentityMatch {
    Strong(String),
    Suspect,
    NoMatch,
}

fn find_identity_match(
    conn: &Connection,
    incoming: &CatalogMedia,
    own_key: &str,
    now: &str,
) -> Result<IdentityMatch> {
    let norm = normalize_title(&incoming.title);
    if norm.chars().count() < 2 {
        return Ok(IdentityMatch::NoMatch);
    }
    let mut stmt = conn.prepare_cached(
        "SELECT key,year FROM media WHERE kind=?1 AND norm_title=?2 AND key<>?3 LIMIT 4",
    )?;
    let rows = stmt
        .query_map(params![incoming.kind, norm, own_key], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<i32>>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.is_empty() {
        return Ok(IdentityMatch::NoMatch);
    }
    // 强证据：同类型 + 同归一化标题 + 同年份。先到先得；同组多行属于历史遗留，
    // 由 dedupe_scan 收敛，这里取第一行。
    if let Some(year) = incoming.year {
        if let Some((key, _)) = rows.iter().find(|(_, existing)| *existing == Some(year)) {
            return Ok(IdentityMatch::Strong(key.clone()));
        }
        record_dupe_candidates(conn, &rows, own_key, "year_conflict", now)?;
    } else {
        record_dupe_candidates(conn, &rows, own_key, "year_missing", now)?;
    }
    Ok(IdentityMatch::Suspect)
}

fn record_dupe_candidates(
    conn: &Connection,
    rows: &[(String, Option<i32>)],
    own_key: &str,
    reason: &str,
    now: &str,
) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        "INSERT INTO media_dupe_candidates(key_a,key_b,reason,status,created_at)
         VALUES (?1,?2,?3,'open',?4) ON CONFLICT(key_a,key_b) DO NOTHING",
    )?;
    for (key, _) in rows {
        let (a, b) = if key.as_str() < own_key { (key.as_str(), own_key) } else { (own_key, key.as_str()) };
        stmt.execute(params![a, b, reason, now])?;
    }
    Ok(())
}

fn ingest(conn: &Connection, draft: &MediaDraft, now: &str) -> Result<IngestionCounts> {
    let own_key = identity(&draft.source, &draft.external_id)?;
    let mut ids: BTreeSet<(String, String)> = draft
        .external_ids
        .iter()
        .map(|id| (id.source.clone(), id.external_id.clone()))
        .collect();
    ids.insert((draft.source.clone(), draft.external_id.clone()));
    let mut resolved = BTreeSet::new();
    for (source, id) in &ids {
        identity(source, id)?;
        if let Some(key) = conn
            .query_row(
                "SELECT media_key FROM media_external_ids WHERE source=?1 AND external_id=?2",
                params![source, id],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            resolved.insert(key);
        }
    }
    // Disagreeing trusted identifiers are a source-data conflict, not permission to
    // destroy an existing season/version. Reject the entire batch for diagnosis.
    if resolved.len() > 1 {
        return Err(conflict());
    }
    let incoming = clean_media(draft.media.clone());
    // 外部 ID 没有交集时，用「归一化标题+类型+年份」做二级匹配：同一作品换了个
    // 来源采集也并进同一行；只有标题对得上的记疑似候选，不自动合并。
    let key = match resolved.into_iter().next() {
        Some(key) => key,
        None => match find_identity_match(conn, &incoming, &own_key, now)? {
            IdentityMatch::Strong(key) => key,
            IdentityMatch::Suspect | IdentityMatch::NoMatch => own_key,
        },
    };
    let existing = conn
        .query_row(
            "SELECT payload,authority_json FROM media WHERE key=?1",
            [&key],
            |row| {
                Ok((
                    json_row::<CatalogMedia>(row, 0)?,
                    json_row::<Authorities>(row, 1)?,
                ))
            },
        )
        .optional()?;
    let is_new = existing.is_none();
    let stamp = if incoming.updated_at.is_empty() {
        now
    } else {
        &incoming.updated_at
    };
    let (old, mut authorities) = existing.unwrap_or_else(|| {
        (
            CatalogMedia {
                key: key.clone(),
                source: draft.source.clone(),
                first_seen: if incoming.first_seen.is_empty() {
                    now.into()
                } else {
                    incoming.first_seen.clone()
                },
                updated_at: stamp.into(),
                ..Default::default()
            },
            Authorities::default(),
        )
    });
    let mut media = merge_media(&old, &incoming, &draft.source, stamp, &mut authorities);
    if media.kind.is_empty() {
        media.kind = "other".into();
    }
    if media.status.is_empty() {
        media.status = "unknown".into();
    }
    if !incoming.first_seen.is_empty()
        && (media.first_seen.is_empty() || incoming.first_seen < media.first_seen)
    {
        media.first_seen.clone_from(&incoming.first_seen);
    }
    if is_new {
        save_media(conn, &media, &authorities)?;
    }
    let mut related_changed = false;
    for (source, id) in ids {
        related_changed |= conn.execute(
            "INSERT INTO media_external_ids(source,external_id,media_key) VALUES (?1,?2,?3)
             ON CONFLICT(source,external_id) DO NOTHING",
            params![source, id, key],
        )? > 0;
    }
    // 各源落地信息：合并后仍逐源保留（详情「来源信息」按源展示，谁采集的写谁）。
    conn.execute(
        "INSERT INTO media_source_info(media_key,source,title,url,episode_text,updated_at)
         VALUES (?1,?2,?3,?4,?5,?6)
         ON CONFLICT(media_key,source) DO UPDATE SET title=excluded.title,url=excluded.url,
             episode_text=excluded.episode_text,updated_at=excluded.updated_at",
        params![
            key,
            draft.source,
            option_text(Some(draft.media.title.clone())),
            option_text(draft.media.source_url.clone()),
            episode_text_of(&draft.media),
            stamp
        ],
    )?;
    for rating in &draft.ratings {
        related_changed |= save_rating(conn, &key, rating, stamp)?;
    }
    for season in &draft.seasons {
        related_changed |= save_season(conn, &key, &draft.source, season, stamp)?;
    }
    let mut release_count = 0;
    for release in &draft.releases {
        if save_release(conn, &key, &draft.source, release, stamp)? {
            release_count += 1;
        }
    }
    related_changed |= release_count > 0;
    choose_rating(conn, &mut media)?;
    let changed = media != old || related_changed;
    if is_new || changed {
        if !is_new && stamp > media.updated_at.as_str() {
            media.updated_at = stamp.into();
        }
        save_media(conn, &media, &authorities)?;
    } else {
        // Freshness is not a semantic update, but must still prevent a subsequent
        // stale response from replacing dates/status after an unchanged sync.
        conn.execute(
            "UPDATE media SET authority_json=?2 WHERE key=?1",
            params![key, serde_json::to_string(&authorities)?],
        )?;
    }
    Ok(IngestionCounts {
        added: u32::from(is_new),
        updated: u32::from(!is_new && changed),
        deduplicated: u32::from(!is_new && !changed),
        releases: release_count,
    })
}

fn save_rating(
    conn: &Connection,
    key: &str,
    incoming: &CatalogRating,
    stamp: &str,
) -> Result<bool> {
    if incoming.source.trim().is_empty() || incoming.category.trim().is_empty() {
        return Err(invalid("评分必须包含来源与类别"));
    }
    if !incoming.scale.is_finite()
        || incoming.scale <= 0.0
        || incoming
            .value
            .is_some_and(|v| !v.is_finite() || v < 0.0 || v > incoming.scale)
        || incoming
            .positive_percent
            .is_some_and(|v| !v.is_finite() || !(0.0..=100.0).contains(&v))
    {
        return Err(invalid("评分原始刻度或数值无效"));
    }
    let old: Option<CatalogRating> = conn
        .query_row(
            "SELECT payload FROM media_ratings WHERE media_key=?1 AND source=?2 AND category=?3",
            params![key, incoming.source, incoming.category],
            |r| json_row(r, 0),
        )
        .optional()?;
    let mut rating = incoming.clone();
    rating.url = option_text(rating.url);
    if rating.updated_at.is_empty() {
        rating.updated_at = stamp.into();
    }
    if let Some(old) = &old {
        if rating.updated_at < old.updated_at {
            return Ok(false);
        }
        if rating.value.is_none() {
            rating.value = old.value;
            rating.scale = old.scale;
        }
        rating.votes = rating.votes.or(old.votes);
        rating.reviews = rating.reviews.or(old.reviews);
        rating.positive_percent = rating.positive_percent.or(old.positive_percent);
        rating.rank = rating.rank.or(old.rank);
        rating.url = rating.url.or_else(|| old.url.clone());
        let mut comparable = rating.clone();
        comparable.updated_at.clone_from(&old.updated_at);
        if comparable == *old {
            conn.execute(
                "UPDATE media_ratings SET updated_at=?4,payload=?5
                WHERE media_key=?1 AND source=?2 AND category=?3",
                params![
                    key,
                    rating.source,
                    rating.category,
                    rating.updated_at,
                    serde_json::to_string(&rating)?
                ],
            )?;
            conn.execute(
                "UPDATE media_reviews SET updated_at=?4
                WHERE media_key=?1 AND source=?2 AND category=?3",
                params![key, rating.source, rating.category, rating.updated_at],
            )?;
            return Ok(false);
        }
    }
    conn.execute(
        "INSERT INTO rating_sources(source,label) VALUES (?1,?1) ON CONFLICT DO NOTHING",
        [&rating.source],
    )?;
    conn.execute(
        "INSERT INTO media_ratings(media_key,source,category,value,scale,votes,reviews,
            positive_percent,rank,url,updated_at,payload) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
         ON CONFLICT(media_key,source,category) DO UPDATE SET value=excluded.value,scale=excluded.scale,
            votes=excluded.votes,reviews=excluded.reviews,positive_percent=excluded.positive_percent,
            rank=excluded.rank,url=excluded.url,updated_at=excluded.updated_at,payload=excluded.payload",
        params![key, rating.source, rating.category, rating.value, rating.scale, rating.votes,
            rating.reviews, rating.positive_percent, rating.rank, rating.url, rating.updated_at,
            serde_json::to_string(&rating)?],
    )?;
    conn.execute(
        "INSERT INTO media_reviews(media_key,source,category,reviews,positive_percent,votes,rank,url,updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
         ON CONFLICT(media_key,source,category) DO UPDATE SET reviews=excluded.reviews,
            positive_percent=excluded.positive_percent,votes=excluded.votes,rank=excluded.rank,
            url=excluded.url,updated_at=excluded.updated_at",
        params![key, rating.source, rating.category, rating.reviews, rating.positive_percent,
            rating.votes, rating.rank, rating.url, rating.updated_at],
    )?;
    Ok(true)
}

fn choose_rating(conn: &Connection, media: &mut CatalogMedia) -> Result<()> {
    // Prefer the originating source, then retain the selected source. Only one user
    // or audience score is eligible; critics never contribute to this filter value.
    let selected: Option<(f64, String)> = conn
        .query_row(
            "SELECT value,source FROM media_ratings WHERE media_key=?1
            AND lower(category) IN ('user','audience','users','audience_score','user_score')
            AND value IS NOT NULL AND scale>0
         ORDER BY CASE WHEN source=?2 THEN 0 WHEN source=?3 THEN 1 ELSE 2 END,
            source, CASE WHEN lower(category)='user' THEN 0 ELSE 1 END,category LIMIT 1",
            params![media.key, media.source, media.rating_source],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    media.rating = selected.as_ref().map(|r| r.0);
    media.rating_source = selected.map(|r| r.1);
    Ok(())
}

fn save_season(
    conn: &Connection,
    key: &str,
    default_source: &str,
    incoming: &CatalogSeason,
    stamp: &str,
) -> Result<bool> {
    let mut season = incoming.clone();
    if season.source.is_empty() {
        season.source = default_source.into();
    }
    identity(&season.source, &season.external_id)?;
    season.title = option_text(season.title);
    season.premiered = option_text(season.premiered);
    season.ended = option_text(season.ended);
    let old = conn.query_row(
        "SELECT payload,updated_at FROM seasons WHERE media_key=?1 AND source=?2 AND external_id=?3",
        params![key, season.source, season.external_id],
        |r| Ok((json_row::<CatalogSeason>(r, 0)?, r.get::<_, String>(1)?)),
    ).optional()?;
    if let Some((old, old_stamp)) = old {
        if stamp < old_stamp.as_str() {
            return Ok(false);
        }
        season.number = season.number.or(old.number);
        season.title = season.title.or_else(|| old.title.clone());
        season.episode_count = season.episode_count.or(old.episode_count);
        season.premiered = season.premiered.or_else(|| old.premiered.clone());
        season.ended = season.ended.or_else(|| old.ended.clone());
        if season == old {
            conn.execute("UPDATE seasons SET updated_at=?4 WHERE media_key=?1 AND source=?2 AND external_id=?3",
                params![key, season.source, season.external_id, stamp])?;
            return Ok(false);
        }
    }
    conn.execute(
        "INSERT INTO seasons(media_key,source,external_id,number,title,episode_count,premiered,ended,payload,updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
         ON CONFLICT(media_key,source,external_id) DO UPDATE SET number=excluded.number,title=excluded.title,
            episode_count=excluded.episode_count,premiered=excluded.premiered,ended=excluded.ended,
            payload=excluded.payload,updated_at=excluded.updated_at",
        params![key, season.source, season.external_id, season.number, season.title, season.episode_count,
            season.premiered, season.ended, serde_json::to_string(&season)?, stamp],
    )?;
    Ok(true)
}

fn save_release(
    conn: &Connection,
    key: &str,
    default_source: &str,
    incoming: &CatalogRelease,
    stamp: &str,
) -> Result<bool> {
    let mut release = incoming.clone();
    if release.source.is_empty() {
        release.source = default_source.into();
    }
    release.key = format!(
        "release:{}",
        identity(&release.source, &release.external_id)?
    );
    release.media_key = key.into();
    if !valid_date(&release.date) {
        return Err(invalid("发行日期格式应为有效的 YYYY-MM-DD"));
    }
    if release.release_kind.trim().is_empty() {
        return Err(invalid("发行记录必须包含发行类别"));
    }
    release.local_time = option_text(release.local_time);
    release.timezone = option_text(release.timezone);
    release.utc_time = option_text(release.utc_time);
    release.episode = option_text(release.episode);
    release.title = option_text(release.title);
    release.platform = option_text(release.platform);
    release.url = option_text(release.url);
    if release.updated_at.is_empty() {
        release.updated_at = stamp.into();
    }
    let old: Option<CatalogRelease> = conn
        .query_row(
            "SELECT payload FROM release_schedules WHERE source=?1 AND external_id=?2",
            params![release.source, release.external_id],
            |r| json_row(r, 0),
        )
        .optional()?;
    if let Some(old) = &old {
        if old.media_key != key {
            return Err(conflict());
        }
        if release.updated_at < old.updated_at {
            return Ok(false);
        }
        macro_rules! retain {
            ($($field:ident),+ $(,)?) => { $(release.$field = release.$field.or_else(|| old.$field.clone());)+ };
        }
        // An old absolute instant is no longer valid after a schedule correction.
        // If the source omits the corrected instant, leave it unknown, not invented.
        if release.utc_time.is_none()
            && release.date == old.date
            && release
                .local_time
                .as_ref()
                .is_none_or(|time| Some(time) == old.local_time.as_ref())
            && release
                .timezone
                .as_ref()
                .is_none_or(|zone| Some(zone) == old.timezone.as_ref())
        {
            release.utc_time.clone_from(&old.utc_time);
        }
        release.season = release.season.or(old.season);
        retain!(local_time, timezone, episode, title, platform, url);
        let mut comparable = release.clone();
        comparable.updated_at.clone_from(&old.updated_at);
        if comparable == *old {
            conn.execute("UPDATE release_schedules SET updated_at=?3,payload=?4 WHERE source=?1 AND external_id=?2",
                params![release.source, release.external_id, release.updated_at, serde_json::to_string(&release)?])?;
            return Ok(false);
        }
    }
    conn.execute(
        "INSERT INTO release_schedules(key,media_key,source,external_id,date,local_time,timezone,utc_time,
            season,episode,title,platform,release_kind,url,updated_at,payload)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)
         ON CONFLICT(source,external_id) DO UPDATE SET date=excluded.date,local_time=excluded.local_time,
            timezone=excluded.timezone,utc_time=excluded.utc_time,season=excluded.season,episode=excluded.episode,
            title=excluded.title,platform=excluded.platform,release_kind=excluded.release_kind,url=excluded.url,
            updated_at=excluded.updated_at,payload=excluded.payload",
        params![release.key, key, release.source, release.external_id, release.date, release.local_time,
            release.timezone, release.utc_time, release.season, release.episode, release.title,
            release.platform, release.release_kind, release.url, release.updated_at, serde_json::to_string(&release)?],
    )?;
    if matches!(release.release_kind.as_str(), "chapter" | "manga_chapter") {
        conn.execute(
            "INSERT INTO manga_chapters(source,external_id,media_key,number,title) VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(source,external_id) DO UPDATE SET number=excluded.number,title=excluded.title",
            params![release.source, release.external_id, key, release.episode, release.title],
        )?;
    } else if release.release_kind == "episode"
        || (release.episode.is_some() && release.release_kind != "legacy")
    {
        conn.execute(
            "INSERT INTO episodes(source,external_id,media_key,season,number,title) VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(source,external_id) DO UPDATE SET season=excluded.season,number=excluded.number,title=excluded.title",
            params![release.source, release.external_id, key, release.season, release.episode, release.title],
        )?;
    }
    Ok(true)
}

fn literal_like(search: &str) -> String {
    format!(
        "%{}%",
        search
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}
fn pagination(q: &CatalogQuery) -> (u32, u32, i64) {
    let page = q.page.unwrap_or(1).max(1);
    let size = q.page_size.unwrap_or(40).clamp(1, 100);
    (page, size, i64::from(page - 1) * i64::from(size))
}
fn media_filters_except(q: &CatalogQuery, skip: &str) -> (String, Vec<Value>) {
    let mut clauses = vec!["1=1".to_string()];
    let mut values: Vec<Value> = Vec::new();
    // 不用闭包做 push：后面还有直接操作 clauses/values 的多选子句，
    // 闭包的互斥借用会跟它们打架。宏展开直接引用本地变量。
    macro_rules! push {
        ($clause:expr, $value:expr $(,)?) => {{
            clauses.push($clause.into());
            values.push($value);
        }};
    }
    if skip != "search" {
        if let Some(search) = q.search.as_deref().filter(|s| !s.trim().is_empty()) {
            push!(
                "m.search_text LIKE ? ESCAPE '\\' COLLATE NOCASE",
                literal_like(search).into(),
            );
        }
    }
    for (name, value, sql) in [
        ("kind", &q.kind, "m.kind=?"),
        ("status", &q.status, "m.status=?"),
        (
            "region",
            &q.region,
            "EXISTS(SELECT 1 FROM media_regions f WHERE f.media_key=m.key AND f.region=?)",
        ),
        (
            "genre",
            &q.genre,
            "EXISTS(SELECT 1 FROM media_genres f WHERE f.media_key=m.key AND f.genre=?)",
        ),
        (
            "platform",
            &q.platform,
            "EXISTS(SELECT 1 FROM media_platforms f WHERE f.media_key=m.key AND f.platform=?)",
        ),
    ] {
        if skip == name { continue; }
        if let Some(value) = value.as_ref().filter(|s| !s.is_empty()) {
            push!(sql, value.clone().into());
        }
    }
    // 国内/国外页签：条目只要带该组任一采集源就算命中。同作品两边都有源时
    // 两个页签都出现，详情里写明每个来源。
    if skip != "scope" {
        if let Some(scope) = q.scope.as_deref().filter(|s| matches!(*s, "domestic" | "foreign")) {
            let sources = super::sources::scope_sources(scope);
            let placeholders = sources.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            clauses.push(format!(
                "EXISTS(SELECT 1 FROM media_external_ids e WHERE e.media_key=m.key AND e.source IN ({placeholders}))"
            ));
            values.extend(sources.iter().map(|s| (*s).to_string().into()));
        }
    }
    // 地区多选（任一命中）
    if skip != "regions" && !q.regions.is_empty() {
        let placeholders = q.regions.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        clauses.push(format!(
            "EXISTS(SELECT 1 FROM media_regions f WHERE f.media_key=m.key AND f.region IN ({placeholders}))"
        ));
        values.extend(q.regions.iter().map(|r| r.clone().into()));
    }
    // 题材多选：mode=all 时每个题材各挂一个 EXISTS（同时包含全部题材）；
    // mode=any（默认）时一个 IN 子查询（任一命中）。
    if skip != "genres" && !q.genres.is_empty() {
        let all = q.genre_mode.as_deref() == Some("all");
        if all {
            for genre in &q.genres {
                push!(
                    "EXISTS(SELECT 1 FROM media_genres f WHERE f.media_key=m.key AND f.genre=?)",
                    genre.clone().into(),
                );
            }
        } else {
            let placeholders = q.genres.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            clauses.push(format!(
                "EXISTS(SELECT 1 FROM media_genres f WHERE f.media_key=m.key AND f.genre IN ({placeholders}))"
            ));
            values.extend(q.genres.iter().map(|g| g.clone().into()));
        }
    }
    // 演员多选：all = 每个名字一个 EXISTS（「两个演员同框」），any = 任一命中。
    if skip != "cast" && !q.cast.is_empty() {
        let all = q.cast_mode.as_deref() == Some("all");
        if all {
            for name in &q.cast {
                push!(
                    "EXISTS(SELECT 1 FROM media_people f WHERE f.media_key=m.key AND f.role='cast' AND f.name=?)",
                    name.clone().into(),
                );
            }
        } else {
            let placeholders = q.cast.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            clauses.push(format!(
                "EXISTS(SELECT 1 FROM media_people f WHERE f.media_key=m.key AND f.role='cast' AND f.name IN ({placeholders}))"
            ));
            values.extend(q.cast.iter().map(|c| c.clone().into()));
        }
    }
    if skip != "year" {
        if let Some(year) = q.year {
            push!("m.year=?", i64::from(year).into());
        }
        if let Some(from) = q.year_from {
            push!("m.year>=?", i64::from(from).into());
        }
        if let Some(to) = q.year_to {
            push!("m.year<=?", i64::from(to).into());
        }
    }
    if skip != "rating" {
        if let Some(rating) = q.min_rating {
            push!("m.rating>=?", rating.into());
        }
    }
    (clauses.join(" AND "), values)
}
fn media_filters(q: &CatalogQuery) -> (String, Vec<Value>) { media_filters_except(q, "") }

/// 分面计数用的过滤：按「除本组外全部生效」口径，日期/星期条件沿用日历的媒体集合。
fn facet_filters(q: &CatalogQuery, skip: &str) -> (String, Vec<Value>) {
    let (mut filter, mut values) = media_filters_except(q, skip);
    for (date, sql) in [(&q.start, "r.date>=?"), (&q.end, "r.date<?")] {
        if let Some(date) = date.as_deref().filter(|d| valid_date(d)) {
            filter.push_str(&format!(" AND EXISTS(SELECT 1 FROM release_schedules r WHERE r.media_key=m.key AND {sql})"));
            values.push(date.to_string().into());
        }
    }
    if let Some(weekday) = q.weekday.filter(|d| (1..=7).contains(d)) {
        filter.push_str(" AND EXISTS(SELECT 1 FROM release_schedules r WHERE r.media_key=m.key AND ((CAST(strftime('%w',r.date) AS INTEGER)+6)%7)+1=?)");
        values.push(i64::from(weekday).into());
    }
    (filter, values)
}
fn media_order(sort: Option<&str>) -> &'static str {
    match sort {
        Some("title") => "m.title COLLATE NOCASE,m.key",
        Some("year") => "m.year IS NULL,m.year DESC,m.title COLLATE NOCASE,m.key",
        Some("rating") => "m.rating IS NULL,m.rating DESC,m.title COLLATE NOCASE,m.key",
        // 最近更新：热增量每 15 分钟改写 updated_at，这是「最新更新」视图的主排序
        Some("updated") => "m.updated_at DESC,m.key",
        _ => "m.first_seen DESC,m.key",
    }
}
fn query_media(conn: &Connection, sql: &str, values: &[Value]) -> Result<Vec<CatalogMedia>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params_from_iter(values.iter()), |r| json_row(r, 0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn library(db: &Db, q: &CatalogQuery) -> Result<CatalogPage> {
    db.read(|conn| {
        let (filter, mut values) = media_filters(q);
        let (page, page_size, offset) = pagination(q);
        let total = conn.query_row(
            &format!("SELECT COUNT(*) FROM media m WHERE {filter}"),
            params_from_iter(values.iter()),
            |r| r.get(0),
        )?;
        let sql = format!(
            "SELECT m.payload FROM media m WHERE {filter} ORDER BY {} LIMIT ? OFFSET ?",
            media_order(q.sort.as_deref())
        );
        values.push(i64::from(page_size).into());
        values.push(offset.into());
        Ok(CatalogPage {
            items: query_media(conn, &sql, &values)?,
            total,
            page,
            page_size,
        })
    })
}

pub fn releases(db: &Db, q: &CatalogQuery) -> Result<CatalogReleasePage> {
    db.read(|conn| {
        let (mut filter, mut values) = media_filters(q);
        for (date, sql) in [(&q.start, "r.date>=?"), (&q.end, "r.date<?")] {
            if let Some(date) = date {
                if !valid_date(date) {
                    return Err(invalid("查询日期格式应为有效的 YYYY-MM-DD"));
                }
                filter.push_str(" AND ");
                filter.push_str(sql);
                values.push(date.clone().into());
            }
        }
        if let Some(weekday) = q.weekday {
            if !(1..=7).contains(&weekday) {
                return Err(invalid("星期范围应为 1 至 7"));
            }
            filter.push_str(" AND ((CAST(strftime('%w',r.date) AS INTEGER)+6)%7)+1=?");
            values.push(i64::from(weekday).into());
        }
        let from =
            format!("FROM release_schedules r JOIN media m ON m.key=r.media_key WHERE {filter}");
        let total = conn.query_row(
            &format!("SELECT COUNT(*) {from}"),
            params_from_iter(values.iter()),
            |r| r.get(0),
        )?;
        let (page, page_size, offset) = pagination(q);
        let order = match q.sort.as_deref() {
            Some("rating") => "m.rating IS NULL,m.rating DESC,r.date,r.local_time,r.key",
            Some("newest") => "r.date DESC,r.local_time DESC,r.key",
            Some("title") => "m.title COLLATE NOCASE,r.date,r.local_time,r.key",
            _ => "r.date,r.local_time,r.key",
        };
        values.push(i64::from(page_size).into());
        values.push(offset.into());
        let mut stmt = conn.prepare(&format!(
            "SELECT r.payload,m.payload {from} ORDER BY {order} LIMIT ? OFFSET ?"
        ))?;
        let items = stmt
            .query_map(params_from_iter(values.iter()), |r| {
                Ok(CatalogReleaseRow {
                    release: json_row(r, 0)?,
                    media: json_row(r, 1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(CatalogReleasePage {
            items,
            total,
            page,
            page_size,
        })
    })
}

pub fn detail(db: &Db, key: &str) -> Result<Option<CatalogDetail>> {
    db.read(|conn| {
        let media = conn.query_row("SELECT payload FROM media WHERE key=?1", [key], |r| json_row(r, 0)).optional()?;
        let Some(media) = media else { return Ok(None); };
        let mut stmt = conn.prepare("SELECT source,external_id FROM media_external_ids WHERE media_key=?1 ORDER BY source,external_id")?;
        let external_ids = stmt.query_map([key], |r| Ok(CatalogExternalId { source: r.get(0)?, external_id: r.get(1)? }))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut stmt = conn.prepare("SELECT payload FROM media_ratings WHERE media_key=?1 ORDER BY source,category")?;
        let ratings = stmt.query_map([key], |r| json_row(r, 0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut stmt = conn.prepare("SELECT payload FROM seasons WHERE media_key=?1 ORDER BY number IS NULL,number,source,external_id")?;
        let seasons = stmt.query_map([key], |r| json_row(r, 0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut stmt = conn.prepare("SELECT payload FROM release_schedules WHERE media_key=?1 ORDER BY date DESC,local_time DESC,key LIMIT ?2")?;
        let releases = stmt.query_map(params![key, DETAIL_RELEASE_LIMIT], |r| json_row(r, 0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut stmt = conn.prepare(
            "SELECT source,title,url,episode_text,updated_at FROM media_source_info
             WHERE media_key=?1 ORDER BY source",
        )?;
        let source_infos = stmt
            .query_map([key], |r| {
                Ok(CatalogSourceInfo {
                    source: r.get(0)?,
                    title: r.get(1)?,
                    url: r.get(2)?,
                    episode_text: r.get(3)?,
                    updated_at: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut stmt = conn.prepare(
            "SELECT key_a,key_b,reason,created_at FROM media_dupe_candidates
             WHERE status='open' AND (key_a=?1 OR key_b=?1) ORDER BY created_at DESC LIMIT 20",
        )?;
        let rows = stmt
            .query_map([key], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut dupe_candidates = Vec::with_capacity(rows.len());
        let mut title_stmt = conn.prepare_cached("SELECT title,year FROM media WHERE key=?1")?;
        for (key_a, key_b, reason, created_at) in rows {
            let (title_a, year_a): (String, Option<i32>) = title_stmt.query_row(params![key_a], |r| Ok((r.get(0)?, r.get(1)?))).optional()?.unwrap_or_default();
            let (title_b, year_b): (String, Option<i32>) = title_stmt.query_row(params![key_b], |r| Ok((r.get(0)?, r.get(1)?))).optional()?.unwrap_or_default();
            dupe_candidates.push(CatalogDupeCandidate { key_a, key_b, reason, title_a, title_b, year_a, year_b, created_at });
        }
        Ok(Some(CatalogDetail { media, external_ids, ratings, seasons, releases, source_infos, dupe_candidates }))
    })
}

/// 跨源去重回填 + 全库扫描的一次性报告。
#[derive(Debug, Clone, Default, Serialize)]
pub struct DedupeReport {
    /// 补写 norm_title 的行数
    pub norm_backfilled: usize,
    /// 补写 media_people 的行数
    pub people_backfilled: usize,
    /// 补写 media_source_info 的行数
    pub source_info_backfilled: usize,
    /// 自动合并掉的行数
    pub merged: usize,
    /// 新记录的疑似重复候选数
    pub suspects: usize,
}

/// 一次性回填 + 全库去重扫描。由 catalog_meta 的 DEDUPE_MARKER 门控，
/// 挂在下一次 auto_sync 开头（互斥锁内）执行；存量行在本次之后进入增量口径。
pub fn dedupe_backfill(db: &Db, now: &str) -> Result<DedupeReport> {
    db.write(|conn| {
        let done: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM catalog_meta WHERE key=?1)",
            [DEDUPE_MARKER],
            |r| r.get(0),
        )?;
        if done {
            return Ok(DedupeReport::default());
        }
        let tx = conn.unchecked_transaction()?;
        let mut report = DedupeReport::default();

        // 1) norm_title 回填：存量行的归一化标题（payload 里的别名也算进分组素材）
        let mut norm_updates: Vec<(String, String)> = Vec::new();
        {
            let mut stmt = tx.prepare("SELECT key,payload FROM media WHERE norm_title IS NULL")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (key, payload) in rows {
                let media = serde_json::from_str::<CatalogMedia>(&payload)
                    .map_err(|e| AppError::new(ErrorKind::Io, format!("资料 payload 损坏（{key}）：{e}")))?;
                let norm = normalize_title(&media.title);
                if !norm.is_empty() {
                    norm_updates.push((key, norm));
                }
            }
        }
        {
            let mut stmt = tx.prepare_cached("UPDATE media SET norm_title=?2 WHERE key=?1")?;
            for (key, norm) in &norm_updates {
                stmt.execute(params![key, norm])?;
            }
        }
        report.norm_backfilled = norm_updates.len();

        // 2) media_people 回填：没有人员行的条目从 payload 解析一次
        {
            let mut stmt = tx.prepare(
                "SELECT key,payload FROM media WHERE key NOT IN (SELECT DISTINCT media_key FROM media_people)",
            )?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut insert = tx.prepare_cached(
                "INSERT INTO media_people(media_key,name,role) VALUES (?1,?2,?3) ON CONFLICT DO NOTHING",
            )?;
            for (key, payload) in rows {
                let Ok(media) = serde_json::from_str::<CatalogMedia>(&payload) else { continue };
                for (names, role) in [
                    (&media.cast, "cast"),
                    (&media.directors, "director"),
                    (&media.writers, "writer"),
                ] {
                    for name in names.iter().filter(|n| !n.trim().is_empty()) {
                        insert.execute(params![key, name.trim(), role])?;
                    }
                }
                report.people_backfilled += 1;
            }
        }

        // 3) media_source_info 回填：每行的主来源各补一条（历史行没有逐源信息）
        {
            let mut stmt = tx.prepare(
                "SELECT key,payload FROM media WHERE key NOT IN (SELECT DISTINCT media_key FROM media_source_info)",
            )?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut insert = tx.prepare_cached(
                "INSERT INTO media_source_info(media_key,source,title,url,episode_text,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(media_key,source) DO NOTHING",
            )?;
            for (key, payload) in rows {
                let Ok(media) = serde_json::from_str::<CatalogMedia>(&payload) else { continue };
                if media.source.trim().is_empty() { continue; }
                insert.execute(params![
                    key,
                    media.source,
                    option_text(Some(media.title.clone())),
                    option_text(media.source_url.clone()),
                    episode_text_of(&media),
                    media.updated_at,
                ])?;
                report.source_info_backfilled += 1;
            }
        }

        // 4) 全库分组扫描：同类型 + 同归一化标题（含别名）为一组。
        //    年份全部一致的组自动合并；年份缺失/冲突的组记疑似候选。
        let mut stmt = tx.prepare(
            "SELECT m.key,m.kind,m.year,m.first_seen,m.payload,
                    (SELECT group_concat(a.alias,'|') FROM media_aliases a WHERE a.media_key=m.key)
             FROM media m WHERE m.norm_title IS NOT NULL",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<i32>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        // (kind, norm) -> keys；一行多个归一化形态（主标题 + 别名）可落在多个组
        let mut groups: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
        let mut meta: BTreeMap<String, (Option<i32>, String, CatalogMedia)> = BTreeMap::new();
        for (key, kind, year, first_seen, payload, aliases) in rows {
            let Ok(media) = serde_json::from_str::<CatalogMedia>(&payload) else { continue };
            let mut forms = vec![normalize_title(&media.title)];
            if let Some(list) = aliases {
                forms.extend(list.split('|').map(normalize_title));
            }
            for alias in &media.aliases {
                forms.push(normalize_title(alias));
            }
            for form in forms.into_iter().filter(|f| f.chars().count() >= 2) {
                groups.entry((kind.clone(), form)).or_default().push(key.clone());
            }
            meta.insert(key, (year, first_seen, media));
        }
        let mut merge_pairs: Vec<(String, String)> = Vec::new();
        let mut suspect_stmt = tx.prepare_cached(
            "INSERT INTO media_dupe_candidates(key_a,key_b,reason,status,created_at)
             VALUES (?1,?2,?3,'open',?4) ON CONFLICT(key_a,key_b) DO NOTHING",
        )?;
        let mut record_suspect = |a: &str, b: &str, reason: &str, suspect_stmt: &mut rusqlite::Statement<'_>| {
            let (key_a, key_b) = if a < b { (a, b) } else { (b, a) };
            suspect_stmt.execute(params![key_a, key_b, reason, now])?;
            report.suspects += 1;
            rusqlite::Result::<()>::Ok(())
        };
        for ((_, _), mut keys) in groups {
            keys.sort();
            keys.dedup();
            if keys.len() < 2 || keys.len() > 12 {
                continue;
            }
            // 组内按年份聚类：同年（都有值）的子组自动合并；无年份与年份冲突的
            // 行不自动合，与同组其他行互记疑似候选。
            let mut by_year: BTreeMap<i32, Vec<String>> = BTreeMap::new();
            let mut undated: Vec<String> = Vec::new();
            for key in &keys {
                match meta.get(key).and_then(|(y, _, _)| *y) {
                    Some(year) => by_year.entry(year).or_default().push(key.clone()),
                    None => undated.push(key.clone()),
                }
            }
            let mut clustered: Vec<String> = Vec::new();
            for cluster in by_year.values() {
                if cluster.len() < 2 {
                    continue;
                }
                let mut ordered = cluster.clone();
                ordered.sort_by_key(|k| meta.get(k).map(|(_, first, _)| first.clone()).unwrap_or_default());
                if let Some(target) = ordered.first().cloned() {
                    for loser in ordered.iter().skip(1) {
                        merge_pairs.push((target.clone(), loser.clone()));
                    }
                }
                clustered.extend(cluster.iter().cloned());
            }
            let ambiguous: Vec<String> = keys.iter().filter(|k| !clustered.contains(k)).cloned().collect();
            let reason = if undated.len() == ambiguous.len() { "year_missing" } else { "year_conflict" };
            for i in 0..ambiguous.len() {
                for j in (i + 1)..ambiguous.len().min(i + 6) {
                    record_suspect(&ambiguous[i], &ambiguous[j], reason, &mut suspect_stmt)?;
                }
                // 疑似行要跟已聚类的每一行对上：用户从哪个来源的详情看，都能看到提示
                for head in &clustered {
                    if ambiguous[i] != *head {
                        record_suspect(&ambiguous[i], head, reason, &mut suspect_stmt)?;
                    }
                }
            }
        }
        drop(suspect_stmt);
        for (target, loser) in &merge_pairs {
            // 同一行可能出现在多个别名组里：早前合并已经把它带走了就跳过本轮
            let loser_exists: bool = tx
                .query_row("SELECT EXISTS(SELECT 1 FROM media WHERE key=?1)", [loser], |r| r.get(0))?;
            let target_exists: bool = tx
                .query_row("SELECT EXISTS(SELECT 1 FROM media WHERE key=?1)", [target], |r| r.get(0))?;
            if !loser_exists || !target_exists || loser == target {
                continue;
            }
            merge_row_into(&tx, target, loser, now)?;
            report.merged += 1;
        }
        tx.execute(
            "INSERT INTO catalog_meta(key,value) VALUES (?1,'complete') ON CONFLICT(key) DO NOTHING",
            [DEDUPE_MARKER],
        )?;
        tx.commit()?;
        Ok(report)
    })
}

/// 把 loser 行并入 target 行：payload 走 merge_media 权威裁决，外部 ID/季/集/发行
/// 记录全部改挂 target，评分非冲突行逐条搬运，败者整行归档后删除。
/// 各季不合并：seasons 按 (media_key,source,external_id) 独立存在，全部跟到 target。
fn merge_row_into(conn: &Connection, target: &str, loser: &str, now: &str) -> Result<()> {
    if target == loser {
        return Ok(());
    }
    let (target_media, mut authorities) = conn
        .query_row(
            "SELECT payload,authority_json FROM media WHERE key=?1",
            [target],
            |r| Ok((json_row::<CatalogMedia>(r, 0)?, json_row::<Authorities>(r, 1)?)),
        )
        .optional()?
        .ok_or_else(|| invalid("要保留的资料条目不存在"))?;
    let loser_media: CatalogMedia = conn
        .query_row("SELECT payload FROM media WHERE key=?1", [loser], |r| json_row(r, 0))
        .optional()?
        .ok_or_else(|| invalid("要合并的资料条目不存在"))?;
    let loser_source = loser_media.source.clone();
    let loser_stamp = loser_media.updated_at.clone();
    let merged = merge_media(&target_media, &loser_media, &loser_source, &loser_stamp, &mut authorities);

    // 归档败者：合并可追溯，payload 里有该源的完整原始元数据
    conn.execute(
        "INSERT INTO media_merge_archive(merged_key,into_key,payload,merged_at)
         VALUES (?1,?2,?3,?4) ON CONFLICT(merged_key) DO NOTHING",
        params![loser, target, serde_json::to_string(&loser_media)?, now],
    )?;

    // 评分：先搬不冲突的（save_rating 会连带 reviews），冲突的 (source,category) 保留 target 侧
    {
        let mut stmt = conn.prepare("SELECT payload FROM media_ratings WHERE media_key=?1")?;
        let ratings = stmt
            .query_map([loser], |r| json_row::<CatalogRating>(r, 0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        let mut exists = conn.prepare_cached(
            "SELECT EXISTS(SELECT 1 FROM media_ratings WHERE media_key=?1 AND source=?2 AND category=?3)",
        )?;
        for rating in &ratings {
            let conflict: bool = exists.query_row(
                params![target, rating.source, rating.category],
                |r| r.get(0),
            )?;
            if !conflict {
                save_rating(conn, target, rating, &rating.updated_at)?;
            }
        }
    }
    conn.execute("DELETE FROM media_reviews WHERE media_key=?1", params![loser])?;
    conn.execute("DELETE FROM media_ratings WHERE media_key=?1", params![loser])?;

    // 季/集/章节/发行记录改挂 target；同 (source,external_id) 已在 target 侧存在的
    // 直接丢弃败者侧（说明同一来源条目已在 target 管辖下）
    for table in ["seasons", "episodes", "manga_chapters", "release_schedules"] {
        conn.execute(
            &format!("DELETE FROM {table} WHERE media_key=?1 AND (source,external_id) IN (SELECT source,external_id FROM {table} WHERE media_key=?2)"),
            params![loser, target],
        )?;
        conn.execute(&format!("UPDATE {table} SET media_key=?2 WHERE media_key=?1"), params![loser, target])?;
    }
    conn.execute(
        "UPDATE media_external_ids SET media_key=?2 WHERE media_key=?1",
        params![loser, target],
    )?;
    for table in ["media_aliases", "media_genres", "media_regions", "media_platforms", "media_people"] {
        conn.execute(&format!("DELETE FROM {table} WHERE media_key=?1"), params![loser])?;
    }
    // 败者的逐源落地信息搬进 target：同源冲突保留 updated_at 较新的那条
    {
        let mut stmt = conn.prepare(
            "SELECT source,title,url,episode_text,updated_at FROM media_source_info WHERE media_key=?1",
        )?;
        let rows = stmt
            .query_map([loser], |r| {
                Ok(CatalogSourceInfo {
                    source: r.get(0)?,
                    title: r.get(1)?,
                    url: r.get(2)?,
                    episode_text: r.get(3)?,
                    updated_at: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        for info in rows {
            let existing: Option<String> = conn
                .query_row(
                    "SELECT updated_at FROM media_source_info WHERE media_key=?1 AND source=?2",
                    params![target, info.source],
                    |r| r.get(0),
                )
                .optional()?;
            if existing.as_deref().is_none_or(|stamp| stamp < info.updated_at.as_str()) {
                conn.execute(
                    "INSERT INTO media_source_info(media_key,source,title,url,episode_text,updated_at)
                     VALUES (?1,?2,?3,?4,?5,?6)
                     ON CONFLICT(media_key,source) DO UPDATE SET title=excluded.title,url=excluded.url,
                         episode_text=excluded.episode_text,updated_at=excluded.updated_at",
                    params![target, info.source, info.title, info.url, info.episode_text, info.updated_at],
                )?;
            }
        }
        conn.execute("DELETE FROM media_source_info WHERE media_key=?1", params![loser])?;
    }
    // 收藏跟到 target：target 已收藏时保留 target 的状态（用户态以在看的为准），
    // 未收藏则整行搬移。收藏是用户态，合并/删除条目绝不能把它弄丢。
    let target_followed: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM media_follows WHERE media_key=?1)",
            [target],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(false);
    if target_followed {
        conn.execute("DELETE FROM media_follows WHERE media_key=?1", params![loser])?;
    } else {
        conn.execute(
            "UPDATE media_follows SET media_key=?2 WHERE media_key=?1",
            params![loser, target],
        )?;
    }
    conn.execute("DELETE FROM media_dupe_candidates WHERE key_a=?1 OR key_b=?1",
        params![loser],
    )?;
    conn.execute("DELETE FROM media WHERE key=?1", params![loser])?;
    save_media(conn, &merged, &authorities)?;
    Ok(())
}

/// 详情页「疑似重复」的人工处置：merge=true 把 drop 并入 keep；false 标记忽略。
pub fn merge_candidates(db: &Db, keep: &str, drop: &str, merge: bool, now: &str) -> Result<String> {
    if keep == drop {
        return Err(invalid("两条资料库 ID 相同，不需要合并"));
    }
    db.write(|conn| {
        if !merge {
            let changed = conn.execute(
                "UPDATE media_dupe_candidates SET status='dismissed'
                 WHERE (key_a=?1 AND key_b=?2) OR (key_a=?2 AND key_b=?1)",
                params![keep, drop],
            )?;
            return Ok(if changed > 0 {
                "已标记为不同作品，不再提示".into()
            } else {
                "没有找到对应的疑似重复候选".into()
            });
        }
        merge_row_into(conn, keep, drop, now)?;
        conn.execute(
            "UPDATE media_dupe_candidates SET status='merged'
             WHERE (key_a=?1 AND key_b=?2) OR (key_a=?2 AND key_b=?1)",
            params![keep, drop],
        )?;
        Ok("已合并：各来源信息保留在详情的「各来源信息」区".into())
    })
}

pub const FOLLOW_STATES: &[&str] = &["want", "watching", "shelve", "done"];

/// 设置/取消追剧收藏。state=None = 取消收藏（整行删除）。
pub fn follow_set(db: &Db, key: &str, state: Option<&str>, now: &str) -> Result<()> {
    if let Some(state) = state {
        if !FOLLOW_STATES.contains(&state) {
            return Err(invalid("追剧状态只能是 want、watching、shelve 或 done"));
        }
    }
    db.write(|conn| {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM media WHERE key=?1)",
                [key],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false);
        if !exists {
            return Err(invalid("资料条目不存在，可能已被合并；请重新打开详情"));
        }
        match state {
            Some(state) => conn.execute(
                "INSERT INTO media_follows(media_key,state,created_at,updated_at)
                 VALUES (?1,?2,?3,?4)
                 ON CONFLICT(media_key) DO UPDATE SET state=excluded.state,updated_at=excluded.updated_at",
                params![key, state, now, now],
            )?,
            None => conn.execute("DELETE FROM media_follows WHERE media_key=?1", params![key])?,
        };
        Ok(())
    })
}

/// 「我的追剧」列表：收藏条目 + 最近发行 / 最近集数 / 即将更新。
pub fn follow_list(db: &Db, states: &[String]) -> Result<Vec<CatalogFollowRow>> {
    db.read(|conn| {
        let mut sql = "SELECT m.payload,f.state,f.updated_at,
                (SELECT MAX(r.date) FROM release_schedules r WHERE r.media_key=m.key) AS latest,
                (SELECT r.episode FROM release_schedules r WHERE r.media_key=m.key
                     ORDER BY r.date DESC,r.local_time DESC,r.key LIMIT 1) AS latest_episode,
                (SELECT MIN(r.date) FROM release_schedules r WHERE r.media_key=m.key
                     AND r.date>date('now','localtime')) AS next_release
             FROM media_follows f JOIN media m ON m.key=f.media_key".to_string();
        if !states.is_empty() {
            let placeholders = states.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            sql.push_str(&format!(" WHERE f.state IN ({placeholders})"));
        }
        sql.push_str(" ORDER BY latest DESC,m.updated_at DESC,m.key");
        let mut stmt = conn.prepare(&sql)?;
        let map_row = |r: &Row<'_>| -> rusqlite::Result<CatalogFollowRow> {
            Ok(CatalogFollowRow {
                media: json_row(r, 0)?,
                state: r.get(1)?,
                updated_at: r.get(2)?,
                latest_release: r.get(3)?,
                latest_episode: r.get(4)?,
                next_release: r.get(5)?,
            })
        };
        let rows = if states.is_empty() {
            stmt.query_map([], map_row)?.collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            stmt.query_map(params_from_iter(states.iter()), map_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(rows)
    })
}

fn kind_label(kind: &str) -> &str {
    match kind {
        "anime" => "动漫",
        "tv" => "电视剧",
        "movie" => "电影",
        "variety" => "综艺",
        "documentary" => "纪录片",
        "manga" => "漫画",
        "other" => "其他",
        _ => kind,
    }
}
fn region_label(region: &str) -> &str {
    match region {
        "CN" => "中国大陆",
        "JP" => "日本",
        "KR" => "韩国",
        "US" => "美国",
        "GB" => "英国",
        "TH" => "泰国",
        "HK" => "中国香港",
        "TW" => "中国台湾",
        "FR" => "法国",
        "DE" => "德国",
        "IT" => "意大利",
        "ES" => "西班牙",
        "PT" => "葡萄牙",
        "NL" => "荷兰",
        "BE" => "比利时",
        "CH" => "瑞士",
        "AT" => "奥地利",
        "IE" => "爱尔兰",
        "SE" => "瑞典",
        "NO" => "挪威",
        "DK" => "丹麦",
        "FI" => "芬兰",
        "IS" => "冰岛",
        "PL" => "波兰",
        "CZ" => "捷克",
        "SK" => "斯洛伐克",
        "HU" => "匈牙利",
        "RO" => "罗马尼亚",
        "BG" => "保加利亚",
        "GR" => "希腊",
        "HR" => "克罗地亚",
        "SI" => "斯洛文尼亚",
        "RS" => "塞尔维亚",
        "BA" => "波黑",
        "ME" => "黑山",
        "MK" => "北马其顿",
        "AL" => "阿尔巴尼亚",
        "EE" => "爱沙尼亚",
        "LV" => "拉脱维亚",
        "LT" => "立陶宛",
        "LU" => "卢森堡",
        "MT" => "马耳他",
        "CY" => "塞浦路斯",
        "UA" => "乌克兰",
        "BY" => "白俄罗斯",
        "MD" => "摩尔多瓦",
        "RU" => "俄罗斯",
        "TR" => "土耳其",
        "AD" => "安道尔",
        "LI" => "列支敦士登",
        "MC" => "摩纳哥",
        "SM" => "圣马力诺",
        "VA" => "梵蒂冈",
        "CA" => "加拿大",
        "AU" => "澳大利亚",
        "NZ" => "新西兰",
        "IN" => "印度",
        "ID" => "印度尼西亚",
        "MY" => "马来西亚",
        "SG" => "新加坡",
        "PH" => "菲律宾",
        "VN" => "越南",
        "BR" => "巴西",
        "MX" => "墨西哥",
        _ => region,
    }
}
fn platform_label(platform: &str) -> &str {
    match platform {
        "bilibili" => "哔哩哔哩",
        "tencent" => "腾讯视频",
        "iqiyi" => "爱奇艺",
        "youku" => "优酷",
        "mgtv" => "芒果TV",
        "netflix" => "奈飞",
        "disney_plus" => "Disney+",
        _ => platform,
    }
}
/// 旧行里可能还存着英文类型；显示时翻一次，不回写历史数据。
fn genre_label(genre: &str) -> &str {
    super::genres::localize_genre(genre).unwrap_or(genre)
}
fn facet_counts(
    conn: &Connection,
    sql: &str,
    values: &[Value],
    label: fn(&str) -> &str,
) -> Result<Vec<CatalogFacetCount>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params_from_iter(values.iter()), |r| {
        let key: String = r.get(0)?;
        Ok(CatalogFacetCount {
            label: label(&key).into(),
            key,
            count: r.get(1)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}
pub fn facets(db: &Db, q: &CatalogQuery) -> Result<CatalogFacets> {
    db.read(|conn| {
        let (filter, values) = facet_filters(q, "kind");
        let kinds = facet_counts(
            conn,
            &format!("SELECT m.kind,COUNT(*) FROM media m WHERE {filter} GROUP BY m.kind ORDER BY COUNT(*) DESC,m.kind"),
            &values,
            kind_label,
        )?;
        let (filter, values) = facet_filters(q, "region");
        let regions = facet_counts(
            conn,
            &format!("SELECT x.region,COUNT(DISTINCT x.media_key) FROM media_regions x JOIN media m ON m.key=x.media_key WHERE {filter} GROUP BY x.region ORDER BY COUNT(DISTINCT x.media_key) DESC,x.region"),
            &values,
            region_label,
        )?;
        let (filter, values) = facet_filters(q, "genre");
        let genres = facet_counts(
            conn,
            &format!("SELECT x.genre,COUNT(DISTINCT x.media_key) FROM media_genres x JOIN media m ON m.key=x.media_key WHERE {filter} GROUP BY x.genre ORDER BY COUNT(DISTINCT x.media_key) DESC,x.genre"),
            &values,
            genre_label,
        )?;
        let (filter, values) = facet_filters(q, "platform");
        let platforms = facet_counts(
            conn,
            &format!("SELECT x.platform,COUNT(DISTINCT x.media_key) FROM media_platforms x JOIN media m ON m.key=x.media_key WHERE {filter} GROUP BY x.platform ORDER BY COUNT(DISTINCT x.media_key) DESC,x.platform"),
            &values,
            platform_label,
        )?;
        let (filter, values) = facet_filters(q, "year");
        let mut stmt = conn.prepare(&format!(
            "SELECT DISTINCT m.year FROM media m WHERE {filter} AND m.year IS NOT NULL ORDER BY m.year DESC",
        ))?;
        let years = stmt
            .query_map(params_from_iter(values.iter()), |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(CatalogFacets { kinds, regions, genres, platforms, years })
    })
}

pub fn overview(db: &Db, today: &str, week_start: &str, week_end: &str, scope: Option<&str>) -> Result<CatalogOverview> {
    if !valid_date(today) || !valid_date(week_start) || !valid_date(week_end) {
        return Err(invalid("概览日期格式应为有效的 YYYY-MM-DD"));
    }
    db.read(|conn| {
        // 国内/国外页签各看各的概览：scope 子句打到每一个统计与推荐位上
        let (scope_clause, scope_values): (String, Vec<Value>) = match scope.filter(|s| matches!(*s, "domestic" | "foreign")) {
            Some(scope) => {
                let sources = super::sources::scope_sources(scope);
                let placeholders = sources.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                (
                    format!(
                        " AND EXISTS(SELECT 1 FROM media_external_ids e WHERE e.media_key=m.key AND e.source IN ({placeholders}))"
                    ),
                    sources.iter().map(|s| (*s).to_string().into()).collect(),
                )
            }
            None => (String::new(), Vec::new()),
        };
        let scoped = |sql: &str| -> String { sql.replace("{scope}", &scope_clause) };
        let bind = |input: (&str, Vec<Value>)| -> (String, Vec<Value>) {
            (scoped(input.0), {
                let mut all = scope_values.clone();
                all.extend(input.1);
                all
            })
        };
        let (sql, values) = bind(("SELECT COUNT(*) FROM media m WHERE 1=1{scope}", vec![]));
        let total = conn.query_row(&sql, params_from_iter(values.iter()), |r| r.get(0))?;
        let (sql, values) = bind(("SELECT COUNT(*) FROM media m WHERE substr(first_seen,1,10)=?1{scope}", vec![today.to_string().into()]));
        let today_added = conn.query_row(&sql, params_from_iter(values.iter()), |r| r.get(0))?;
        let (sql, values) = bind(("SELECT COUNT(*) FROM media m WHERE substr(first_seen,1,10)>=?1 AND substr(first_seen,1,10)<?2{scope}", vec![week_start.to_string().into(), week_end.to_string().into()]));
        let week_added = conn.query_row(&sql, params_from_iter(values.iter()), |r| r.get(0))?;
        let (sql, values) = bind((
            "SELECT COUNT(*) FROM release_schedules r JOIN media m ON m.key=r.media_key WHERE r.date=?1{scope}",
            vec![today.to_string().into()],
        ));
        let today_releases = conn.query_row(&sql, params_from_iter(values.iter()), |r| r.get(0))?;
        let (sql, values) = bind((
            "SELECT COUNT(DISTINCT r.media_key) FROM release_schedules r JOIN media m ON m.key=r.media_key WHERE r.date>=?1 AND r.date<?2{scope}",
            vec![week_start.to_string().into(), week_end.to_string().into()],
        ));
        let week_updated = conn.query_row(&sql, params_from_iter(values.iter()), |r| r.get(0))?;
        let (sql, values) = bind((
            "SELECT m.payload FROM media m WHERE (m.premiered>?1
                OR EXISTS(SELECT 1 FROM release_schedules r WHERE r.media_key=m.key AND r.date>?1)){scope}
             ORDER BY COALESCE((SELECT MIN(r.date) FROM release_schedules r WHERE r.media_key=m.key AND r.date>?1),
                CASE WHEN m.premiered>?1 THEN m.premiered END),m.key LIMIT 12",
            vec![today.to_string().into()],
        ));
        let upcoming = query_media(conn, &sql, &values)?;
        // Allocate an independent lane per kind so a popular anime source cannot
        // crowd all TV/variety entries out of the latest list.
        let (sql, values) = bind((
            "WITH latest AS (
                SELECT m.payload,m.key,m.kind,COALESCE((SELECT MAX(r.date) FROM release_schedules r
                    WHERE r.media_key=m.key AND r.date<=?1),CASE WHEN m.premiered<=?1 THEN m.premiered END,
                    substr(m.first_seen,1,10)) AS activity,
                    ROW_NUMBER() OVER(PARTITION BY m.kind ORDER BY COALESCE(
                        (SELECT MAX(r.date) FROM release_schedules r WHERE r.media_key=m.key AND r.date<=?1),
                        CASE WHEN m.premiered<=?1 THEN m.premiered END,substr(m.first_seen,1,10)) DESC,m.key) AS lane
                FROM media m WHERE m.kind IN ('anime','tv','variety')
                    AND (m.premiered IS NULL OR m.premiered<=?1){scope})
             SELECT payload FROM latest WHERE lane<=4 ORDER BY activity DESC,kind,key",
            vec![today.to_string().into()],
        ));
        let latest = query_media(conn, &sql, &values)?;
        let (sql, values) = bind((
            "SELECT m.payload FROM media m WHERE m.rating IS NOT NULL{scope} ORDER BY m.rating DESC,m.key LIMIT 12",
            vec![],
        ));
        let highly_rated = query_media(conn, &sql, &values)?;
        Ok(CatalogOverview { total, today_added, today_releases, week_added, week_updated, upcoming, latest, highly_rated })
    })
}

pub fn count_all(db: &Db) -> Result<i64> {
    db.read(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM media", [], |r| r.get(0))?))
}
pub fn count_source(db: &Db, source: &str) -> Result<i64> {
    db.read(|conn| {
        Ok(conn.query_row(
            "SELECT COUNT(DISTINCT media_key) FROM media_external_ids WHERE source=?1",
            [source],
            |r| r.get(0),
        )?)
    })
}
fn insert_source(conn: &Connection, source: &str, enabled: bool) -> Result<()> {
    if source.trim().is_empty() {
        return Err(invalid("数据来源不能为空"));
    }
    conn.execute(
        "INSERT INTO data_sources(source,enabled) VALUES (?1,?2) ON CONFLICT DO NOTHING",
        params![source, enabled],
    )?;
    Ok(())
}
pub fn ensure_source(db: &Db, source: &str, enabled: bool) -> Result<()> {
    db.write(|conn| insert_source(conn, source, enabled))
}
pub fn source_enabled(db: &Db, source: &str) -> Result<bool> {
    db.read(|conn| {
        Ok(conn
            .query_row(
                "SELECT enabled FROM data_sources WHERE source=?1",
                [source],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
    })
}
pub fn set_source_enabled(db: &Db, source: &str, enabled: bool) -> Result<()> {
    db.write(|conn| {
        let tx = conn.unchecked_transaction()?;
        insert_source(&tx, source, enabled)?;
        tx.execute(
            "UPDATE data_sources SET enabled=?2,updated_at=?3 WHERE source=?1",
            params![source, enabled, crate::app::timeutil::now_ms()],
        )?;
        tx.commit()?;
        Ok(())
    })
}
pub fn source_key(db: &Db, source: &str) -> Result<Option<Vec<u8>>> {
    db.read(|conn| {
        Ok(conn
            .query_row(
                "SELECT key_blob FROM data_sources WHERE source=?1",
                [source],
                |r| r.get::<_, Option<Vec<u8>>>(0),
            )
            .optional()?
            .flatten())
    })
}
pub fn set_source_key(db: &Db, source: &str, blob: Option<&[u8]>) -> Result<()> {
    db.write(|conn| {
        let tx = conn.unchecked_transaction()?;
        insert_source(&tx, source, true)?;
        tx.execute(
            "UPDATE data_sources SET key_blob=?2,updated_at=?3 WHERE source=?1",
            params![source, blob, crate::app::timeutil::now_ms()],
        )?;
        tx.commit()?;
        Ok(())
    })
}
pub fn source_state(db: &Db, source: &str) -> Result<Option<CatalogSourceStatus>> {
    db.read(|conn| {
        let row = conn
            .query_row(
                "SELECT state_json,enabled,key_blob IS NOT NULL FROM data_sources WHERE source=?1",
                [source],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, bool>(1)?,
                        r.get::<_, bool>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((Some(raw), enabled, has_key)) = row else {
            return Ok(None);
        };
        let mut state: CatalogSourceStatus = serde_json::from_str(&raw)?;
        state.enabled = enabled;
        state.item_count = conn.query_row(
            "SELECT COUNT(DISTINCT media_key) FROM media_external_ids WHERE source=?1",
            [source],
            |r| r.get(0),
        )?;
        if state.requires_key {
            state.configured = has_key;
        }
        Ok(Some(state))
    })
}

// 保留服务层的脱敏诊断原文以便定位失败，仅对敏感参数值兜底打码并限长；
// 无错误/警告时保持 None，绝不凭空生成提示，也不把有用错误替换成通用文案。
fn safe_diagnostic(text: Option<&str>, _fallback: &str) -> Option<String> {
    let text = text?.trim();
    if text.is_empty() {
        return None;
    }
    Some(redact_secrets(text).chars().take(600).collect())
}

fn redact_secrets(text: &str) -> String {
    const KEYS: &[&str] = &[
        "access_token=", "apikey=", "api_key=", "token=", "password=", "passwd=",
        "secret=", "cookie=", "authorization=",
    ];
    let lower = text.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < text.len() {
        let mut hit: Option<(usize, usize)> = None;
        for key in KEYS {
            if let Some(pos) = lower[i..].find(key) {
                let start = i + pos;
                let boundary = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
                if boundary && hit.is_none_or(|(s, _)| start < s) {
                    hit = Some((start, key.len()));
                }
            }
        }
        match hit {
            Some((start, key_len)) => {
                out.push_str(&text[i..start + key_len]);
                let value_start = start + key_len;
                let value_end = text[value_start..]
                    .find(|c: char| c.is_whitespace() || matches!(c, '&' | '"' | '\'' | ',' | ';' | ')' | '}'))
                    .map(|p| value_start + p)
                    .unwrap_or(text.len());
                out.push_str("***");
                i = value_end;
            }
            None => {
                out.push_str(&text[i..]);
                break;
            }
        }
    }
    out
}
pub fn set_source_state(db: &Db, state: &CatalogSourceStatus) -> Result<()> {
    db.write(|conn| {
        let tx = conn.unchecked_transaction()?;
        insert_source(&tx, &state.source, state.enabled)?;
        let (enabled, has_key) = tx.query_row(
            "SELECT enabled,key_blob IS NOT NULL FROM data_sources WHERE source=?1",
            [&state.source],
            |r| Ok((r.get::<_, bool>(0)?, r.get::<_, bool>(1)?)),
        )?;
        let mut state = state.clone();
        state.enabled = enabled;
        if state.requires_key {
            state.configured = has_key;
        }
        state.error = safe_diagnostic(
            state.error.as_deref(),
            "来源同步失败；请根据 HTTP 状态与进度检查来源设置",
        );
        state.warning = safe_diagnostic(state.warning.as_deref(), "来源报告数据覆盖或分页警告");
        tx.execute(
            "UPDATE data_sources SET state_json=?2,updated_at=?3 WHERE source=?1",
            params![
                state.source,
                serde_json::to_string(&state)?,
                crate::app::timeutil::now_ms()
            ],
        )?;
        tx.commit()?;
        Ok(())
    })
}
pub fn save_job(db: &Db, job: &CatalogJob) -> Result<()> {
    if job.id.trim().is_empty() {
        return Err(invalid("采集任务标识不能为空"));
    }
    db.write(|conn| {
        let tx = conn.unchecked_transaction()?;
        insert_source(&tx, &job.source, true)?;
        let mut job = job.clone();
        job.error = safe_diagnostic(job.error.as_deref(), "来源同步失败；请检查 HTTP 状态与分页进度");
        job.warning = safe_diagnostic(job.warning.as_deref(), "来源报告数据覆盖或分页警告");
        let payload = serde_json::to_string(&job)?;
        let old: Option<String> = tx.query_row("SELECT payload FROM ingestion_jobs WHERE id=?1", [&job.id], |r| r.get(0)).optional()?;
        if old.as_ref() != Some(&payload) {
            tx.execute("INSERT INTO ingestion_jobs(id,source,started_at,finished_at,status,pages,payload)
                VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(id) DO UPDATE SET source=excluded.source,
                    started_at=excluded.started_at,finished_at=excluded.finished_at,status=excluded.status,
                    pages=excluded.pages,payload=excluded.payload",
                params![job.id, job.source, job.started_at, job.finished_at, job.status, job.pages, payload])?;
            tx.execute("INSERT INTO ingestion_logs(job_id,source,page,status,http_status,failed,error,warning,logged_at)
                VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![job.id, job.source, job.pages, job.status, job.http_status, job.failed,
                    job.error, job.warning, crate::app::timeutil::now_ms()])?;
        }
        tx.commit()?; Ok(())
    })
}
pub fn jobs(db: &Db) -> Result<Vec<CatalogJob>> {
    db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT payload FROM ingestion_jobs ORDER BY started_at DESC,id DESC LIMIT 100",
        )?;
        let rows = stmt.query_map([], |r| json_row(r, 0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    })
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
