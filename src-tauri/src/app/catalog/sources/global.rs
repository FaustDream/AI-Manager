use super::{json, non_empty, pace, publish, strip_html, text, AdapterContext};
use crate::app::catalog::types::{
    CatalogExternalId, CatalogMedia, CatalogRating, CatalogRelease, IngestionBatch, MediaDraft,
};
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::HttpRequest;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use tokio::sync::mpsc::Sender;

const ANILIST_URL: &str = "https://graphql.anilist.co";
const MANGADEX_PAGE_SIZE: u64 = 100;
// This is a source API restriction, not a configurable resource/completeness limit.
const MANGADEX_OFFSET_LIMIT: u64 = 10_000;
const TVMAZE_NOTE: &str = "仅覆盖所选日期、国家电视日历与全球网络日历；TVmaze 台站国家是发行国家，缺少可靠制作国家时地区留空。";
const ANILIST_NOTE: &str = "动漫仅覆盖所选 UTC 播出范围；漫画仅当前非成人连载目录，不是历史全库，也不把元数据更新时间当作章节日期。播出日历不支持成人过滤参数，返回的成人条目不入库。";
const MANGADEX_NOTE: &str = "章节日期是 MangaDex 译本/平台发布时间（UTC）；目录仅覆盖所选更新时间范围内的当前安全/建议级元数据，不重建历史快照。API 无 publishAtUntil，按时间升序在排他结束边界停止。";
const OFFSET_NOTE: &str = "MangaDex 已到达 offset + limit ≤ 10000 的 API 限制，已保留未完成游标；同范围重试或增加预算不能越过限制，须缩小日期范围重新采集。";

#[derive(Debug, Serialize, Deserialize)]
struct Cursor {
    version: u8,
    source: String,
    start: NaiveDate,
    end: NaiveDate,
    mode: String,
    countries: Vec<String>,
    phase: String,
    next: u64,
}

impl Cursor {
    fn load(ctx: &AdapterContext, source: &str, phase: &str, next: u64) -> Result<Self> {
        if ctx.start >= ctx.end {
            return Err(AppError::new(ErrorKind::Invalid, "采集日期范围必须为开始日包含、结束日不包含的非空范围"));
        }
        let countries = if source == "tvmaze" { ctx.countries.clone() } else { Vec::new() };
        if let Some(raw) = ctx.cursor.as_deref() {
            let saved: Self = serde_json::from_str(raw)
                .map_err(|_| AppError::new(ErrorKind::Invalid, "采集游标格式无效，请重新启动采集范围"))?;
            if saved.version == 1 && saved.source == source && saved.start == ctx.start
                && saved.end == ctx.end && saved.mode == ctx.mode && saved.countries == countries
                && saved.phase != "done"
            {
                return Ok(saved);
            }
        }
        Ok(Self {
            version: 1, source: source.into(), start: ctx.start, end: ctx.end,
            mode: ctx.mode.clone(), countries, phase: phase.into(), next,
        })
    }

    fn encode(&self) -> Result<String> {
        serde_json::to_string(self)
            .map_err(|_| AppError::new(ErrorKind::Invalid, "无法保存采集游标"))
    }
}

fn payload_error(source: &str, field: &str) -> AppError {
    // Do not echo source bodies or GraphQL error messages: they may contain sensitive data.
    AppError::new(ErrorKind::Provider, format!("{source} 返回资料缺少或包含无效的 {field} 字段"))
}

fn required_text(value: &Value, source: &str, field: &str) -> Result<String> {
    text(value).ok_or_else(|| payload_error(source, field))
}

fn required_array<'a>(value: &'a Value, source: &str, field: &str) -> Result<&'a [Value]> {
    value.as_array().map(Vec::as_slice).ok_or_else(|| payload_error(source, field))
}

fn number(value: &Value) -> Option<u32> {
    value.as_u64().and_then(|n| u32::try_from(n).ok())
}

fn append_unique(items: &mut Vec<String>, value: Option<String>) {
    if let Some(value) = value {
        if !items.contains(&value) { items.push(value); }
    }
}

fn strings(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(values) = value.as_array() {
        for value in values { append_unique(&mut out, text(value)); }
    }
    out
}

fn range_epochs(ctx: &AdapterContext) -> (i64, i64) {
    // A valid NaiveDate always has a midnight; no local/production timezone is inferred.
    (ctx.start.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp(),
     ctx.end.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp())
}

fn timestamp(value: &Value, source: &str, field: &str) -> Result<DateTime<Utc>> {
    let raw = required_text(value, source, field)?;
    DateTime::parse_from_rfc3339(&raw).map(|date| date.with_timezone(&Utc))
        .map_err(|_| payload_error(source, field))
}

fn utc_text(date: DateTime<Utc>) -> String {
    date.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

async fn send_page(
    ctx: &AdapterContext, tx: &Sender<IngestionBatch>, cursor: &Cursor,
    items: Vec<MediaDraft>, page: u32, complete: bool, note: &str,
) -> Result<()> {
    let warning = if !complete && page >= ctx.max_pages {
        format!("{note} 本次达到分页预算，尚未完成，已保留游标；若未同时遇到源 API 上限，以相同范围续采即可继续。")
    } else { note.to_string() };
    publish(tx, IngestionBatch {
        items, page, cursor: Some(cursor.encode()?), complete,
        http_status: if page == 0 { None } else { Some(200) }, warning: Some(warning),
    }).await
}

pub async fn tvmaze(ctx: AdapterContext, tx: Sender<IngestionBatch>) -> Result<()> {
    let mut cursor = Cursor::load(&ctx, "tvmaze", "schedule", 0)?;
    if cursor.phase != "schedule" || ctx.countries.iter().any(|country| {
        country.len() != 2 || !country.bytes().all(|b| b.is_ascii_uppercase())
    }) {
        return Err(AppError::new(ErrorKind::Invalid, "TVmaze 国家配置或采集游标无效"));
    }
    let queries_per_day = ctx.countries.len() as u64 + 1;
    let steps = (ctx.end - ctx.start).num_days() as u64 * queries_per_day;
    if cursor.next >= steps {
        return Err(AppError::new(ErrorKind::Invalid, "TVmaze 采集游标超出日期范围"));
    }
    if ctx.max_pages == 0 {
        return send_page(&ctx, &tx, &cursor, Vec::new(), 0, false, TVMAZE_NOTE).await;
    }
    for page in 1..=ctx.max_pages {
        let date = ctx.start + chrono::Duration::days((cursor.next / queries_per_day) as i64);
        let country_step = (cursor.next % queries_per_day) as usize;
        let web = country_step == ctx.countries.len();
        let url = if web {
            format!("https://api.tvmaze.com/schedule/web?date={date}")
        } else {
            format!("https://api.tvmaze.com/schedule?country={}&date={date}", ctx.countries[country_step])
        };
        pace().await;
        let payload = json(&ctx.http, HttpRequest::get(url)).await?;
        let episodes = required_array(&payload, "TVmaze", "schedule")?;
        let mut items = Vec::with_capacity(episodes.len());
        for episode in episodes {
            let draft = tvmaze_episode(episode, web)?;
            let date = NaiveDate::parse_from_str(&draft.releases[0].date, "%Y-%m-%d")
                .map_err(|_| payload_error("TVmaze", "airdate"))?;
            if date >= ctx.start && date < ctx.end { items.push(draft); }
        }
        cursor.next += 1;
        let complete = cursor.next == steps;
        if complete { cursor.phase = "done".into(); }
        send_page(&ctx, &tx, &cursor, items, page, complete, TVMAZE_NOTE).await?;
        if complete { return Ok(()); }
    }
    Ok(())
}

fn tvmaze_episode(episode: &Value, web: bool) -> Result<MediaDraft> {
    let show = if web { &episode["_embedded"]["show"] } else { &episode["show"] };
    let id = required_text(&show["id"], "TVmaze", "show.id")?;
    let episode_id = required_text(&episode["id"], "TVmaze", "episode.id")?;
    let title = required_text(&show["name"], "TVmaze", "show.name")?;
    let show_type = required_text(&show["type"], "TVmaze", "show.type")?;
    let date = required_text(&episode["airdate"], "TVmaze", "airdate")?;
    NaiveDate::parse_from_str(&date, "%Y-%m-%d").map_err(|_| payload_error("TVmaze", "airdate"))?;
    let kind = match show_type.as_str() {
        "Animation" => "anime",
        "Reality" | "Game Show" | "Talk Show" => "variety",
        "Documentary" => "documentary",
        "Scripted" => "tv",
        _ => "other",
    };
    let channel = if web && show["webChannel"].is_object() { &show["webChannel"] }
        else if show["network"].is_object() { &show["network"] }
        else { &show["webChannel"] };
    let mut platforms = Vec::new();
    append_unique(&mut platforms, text(&show["network"]["name"]));
    append_unique(&mut platforms, text(&show["webChannel"]["name"]));
    let rating = show["rating"]["average"].as_f64();
    let source_url = text(&show["url"]);
    let premiered = text(&show["premiered"]);
    let mut draft = MediaDraft {
        source: "tvmaze".into(), external_id: id.clone(),
        media: CatalogMedia {
            key: format!("tvmaze:{id}"), source: "tvmaze".into(), title,
            kind: kind.into(), subkind: Some(show_type), genres: strings(&show["genres"]),
            poster: text(&show["image"]["original"]).or_else(|| text(&show["image"]["medium"])),
            summary: text(&show["summary"]).and_then(|s| non_empty(Some(&strip_html(&s)))),
            distributors: platforms.clone(), platforms,
            status: match show["status"].as_str() {
                Some("Running") => "airing", Some("Ended") => "completed",
                Some("In Development") => "upcoming", _ => "unknown",
            }.into(),
            year: premiered.as_deref().and_then(|s| s.get(..4)).and_then(|s| s.parse().ok()),
            premiered, ended: text(&show["ended"]),
            runtime: number(&show["averageRuntime"]).or_else(|| number(&show["runtime"])),
            rating, rating_source: rating.map(|_| "tvmaze".into()), source_url: source_url.clone(),
            ..Default::default()
        }, ..Default::default()
    };
    for (field, source) in [("imdb", "imdb"), ("thetvdb", "tvdb"), ("tvrage", "tvrage")] {
        if let Some(external_id) = text(&show["externals"][field]) {
            draft.external_ids.push(CatalogExternalId { source: source.into(), external_id });
        }
    }
    if let Some(value) = rating {
        draft.ratings.push(CatalogRating {
            source: "tvmaze".into(), category: "audience".into(), value: Some(value),
            scale: 10.0, url: source_url, ..Default::default()
        });
    }
    let utc_time = if episode["airstamp"].is_null() || episode["airstamp"] == "" { None }
        else { Some(utc_text(timestamp(&episode["airstamp"], "TVmaze", "airstamp")?)) };
    draft.releases.push(CatalogRelease {
        key: format!("tvmaze:{episode_id}"), media_key: format!("tvmaze:{id}"),
        source: "tvmaze".into(), external_id: episode_id, date,
        local_time: text(&episode["airtime"]), timezone: text(&channel["country"]["timezone"]),
        utc_time, season: number(&episode["season"]), episode: text(&episode["number"]),
        title: text(&episode["name"]), platform: text(&channel["name"]),
        release_kind: "episode".into(), url: text(&episode["url"]), ..Default::default()
    });
    Ok(draft)
}

const ANILIST_MEDIA_FIELDS: &str = r#"
    id idMal title { romaji english native } synonyms type format countryOfOrigin genres
    description isAdult status startDate { year month day } endDate { year month day }
    episodes chapters volumes duration averageScore popularity updatedAt siteUrl
    coverImage { large } bannerImage studios { nodes { name } }
    externalLinks { site url type } stats { scoreDistribution { score amount } }
"#;

fn anilist_request(phase: &str, page: u64, start: i64, end: i64) -> HttpRequest {
    let query = if phase == "airing" {
        format!(r#"query($page: Int!, $start: Int!, $end: Int!) {{
            Page(page: $page, perPage: 50) {{
                pageInfo {{ total hasNextPage lastPage }}
                airingSchedules(airingAt_greater: $start, airingAt_lesser: $end, sort: TIME) {{
                    id airingAt episode media {{ {ANILIST_MEDIA_FIELDS} }}
                }}
            }}
        }}"#)
    } else {
        format!(r#"query($page: Int!) {{
            Page(page: $page, perPage: 50) {{
                pageInfo {{ total hasNextPage lastPage }}
                media(type: MANGA, status: RELEASING, sort: UPDATED_AT_DESC, isAdult: false) {{
                    {ANILIST_MEDIA_FIELDS}
                }}
            }}
        }}"#)
    };
    let variables = if phase == "airing" {
        serde_json::json!({"page": page, "start": start - 1, "end": end})
    } else { serde_json::json!({"page": page}) };
    HttpRequest::post(ANILIST_URL).json_body(&serde_json::json!({"query": query, "variables": variables}))
}

pub async fn anilist(ctx: AdapterContext, tx: Sender<IngestionBatch>) -> Result<()> {
    let mut cursor = Cursor::load(&ctx, "anilist", "airing", 1)?;
    if !matches!(cursor.phase.as_str(), "airing" | "manga") || cursor.next == 0 || cursor.next > i32::MAX as u64 {
        return Err(AppError::new(ErrorKind::Invalid, "AniList 采集游标无效"));
    }
    let (start, end) = range_epochs(&ctx);
    if i32::try_from(start - 1).is_err() || i32::try_from(end).is_err() {
        return Err(AppError::new(ErrorKind::Invalid, "AniList 日期范围超过其 GraphQL Int 时间戳支持范围"));
    }
    if ctx.max_pages == 0 {
        return send_page(&ctx, &tx, &cursor, Vec::new(), 0, false, ANILIST_NOTE).await;
    }
    for page in 1..=ctx.max_pages {
        pace().await;
        let payload = json(&ctx.http, anilist_request(&cursor.phase, cursor.next, start, end)).await?;
        if !payload["errors"].is_null() && payload["errors"].as_array().is_none_or(|errors| !errors.is_empty()) {
            return Err(payload_error("AniList", "GraphQL errors"));
        }
        let data = &payload["data"]["Page"];
        let has_next = data["pageInfo"]["hasNextPage"].as_bool()
            .ok_or_else(|| payload_error("AniList", "pageInfo.hasNextPage"))?;
        let airing = cursor.phase == "airing";
        let rows = required_array(&data[if airing { "airingSchedules" } else { "media" }], "AniList", "Page items")?;
        if rows.is_empty() && has_next { return Err(payload_error("AniList", "nonempty next page")); }
        let mut items = Vec::with_capacity(rows.len());
        let mut before_start = false;
        let mut previous_update = None;
        for row in rows {
            let media = if airing { &row["media"] } else { row };
            let adult = media["isAdult"].as_bool().ok_or_else(|| payload_error("AniList", "isAdult"))?;
            let updated = if airing { None } else {
                let updated = media["updatedAt"].as_i64().ok_or_else(|| payload_error("AniList", "updatedAt"))?;
                if previous_update.is_some_and(|previous| updated > previous) {
                    return Err(payload_error("AniList", "UPDATED_AT_DESC order"));
                }
                previous_update = Some(updated);
                Some(updated)
            };
            if !airing && ctx.mode != "full" {
                if updated.is_some_and(|updated| updated < start) { before_start = true; continue; }
                if updated.is_some_and(|updated| updated >= end) { continue; }
            }
            if adult { continue; }
            let mut draft = anilist_media(media)?;
            if airing {
                if draft.media.kind != "anime" { return Err(payload_error("AniList", "airing media.type")); }
                let seconds = row["airingAt"].as_i64().ok_or_else(|| payload_error("AniList", "airingAt"))?;
                let date = DateTime::<Utc>::from_timestamp(seconds, 0).ok_or_else(|| payload_error("AniList", "airingAt"))?;
                let release_id = required_text(&row["id"], "AniList", "airingSchedule.id")?;
                let episode = required_text(&row["episode"], "AniList", "airingSchedule.episode")?;
                if seconds < start || seconds >= end { continue; }
                draft.releases.push(CatalogRelease {
                    key: format!("anilist:{release_id}"), media_key: draft.media.key.clone(),
                    source: "anilist".into(), external_id: release_id,
                    date: date.date_naive().to_string(), local_time: Some(date.format("%H:%M:%S").to_string()),
                    timezone: Some("UTC".into()), utc_time: Some(utc_text(date)), episode: Some(episode),
                    platform: Some("AniList".into()), release_kind: "episode".into(), ..Default::default()
                });
            } else if draft.media.kind != "manga" { return Err(payload_error("AniList", "manga media.type")); }
            items.push(draft);
        }
        let exhausted = !has_next || before_start;
        if exhausted {
            if airing { cursor.phase = "manga".into(); cursor.next = 1; }
            else { cursor.phase = "done".into(); cursor.next = 0; }
        } else {
            cursor.next = cursor.next.checked_add(1).ok_or_else(|| payload_error("AniList", "page cursor"))?;
        }
        let complete = cursor.phase == "done";
        send_page(&ctx, &tx, &cursor, items, page, complete, ANILIST_NOTE).await?;
        if complete { return Ok(()); }
    }
    Ok(())
}

fn fuzzy_date(value: &Value) -> Option<String> {
    let year = i32::try_from(value["year"].as_i64()?).ok()?;
    NaiveDate::from_ymd_opt(year, number(&value["month"])?, number(&value["day"])?).map(|d| d.to_string())
}

fn anilist_media(value: &Value) -> Result<MediaDraft> {
    let id = required_text(&value["id"], "AniList", "media.id")?;
    let native = text(&value["title"]["native"]);
    let english = text(&value["title"]["english"]);
    let romaji = text(&value["title"]["romaji"]);
    let title = native.clone().or_else(|| english.clone()).or_else(|| romaji.clone())
        .ok_or_else(|| payload_error("AniList", "media.title"))?;
    let kind = match value["type"].as_str() {
        Some("ANIME") => "anime", Some("MANGA") => "manga", _ => return Err(payload_error("AniList", "media.type")),
    };
    let mut aliases = strings(&value["synonyms"]);
    for name in [native.clone(), romaji, english.clone()] { append_unique(&mut aliases, name); }
    let score = value["averageScore"].as_f64();
    let mut studios = Vec::new();
    if let Some(nodes) = value["studios"]["nodes"].as_array() {
        for node in nodes { append_unique(&mut studios, text(&node["name"])); }
    }
    let mut platforms = Vec::new();
    if let Some(links) = value["externalLinks"].as_array() {
        for link in links {
            if link["type"] == "STREAMING" { append_unique(&mut platforms, text(&link["site"])); }
        }
    }
    let source_url = text(&value["siteUrl"]);
    let mut draft = MediaDraft {
        source: "anilist".into(), external_id: id.clone(),
        media: CatalogMedia {
            key: format!("anilist:{id}"), source: "anilist".into(), title,
            original_title: native, english_title: english, aliases, kind: kind.into(),
            subkind: text(&value["format"]), regions: text(&value["countryOfOrigin"]).into_iter().collect(),
            genres: strings(&value["genres"]), poster: text(&value["coverImage"]["large"]),
            backdrop: text(&value["bannerImage"]),
            summary: text(&value["description"]).and_then(|s| non_empty(Some(&strip_html(&s)))),
            studios, platforms, status: match value["status"].as_str() {
                Some("RELEASING") => "airing", Some("FINISHED") => "completed",
                Some("NOT_YET_RELEASED") => "upcoming", Some("HIATUS") => "hiatus",
                Some("CANCELLED") => "cancelled", _ => "unknown",
            }.into(),
            year: value["startDate"]["year"].as_i64().and_then(|year| i32::try_from(year).ok()),
            premiered: fuzzy_date(&value["startDate"]), ended: fuzzy_date(&value["endDate"]),
            episode_count: if kind == "anime" { number(&value["episodes"]) } else { None },
            chapter_count: if kind == "manga" { number(&value["chapters"]) } else { None },
            runtime: number(&value["duration"]), rating: score.map(|score| score / 10.0),
            rating_source: score.map(|_| "anilist".into()), source_url: source_url.clone(),
            ..Default::default()
        }, ..Default::default()
    };
    if let Some(mal) = value["idMal"].as_u64().filter(|id| *id > 0) {
        draft.external_ids.push(CatalogExternalId { source: "mal".into(), external_id: mal.to_string() });
    }
    if let Some(score) = score {
        let votes = if value["stats"].is_null() { None } else {
            let distribution = required_array(&value["stats"]["scoreDistribution"], "AniList", "stats.scoreDistribution")?;
            let mut sum = 0_u64;
            for bucket in distribution {
                let amount = bucket["amount"].as_u64().ok_or_else(|| payload_error("AniList", "scoreDistribution.amount"))?;
                sum = sum.checked_add(amount).ok_or_else(|| payload_error("AniList", "scoreDistribution.amount"))?;
            }
            Some(u32::try_from(sum).map_err(|_| payload_error("AniList", "scoreDistribution.amount"))?)
        };
        draft.ratings.push(CatalogRating {
            source: "anilist".into(), category: "user".into(), value: Some(score),
            scale: 100.0, votes, url: source_url, ..Default::default()
        });
    }
    Ok(draft)
}

fn mangadex_detail_url(id: &str) -> Result<String> {
    let mut url = reqwest::Url::parse("https://api.mangadex.org/manga/")
        .map_err(|_| AppError::new(ErrorKind::Invalid, "MangaDex 地址配置无效"))?;
    url.path_segments_mut().map_err(|_| AppError::new(ErrorKind::Invalid, "MangaDex 地址配置无效"))?
        .pop_if_empty().push(id);
    for include in ["author", "artist", "cover_art"] { url.query_pairs_mut().append_pair("includes[]", include); }
    Ok(url.into())
}

fn mangadex_url(ctx: &AdapterContext, resource: &str, offset: u64) -> Result<String> {
    let mut url = reqwest::Url::parse(&format!("https://api.mangadex.org/{resource}"))
        .map_err(|_| AppError::new(ErrorKind::Invalid, "MangaDex 地址配置无效"))?;
    let limit = MANGADEX_PAGE_SIZE.min(MANGADEX_OFFSET_LIMIT.saturating_sub(offset));
    let start = ctx.start.format("%Y-%m-%dT00:00:00").to_string();
    let mut query = url.query_pairs_mut();
    query.append_pair("limit", &limit.to_string()).append_pair("offset", &offset.to_string());
    if resource == "chapter" {
        query.append_pair("publishAtSince", &start).append_pair("order[publishAt]", "asc")
            .append_pair("includes[]", "manga").append_pair("includeExternalUrl", "0");
    } else {
        query.append_pair("updatedAtSince", &start).append_pair("order[updatedAt]", "desc");
        for include in ["author", "artist", "cover_art"] { query.append_pair("includes[]", include); }
    }
    query.append_pair("contentRating[]", "safe").append_pair("contentRating[]", "suggestive");
    drop(query);
    Ok(url.into())
}

fn mangadex_page(payload: &Value, offset: u64) -> Result<(&[Value], u64)> {
    if payload["result"] != "ok" { return Err(payload_error("MangaDex", "result")); }
    let rows = required_array(&payload["data"], "MangaDex", "data")?;
    let total = payload["total"].as_u64().ok_or_else(|| payload_error("MangaDex", "total"))?;
    if payload["offset"].as_u64() != Some(offset) || payload["limit"].as_u64().is_none() {
        return Err(payload_error("MangaDex", "offset/limit"));
    }
    if rows.is_empty() && offset < total { return Err(payload_error("MangaDex", "nonempty next page")); }
    Ok((rows, total))
}

pub async fn mangadex(ctx: AdapterContext, tx: Sender<IngestionBatch>) -> Result<()> {
    // Calendar first: a large mutable metadata index must not starve the selected daily/history window.
    let mut cursor = Cursor::load(&ctx, "mangadex", "chapter", 0)?;
    if !matches!(cursor.phase.as_str(), "chapter" | "manga") || cursor.next > MANGADEX_OFFSET_LIMIT {
        return Err(AppError::new(ErrorKind::Invalid, "MangaDex 采集游标无效"));
    }
    if cursor.next == MANGADEX_OFFSET_LIMIT {
        let note = format!("{MANGADEX_NOTE} {OFFSET_NOTE}");
        return send_page(&ctx, &tx, &cursor, Vec::new(), 0, false, &note).await;
    }
    if ctx.max_pages == 0 {
        return send_page(&ctx, &tx, &cursor, Vec::new(), 0, false, MANGADEX_NOTE).await;
    }
    let (start, end) = range_epochs(&ctx);
    let mut cache: HashMap<String, Option<MediaDraft>> = HashMap::new();
    for page in 1..=ctx.max_pages {
        pace().await;
        let payload = json(&ctx.http, HttpRequest::get(mangadex_url(&ctx, &cursor.phase, cursor.next)?)).await?;
        let (rows, total) = mangadex_page(&payload, cursor.next)?;
        let chapters = cursor.phase == "chapter";
        let mut items = Vec::with_capacity(rows.len());
        let mut boundary = false;
        let mut previous_time = None;
        for row in rows {
            let expected_type = if chapters { "chapter" } else { "manga" };
            if row["type"] != expected_type { return Err(payload_error("MangaDex", "data.type")); }
            required_text(&row["id"], "MangaDex", "data.id")?;
            let field = if chapters { "publishAt" } else { "updatedAt" };
            let time = timestamp(&row["attributes"][field], "MangaDex", field)?;
            let seconds = time.timestamp();
            if previous_time.is_some_and(|previous| if chapters { seconds < previous } else { seconds > previous }) {
                return Err(payload_error("MangaDex", "date pagination order"));
            }
            previous_time = Some(seconds);
            if chapters && seconds >= end { boundary = true; continue; }
            if !chapters && seconds < start { boundary = true; continue; }
            if seconds < start || seconds >= end { continue; }
            if chapters {
                if non_empty(row["attributes"]["externalUrl"].as_str()).is_some() { continue; }
                let relationships = required_array(&row["relationships"], "MangaDex", "chapter.relationships")?;
                let manga = relationships.iter().find(|rel| rel["type"] == "manga")
                    .ok_or_else(|| payload_error("MangaDex", "chapter.manga"))?;
                let id = required_text(&manga["id"], "MangaDex", "chapter.manga.id")?;
                if !cache.contains_key(&id) {
                    let draft = if manga["attributes"].is_object() && manga["relationships"].is_array() {
                        mangadex_media(manga)?
                    } else {
                        pace().await;
                        let detail = json(&ctx.http, HttpRequest::get(mangadex_detail_url(&id)?)).await?;
                        if detail["result"] != "ok" || detail["data"]["id"] != id {
                            return Err(payload_error("MangaDex", "manga detail"));
                        }
                        mangadex_media(&detail["data"])?
                    };
                    cache.insert(id.clone(), draft);
                }
                if let Some(mut draft) = cache.get(&id).and_then(|draft| draft.clone()) {
                    let release = mangadex_chapter(row, time, &draft.media.key)?;
                    if draft.media.latest_chapter.is_none() { draft.media.latest_chapter = release.episode.clone(); }
                    draft.releases.push(release);
                    items.push(draft);
                }
            } else if let Some(draft) = mangadex_media(row)? {
                cache.insert(draft.external_id.clone(), Some(draft.clone()));
                items.push(draft);
            }
        }
        let next = cursor.next + rows.len() as u64;
        let exhausted = boundary || next >= total;
        if exhausted {
            if chapters { cursor.phase = "manga".into(); cursor.next = 0; }
            else { cursor.phase = "done".into(); cursor.next = 0; }
        } else { cursor.next = next; }
        let complete = cursor.phase == "done";
        let restricted = !complete && cursor.next >= MANGADEX_OFFSET_LIMIT;
        let note = if restricted { format!("{MANGADEX_NOTE} {OFFSET_NOTE}") } else { MANGADEX_NOTE.into() };
        send_page(&ctx, &tx, &cursor, items, page, complete, &note).await?;
        if complete || restricted { return Ok(()); }
    }
    Ok(())
}

fn localized(value: &Value, preferred: &[&str]) -> Option<String> {
    for language in preferred {
        if let Some(name) = text(&value[*language]) { return Some(name); }
    }
    value.as_object()?.values().find_map(text)
}

fn title_in_language(attributes: &Value, language: &str) -> Option<String> {
    text(&attributes["title"][language]).or_else(|| {
        attributes["altTitles"].as_array()?.iter().find_map(|title| text(&title[language]))
    })
}

fn mangadex_media(value: &Value) -> Result<Option<MediaDraft>> {
    if value["type"] != "manga" { return Err(payload_error("MangaDex", "manga.type")); }
    let id = required_text(&value["id"], "MangaDex", "manga.id")?;
    let attributes = &value["attributes"];
    let content_rating = required_text(&attributes["contentRating"], "MangaDex", "contentRating")?;
    if !matches!(content_rating.as_str(), "safe" | "suggestive") { return Ok(None); }
    let title = localized(&attributes["title"], &["zh", "zh-hans", "zh-hant", "ja", "ko", "en"])
        .ok_or_else(|| payload_error("MangaDex", "manga.title"))?;
    let language = required_text(&attributes["originalLanguage"], "MangaDex", "originalLanguage")?;
    let mut aliases = Vec::new();
    if let Some(titles) = attributes["title"].as_object() {
        for name in titles.values() { append_unique(&mut aliases, text(name)); }
    }
    for titles in required_array(&attributes["altTitles"], "MangaDex", "altTitles")? {
        if let Some(titles) = titles.as_object() {
            for name in titles.values() { append_unique(&mut aliases, text(name)); }
        } else { return Err(payload_error("MangaDex", "altTitles")); }
    }
    let original_title = title_in_language(attributes, &language).or_else(|| {
        if language == "zh" {
            title_in_language(attributes, "zh-hans").or_else(|| title_in_language(attributes, "zh-hant"))
        } else { None }
    });
    let mut genres = Vec::new();
    for tag in required_array(&attributes["tags"], "MangaDex", "tags")? {
        append_unique(&mut genres, localized(&tag["attributes"]["name"], &["en"]));
    }
    let mut writers = Vec::new();
    let mut poster = None;
    for relationship in required_array(&value["relationships"], "MangaDex", "manga.relationships")? {
        if relationship["type"] == "author" {
            append_unique(&mut writers, text(&relationship["attributes"]["name"]));
        } else if relationship["type"] == "cover_art" {
            if let Some(file) = text(&relationship["attributes"]["fileName"]) {
                if !file.contains(['/', '\\', '?', '#', '%']) {
                    poster = Some(format!("https://uploads.mangadex.org/covers/{id}/{file}.256.jpg"));
                }
            }
        }
    }
    let mut external_ids = Vec::new();
    for (field, source) in [("al", "anilist"), ("mal", "mal")] {
        if let Some(external_id) = text(&attributes["links"][field]) {
            if external_id.parse::<u64>().is_ok_and(|id| id > 0) {
                external_ids.push(CatalogExternalId { source: source.into(), external_id });
            }
        }
    }
    if let Some(external_id) = text(&attributes["links"]["imdb"]) {
        if external_id.strip_prefix("tt").is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())) {
            external_ids.push(CatalogExternalId { source: "imdb".into(), external_id });
        }
    }
    let region = match language.as_str() {
        "ja" => Some("JP"), "ko" => Some("KR"), "zh" | "zh-hans" | "zh-hant" => Some("CN"),
        // English-language originals need an explicit origin; language alone cannot establish US.
        _ => None,
    };
    Ok(Some(MediaDraft {
        source: "mangadex".into(), external_id: id.clone(), external_ids,
        media: CatalogMedia {
            key: format!("mangadex:{id}"), source: "mangadex".into(), title, original_title,
            english_title: title_in_language(attributes, "en"), aliases, kind: "manga".into(),
            regions: region.map(str::to_string).into_iter().collect(), genres, poster,
            summary: localized(&attributes["description"], &["zh", "zh-hans", "zh-hant", "en", "ja", "ko"])
                .and_then(|s| non_empty(Some(&strip_html(&s)))),
            writers, status: match attributes["status"].as_str() {
                Some("ongoing") => "airing", Some("completed") => "completed",
                Some("hiatus") => "hiatus", Some("cancelled") => "cancelled", _ => "unknown",
            }.into(),
            year: attributes["year"].as_i64().and_then(|year| i32::try_from(year).ok()),
            latest_chapter: text(&attributes["lastChapter"]), source_url: Some(format!("https://mangadex.org/title/{id}")),
            ..Default::default()
        }, ..Default::default()
    }))
}

fn mangadex_chapter(value: &Value, time: DateTime<Utc>, media_key: &str) -> Result<CatalogRelease> {
    let id = required_text(&value["id"], "MangaDex", "chapter.id")?;
    let attributes = &value["attributes"];
    let language = required_text(&attributes["translatedLanguage"], "MangaDex", "translatedLanguage")?;
    let chapter = text(&attributes["chapter"]);
    let mut label = text(&attributes["title"]).unwrap_or_else(|| "章节".into());
    if let Some(volume) = text(&attributes["volume"]) { label.push_str(&format!(" · Vol {volume}")); }
    if let Some(chapter) = &chapter { label.push_str(&format!(" · Ch {chapter}")); }
    label.push_str(&format!(" · {language}"));
    Ok(CatalogRelease {
        key: format!("mangadex:{id}"), media_key: media_key.into(), source: "mangadex".into(),
        external_id: id.clone(), date: time.date_naive().to_string(),
        local_time: Some(time.format("%H:%M:%S").to_string()), timezone: Some("UTC".into()),
        utc_time: Some(utc_text(time)), episode: chapter, title: Some(label),
        platform: Some("MangaDex".into()), release_kind: "chapter".into(),
        url: Some(format!("https://mangadex.org/chapter/{id}")), ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::net::MockHttp;
    use serde_json::json as value;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    fn context(http: Arc<MockHttp>, mode: &str, pages: u32) -> AdapterContext {
        AdapterContext {
            http,
            start: NaiveDate::from_ymd_opt(2026, 11, 24).unwrap(),
            end: NaiveDate::from_ymd_opt(2026, 11, 25).unwrap(),
            mode: mode.into(),
            cursor: None,
            max_pages: pages,
            countries: vec!["US".into(), "JP".into()],
            token: None,
        }
    }

    async fn collect(mut rx: mpsc::Receiver<IngestionBatch>) -> Vec<IngestionBatch> {
        let mut batches = Vec::new();
        while let Some(batch) = rx.recv().await {
            batches.push(batch);
        }
        batches
    }

    fn tv_episode(id: u32, date: &str, web: bool) -> Value {
        let show = value!({
            "id": 7, "name": "番組", "type": "Animation", "status": "Running",
            "url": "https://www.tvmaze.com/shows/7/example",
            "genres": ["Drama"], "rating": {"average": 8.1},
            "externals": {"imdb": "tt1234567", "thetvdb": 42},
            "network": {"name": "日本局", "country": {"code": "JP", "timezone": "Asia/Tokyo"}},
            "webChannel": {"name": "配信", "country": {"code": "JP", "timezone": "Asia/Tokyo"}}
        });
        let mut episode = value!({
            "id": id, "name": "第二話", "season": 1, "number": 2,
            "airdate": date, "airtime": "00:30", "airstamp": "2026-11-23T15:30:00Z",
            "url": format!("https://www.tvmaze.com/episodes/{id}")
        });
        if web {
            episode["_embedded"] = value!({"show": show});
        } else {
            episode["show"] = show;
        }
        episode
    }

    #[tokio::test]
    async fn tvmaze_full_iterates_every_date_country_and_global_web() {
        let http = Arc::new(MockHttp::new());
        let mut ctx = context(http.clone(), "full", 6);
        ctx.end = NaiveDate::from_ymd_opt(2026, 11, 26).unwrap();
        let mut expected = Vec::new();
        for date in ["2026-11-24", "2026-11-25"] {
            for country in ["US", "JP"] {
                let url = format!("https://api.tvmaze.com/schedule?country={country}&date={date}");
                http.on(&url, 200, &value!([tv_episode(1, date, false)]).to_string());
                expected.push(url);
            }
            let url = format!("https://api.tvmaze.com/schedule/web?date={date}");
            http.on(&url, 200, &value!([tv_episode(2, date, true)]).to_string());
            expected.push(url);
        }
        let (tx, rx) = mpsc::channel(1);
        let (result, batches) = tokio::join!(tvmaze(ctx, tx), collect(rx));
        result.unwrap();
        assert_eq!(http.calls(), expected);
        assert_eq!(batches.len(), 6);
        assert!(batches.iter().all(|b| b.cursor.is_some()));
        assert!(batches[..5].iter().all(|b| !b.complete));
        assert!(batches[5].complete);
        let item = &batches[0].items[0];
        let release = &item.releases[0];
        assert_eq!(release.date, "2026-11-24");
        assert_eq!(release.local_time.as_deref(), Some("00:30"));
        assert_eq!(release.timezone.as_deref(), Some("Asia/Tokyo"));
        assert_eq!(release.utc_time.as_deref(), Some("2026-11-23T15:30:00Z"));
        assert_eq!(release.release_kind, "episode");
        assert!(item.media.regions.is_empty(), "发行国家不能当作制作国家");
        assert_eq!(item.ratings[0].votes, None);
        assert!(item.external_ids.iter().any(|id| id.source == "tvdb" && id.external_id == "42"));
        assert_eq!(batches[2].items[0].media.title, "番組");
    }

    #[tokio::test]
    async fn tvmaze_budget_cursor_resumes_instead_of_claiming_complete() {
        let http = Arc::new(MockHttp::new());
        let ctx = context(http.clone(), "history", 1);
        http.on("https://api.tvmaze.com/schedule?country=US&date=2026-11-24", 200, "[]");
        http.on("https://api.tvmaze.com/schedule?country=JP&date=2026-11-24", 200, "[]");
        http.on("https://api.tvmaze.com/schedule/web?date=2026-11-24", 200, "[]");
        let (tx, rx) = mpsc::channel(1);
        let (result, first) = tokio::join!(tvmaze(ctx.clone(), tx), collect(rx));
        result.unwrap();
        assert!(!first[0].complete);
        assert!(first[0].warning.as_deref().unwrap().contains("预算"));
        let mut resumed = ctx;
        resumed.max_pages = 2;
        resumed.cursor = first[0].cursor.clone();
        let (tx, rx) = mpsc::channel(1);
        let (result, rest) = tokio::join!(tvmaze(resumed, tx), collect(rx));
        result.unwrap();
        assert_eq!(rest.len(), 2);
        assert!(rest[1].complete);
        assert_eq!(http.calls().len(), 3);
    }

    fn anilist_media(id: u32, kind: &str) -> Value {
        value!({
            "id": id, "idMal": 42, "title": {"native": "原題", "romaji": "Gendai", "english": "Original"},
            "synonyms": ["別名"], "type": kind, "format": if kind == "MANGA" {"MANGA"} else {"TV"},
            "countryOfOrigin": "JP", "genres": ["Drama"], "description": "<p>Story</p>",
            "isAdult": false, "status": "RELEASING", "episodes": 12, "chapters": null,
            "updatedAt": 1795478400, "averageScore": 82, "popularity": 99999,
            "startDate": {"year": 2026, "month": 10, "day": 1}, "endDate": null,
            "siteUrl": format!("https://anilist.co/{}/{id}", kind.to_lowercase()),
            "studios": {"nodes": [{"name": "Studio"}]}, "externalLinks": [],
            "stats": {"scoreDistribution": [{"score": 80, "amount": 3}, {"score": 90, "amount": 4}]}
        })
    }

    #[tokio::test]
    async fn anilist_two_airing_pages_use_has_next_and_actual_vote_distribution() {
        let http = Arc::new(MockHttp::new());
        let ctx = context(http.clone(), "full", 2);
        let first = value!({"data": {"Page": {
            "pageInfo": {"hasNextPage": true}, "airingSchedules": [
                {"id": 100, "episode": 1, "airingAt": 1795478400, "media": anilist_media(7, "ANIME")}
            ]
        }}}).to_string();
        let second = value!({"data": {"Page": {
            "pageInfo": {"hasNextPage": false}, "airingSchedules": [
                {"id": 101, "episode": 2, "airingAt": 1795482000, "media": anilist_media(7, "ANIME")}
            ]
        }}}).to_string();
        let manga = value!({"data": {"Page": {
            "pageInfo": {"hasNextPage": false}, "media": [anilist_media(8, "MANGA")]
        }}}).to_string();
        http.on_sequence(ANILIST_URL, vec![(200, &first), (200, &second), (200, &manga)]);
        let (tx, rx) = mpsc::channel(1);
        let (result, batches) = tokio::join!(anilist(ctx.clone(), tx), collect(rx));
        result.unwrap();
        assert_eq!(batches.len(), 2);
        assert!(batches.iter().all(|b| !b.complete));
        assert_eq!(batches[0].items[0].ratings[0].votes, Some(7));
        assert_eq!(batches[0].items[0].ratings[0].scale, 100.0);
        assert_eq!(batches[0].items[0].ratings[0].value, Some(82.0));
        assert_eq!(batches[0].items[0].releases[0].timezone.as_deref(), Some("UTC"));
        assert_eq!(batches[1].items[0].releases[0].external_id, "101");
        let body: Value = serde_json::from_str(&http.last_body(ANILIST_URL).unwrap()).unwrap();
        assert_eq!(body["variables"]["page"], 2);
        assert_eq!(body["variables"]["start"], 1795478399_i64);
        assert_eq!(body["variables"]["end"], 1795564800_i64);
        let mut resumed = ctx;
        resumed.max_pages = 1;
        resumed.cursor = batches[1].cursor.clone();
        let (tx, rx) = mpsc::channel(1);
        let (result, manga_batches) = tokio::join!(anilist(resumed, tx), collect(rx));
        result.unwrap();
        assert!(manga_batches[0].complete);
        assert!(manga_batches[0].items[0].releases.is_empty());
        assert!(manga_batches[0].items[0].media.aliases.contains(&"Gendai".into()));
        assert!(http.last_body(ANILIST_URL).unwrap().contains("isAdult: false"));
    }

    fn dex_manga() -> Value {
        value!({
            "id": "manga-a", "type": "manga",
            "attributes": {
                "title": {"en": "Translated"}, "altTitles": [{"ja": "原作名"}, {"zh-hans": "中文别名"}],
                "originalLanguage": "ja", "status": "ongoing", "contentRating": "safe",
                "updatedAt": "2026-11-24T08:00:00+00:00", "year": 2026, "lastChapter": null,
                "links": {"al": "7", "mal": "42"}, "tags": []
            },
            "relationships": [
                {"id": "author-a", "type": "author", "attributes": {"name": "作者"}},
                {"id": "cover-a", "type": "cover_art", "attributes": {"fileName": "cover.jpg"}}
            ]
        })
    }

    fn dex_chapter(id: &str, chapter: &str) -> Value {
        value!({
            "id": id, "type": "chapter",
            "attributes": {
                "publishAt": "2026-11-24T08:00:00+00:00", "updatedAt": "2026-11-25T08:00:00+00:00",
                "chapter": chapter, "volume": "1", "title": "译本", "translatedLanguage": "en", "externalUrl": null
            },
            "relationships": [{"type": "manga", "id": "manga-a"}]
        })
    }

    fn dex_page(offset: u64, total: u64, data: Value) -> Value {
        value!({"result": "ok", "limit": 100, "offset": offset, "total": total, "data": data})
    }

    fn prepare_dex(http: &Arc<MockHttp>, ctx: &AdapterContext) {
        http.on(&mangadex_url(ctx, "chapter", 0).unwrap(), 200,
            &dex_page(0, 2, value!([dex_chapter("chapter-a", "1")])).to_string());
        http.on(&mangadex_url(ctx, "chapter", 1).unwrap(), 200,
            &dex_page(1, 2, value!([dex_chapter("chapter-b", "2")])).to_string());
        http.on(&mangadex_detail_url("manga-a").unwrap(), 200,
            &value!({"result": "ok", "data": dex_manga()}).to_string());
        http.on(&mangadex_url(ctx, "manga", 0).unwrap(), 200,
            &dex_page(0, 0, value!([])).to_string());
    }

    #[tokio::test]
    async fn mangadex_two_chapter_pages_preserve_both_and_native_aliases() {
        let http = Arc::new(MockHttp::new());
        let ctx = context(http.clone(), "history", 3);
        prepare_dex(&http, &ctx);
        let (tx, rx) = mpsc::channel(1);
        let (result, batches) = tokio::join!(mangadex(ctx.clone(), tx), collect(rx));
        result.unwrap();
        assert_eq!(batches.len(), 3);
        assert!(batches[2].complete);
        let items: Vec<_> = batches.iter().flat_map(|b| &b.items).collect();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].releases[0].external_id, "chapter-a");
        assert_eq!(items[1].releases[0].external_id, "chapter-b");
        assert_eq!(items[1].releases[0].date, "2026-11-24");
        assert_eq!(items[1].releases[0].release_kind, "chapter");
        assert!(items[1].releases[0].title.as_deref().unwrap().contains("en"));
        assert_eq!(items[0].media.original_title.as_deref(), Some("原作名"));
        assert!(items[0].media.aliases.contains(&"中文别名".into()));
        assert_eq!(items[0].media.regions, vec!["JP"]);
        assert_eq!(items[0].media.chapter_count, None);
        assert_eq!(http.call_count(&mangadex_detail_url("manga-a").unwrap()), 1);
        let url = reqwest::Url::parse(&mangadex_url(&ctx, "chapter", 0).unwrap()).unwrap();
        assert!(!url.query_pairs().any(|(key, _)| key == "publishAtUntil"));
        assert!(url.query_pairs().any(|(key, value)| key == "order[publishAt]" && value == "asc"));
    }

    #[tokio::test]
    async fn mangadex_page_two_failure_keeps_already_published_page() {
        let dir = tempfile::tempdir().unwrap();
        let http = Arc::new(MockHttp::new());
        let ctx = context(http.clone(), "incremental", 3);
        http.on(&mangadex_url(&ctx, "chapter", 0).unwrap(), 200,
            &dex_page(0, 2, value!([dex_chapter("chapter-a", "1")])).to_string());
        http.on(&mangadex_detail_url("manga-a").unwrap(), 200,
            &value!({"result": "ok", "data": dex_manga()}).to_string());
        http.on(&mangadex_url(&ctx, "chapter", 1).unwrap(), 200, "{\"result\":\"ok\"}");
        let (tx, mut rx) = mpsc::channel(1);
        let task = tokio::spawn(mangadex(ctx, tx));
        let page = rx.recv().await.unwrap();
        assert!(!page.complete);
        assert!(page.cursor.is_some());
        let saved = dir.path().join("first-page.json");
        std::fs::write(&saved, serde_json::to_vec(&page.items[0].releases).unwrap()).unwrap();
        assert_eq!(task.await.unwrap().unwrap_err().kind, ErrorKind::Provider);
        let releases: Vec<CatalogRelease> = serde_json::from_slice(&std::fs::read(saved).unwrap()).unwrap();
        assert_eq!(releases[0].external_id, "chapter-a");
    }

    #[tokio::test]
    async fn anilist_incremental_manga_stops_only_at_actual_update_boundary() {
        let http = Arc::new(MockHttp::new());
        let mut ctx = context(http.clone(), "incremental", 2);
        let mut cursor = Cursor::load(&ctx, "anilist", "manga", 1).unwrap();
        cursor.phase = "manga".into();
        ctx.cursor = Some(cursor.encode().unwrap());
        let mut recent = anilist_media(8, "MANGA");
        recent["updatedAt"] = value!(1795478400_i64);
        let mut old = anilist_media(9, "MANGA");
        old["updatedAt"] = value!(1795478399_i64);
        http.on(ANILIST_URL, 200, &value!({"data": {"Page": {
            "pageInfo": {"hasNextPage": true}, "media": [recent, old]
        }}}).to_string());
        let (tx, rx) = mpsc::channel(1);
        let (result, batches) = tokio::join!(anilist(ctx, tx), collect(rx));
        result.unwrap();
        assert_eq!(batches.len(), 1);
        assert!(batches[0].complete);
        assert_eq!(batches[0].items.len(), 1);
        assert_eq!(batches[0].items[0].external_id, "8");
        assert!(batches[0].items[0].releases.is_empty());
        assert_eq!(http.call_count(ANILIST_URL), 1);
    }

    #[tokio::test]
    async fn mangadex_offset_restriction_preserves_cursor_and_entire_page() {
        let http = Arc::new(MockHttp::new());
        let mut ctx = context(http.clone(), "full", 1);
        let cursor = Cursor::load(&ctx, "mangadex", "manga", 9900).unwrap();
        ctx.cursor = Some(cursor.encode().unwrap());
        let rows: Vec<_> = (0..100).map(|index| {
            let mut manga = dex_manga();
            manga["id"] = value!(format!("manga-{index}"));
            manga
        }).collect();
        http.on(&mangadex_url(&ctx, "manga", 9900).unwrap(), 200,
            &dex_page(9900, 20_000, value!(rows)).to_string());
        let (tx, rx) = mpsc::channel(1);
        let (result, batches) = tokio::join!(mangadex(ctx.clone(), tx), collect(rx));
        result.unwrap();
        assert_eq!(batches[0].items.len(), 100);
        assert!(!batches[0].complete);
        assert!(batches[0].warning.as_deref().unwrap().contains("10000"));
        let saved: Cursor = serde_json::from_str(batches[0].cursor.as_deref().unwrap()).unwrap();
        assert_eq!(saved.next, 10_000);
        ctx.cursor = batches[0].cursor.clone();
        let (tx, rx) = mpsc::channel(1);
        let (result, resumed) = tokio::join!(mangadex(ctx, tx), collect(rx));
        result.unwrap();
        assert!(!resumed[0].complete);
        assert_eq!(resumed[0].page, 0);
        assert_eq!(http.calls().len(), 1);
    }

    #[tokio::test]
    async fn mangadex_exclusive_end_stops_ascending_chapter_pages() {
        let http = Arc::new(MockHttp::new());
        let ctx = context(http.clone(), "history", 2);
        let mut outside = dex_chapter("chapter-end", "2");
        outside["attributes"]["publishAt"] = value!("2026-11-25T00:00:00Z");
        http.on(&mangadex_url(&ctx, "chapter", 0).unwrap(), 200,
            &dex_page(0, 1000, value!([dex_chapter("chapter-a", "1"), outside])).to_string());
        http.on(&mangadex_detail_url("manga-a").unwrap(), 200,
            &value!({"result": "ok", "data": dex_manga()}).to_string());
        http.on(&mangadex_url(&ctx, "manga", 0).unwrap(), 200,
            &dex_page(0, 0, value!([])).to_string());
        let (tx, rx) = mpsc::channel(1);
        let (result, batches) = tokio::join!(mangadex(ctx, tx), collect(rx));
        result.unwrap();
        assert_eq!(batches[0].items.len(), 1);
        assert!(batches[1].complete);
        assert_eq!(http.calls().len(), 3);
    }

    #[test]
    fn mangadex_english_does_not_invent_us_origin() {
        let mut manga = dex_manga();
        manga["attributes"]["originalLanguage"] = value!("en");
        assert!(super::mangadex_media(&manga).unwrap().unwrap().media.regions.is_empty());
    }

    #[tokio::test]
    async fn missing_source_fields_are_provider_errors_not_empty_success() {
        let http = Arc::new(MockHttp::new());
        http.on("https://api.tvmaze.com/schedule?country=US&date=2026-11-24", 200, "{}");
        let (tx, _rx) = mpsc::channel(1);
        assert_eq!(tvmaze(context(http, "full", 1), tx).await.unwrap_err().kind, ErrorKind::Provider);
        let http = Arc::new(MockHttp::new());
        http.on(ANILIST_URL, 200, "{\"data\":{\"Page\":{\"pageInfo\":{\"hasNextPage\":false}}}}");
        let (tx, _rx) = mpsc::channel(1);
        assert_eq!(anilist(context(http, "full", 1), tx).await.unwrap_err().kind, ErrorKind::Provider);
    }
}
