//! Global media metadata DAL. Source identities, ratings and individual releases are facts;
//! a missing item in a later sync is not a deletion or a change in watching state.

use super::types::*;
use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use rusqlite::{params, params_from_iter, types::Value, Connection, OptionalExtension, Row};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const LEGACY_MARKER: &str = "catalog.media.legacy.v1";
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
    migrate_legacy(&tx)?;
    tx.commit()?;
    Ok(())
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
fn valid_date(date: &str) -> bool {
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
                           first_seen,updated_at,search_text,payload,authority_json)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
         ON CONFLICT(key) DO UPDATE SET title=excluded.title,kind=excluded.kind,status=excluded.status,
             year=excluded.year,premiered=excluded.premiered,rating=excluded.rating,
             rating_source=excluded.rating_source,first_seen=excluded.first_seen,updated_at=excluded.updated_at,
             search_text=excluded.search_text,payload=excluded.payload,authority_json=excluded.authority_json",
        params![media.key, media.title, media.kind, media.status, media.year, media.premiered,
            filter_rating, media.rating_source, media.source, media.first_seen, media.updated_at,
            search_text(media), serde_json::to_string(media)?, serde_json::to_string(authorities)?],
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
    Ok(())
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
    let key = resolved.into_iter().next().unwrap_or(own_key);
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
    let incoming = clean_media(draft.media.clone());
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
fn media_filters(q: &CatalogQuery) -> (String, Vec<Value>) {
    let mut clauses = vec!["1=1".to_string()];
    let mut values = Vec::new();
    let mut push = |clause: &str, value: Value| {
        clauses.push(clause.into());
        values.push(value);
    };
    if let Some(search) = q.search.as_deref().filter(|s| !s.trim().is_empty()) {
        push(
            "m.search_text LIKE ? ESCAPE '\\' COLLATE NOCASE",
            literal_like(search).into(),
        );
    }
    for (value, sql) in [
        (&q.kind, "m.kind=?"),
        (&q.status, "m.status=?"),
        (
            &q.region,
            "EXISTS(SELECT 1 FROM media_regions f WHERE f.media_key=m.key AND f.region=?)",
        ),
        (
            &q.genre,
            "EXISTS(SELECT 1 FROM media_genres f WHERE f.media_key=m.key AND f.genre=?)",
        ),
        (
            &q.platform,
            "EXISTS(SELECT 1 FROM media_platforms f WHERE f.media_key=m.key AND f.platform=?)",
        ),
    ] {
        if let Some(value) = value.as_ref().filter(|s| !s.is_empty()) {
            push(sql, value.clone().into());
        }
    }
    if let Some(year) = q.year {
        push("m.year=?", i64::from(year).into());
    }
    if let Some(rating) = q.min_rating {
        push("m.rating>=?", rating.into());
    }
    (clauses.join(" AND "), values)
}
fn media_order(sort: Option<&str>) -> &'static str {
    match sort {
        Some("title") => "m.title COLLATE NOCASE,m.key",
        Some("year") => "m.year IS NULL,m.year DESC,m.title COLLATE NOCASE,m.key",
        Some("rating") => "m.rating IS NULL,m.rating DESC,m.title COLLATE NOCASE,m.key",
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
        Ok(Some(CatalogDetail { media, external_ids, ratings, seasons, releases }))
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
    label: fn(&str) -> &str,
) -> Result<Vec<CatalogFacetCount>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |r| {
        let key: String = r.get(0)?;
        Ok(CatalogFacetCount {
            label: label(&key).into(),
            key,
            count: r.get(1)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}
pub fn facets(db: &Db) -> Result<CatalogFacets> {
    db.read(|conn| {
        let kinds = facet_counts(conn, "SELECT kind,COUNT(*) FROM media GROUP BY kind ORDER BY COUNT(*) DESC,kind", kind_label)?;
        let regions = facet_counts(conn, "SELECT region,COUNT(DISTINCT media_key) FROM media_regions GROUP BY region ORDER BY COUNT(DISTINCT media_key) DESC,region", region_label)?;
        let genres = facet_counts(conn, "SELECT genre,COUNT(DISTINCT media_key) FROM media_genres GROUP BY genre ORDER BY COUNT(DISTINCT media_key) DESC,genre", genre_label)?;
        let platforms = facet_counts(conn, "SELECT platform,COUNT(DISTINCT media_key) FROM media_platforms GROUP BY platform ORDER BY COUNT(DISTINCT media_key) DESC,platform", platform_label)?;
        let mut stmt = conn.prepare("SELECT DISTINCT year FROM media WHERE year IS NOT NULL ORDER BY year DESC")?;
        let years = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(CatalogFacets { kinds, regions, genres, platforms, years })
    })
}

pub fn overview(db: &Db, today: &str, week_start: &str, week_end: &str) -> Result<CatalogOverview> {
    if !valid_date(today) || !valid_date(week_start) || !valid_date(week_end) {
        return Err(invalid("概览日期格式应为有效的 YYYY-MM-DD"));
    }
    db.read(|conn| {
        let total = conn.query_row("SELECT COUNT(*) FROM media", [], |r| r.get(0))?;
        let today_added = conn.query_row("SELECT COUNT(*) FROM media WHERE substr(first_seen,1,10)=?1", [today], |r| r.get(0))?;
        let week_added = conn.query_row("SELECT COUNT(*) FROM media WHERE substr(first_seen,1,10)>=?1 AND substr(first_seen,1,10)<?2",
            params![week_start, week_end], |r| r.get(0))?;
        let today_releases = conn.query_row("SELECT COUNT(*) FROM release_schedules WHERE date=?1", [today], |r| r.get(0))?;
        let week_updated = conn.query_row("SELECT COUNT(DISTINCT media_key) FROM release_schedules WHERE date>=?1 AND date<?2",
            params![week_start, week_end], |r| r.get(0))?;
        let upcoming = query_media(conn,
            "SELECT m.payload FROM media m WHERE m.premiered>?1
                OR EXISTS(SELECT 1 FROM release_schedules r WHERE r.media_key=m.key AND r.date>?1)
             ORDER BY COALESCE((SELECT MIN(r.date) FROM release_schedules r WHERE r.media_key=m.key AND r.date>?1),
                CASE WHEN m.premiered>?1 THEN m.premiered END),m.key LIMIT 12", &[today.to_string().into()])?;
        // Allocate an independent lane per kind so a popular anime source cannot
        // crowd all TV/variety entries out of the latest list.
        let latest = query_media(conn,
            "WITH latest AS (
                SELECT m.payload,m.key,m.kind,COALESCE((SELECT MAX(r.date) FROM release_schedules r
                    WHERE r.media_key=m.key AND r.date<=?1),CASE WHEN m.premiered<=?1 THEN m.premiered END,
                    substr(m.first_seen,1,10)) AS activity,
                    ROW_NUMBER() OVER(PARTITION BY m.kind ORDER BY COALESCE(
                        (SELECT MAX(r.date) FROM release_schedules r WHERE r.media_key=m.key AND r.date<=?1),
                        CASE WHEN m.premiered<=?1 THEN m.premiered END,substr(m.first_seen,1,10)) DESC,m.key) AS lane
                FROM media m WHERE m.kind IN ('anime','tv','variety')
                    AND (m.premiered IS NULL OR m.premiered<=?1))
             SELECT payload FROM latest WHERE lane<=4 ORDER BY activity DESC,kind,key", &[today.to_string().into()])?;
        let highly_rated = query_media(conn,
            "SELECT m.payload FROM media m WHERE m.rating IS NOT NULL ORDER BY m.rating DESC,m.key LIMIT 12", &[])?;
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
