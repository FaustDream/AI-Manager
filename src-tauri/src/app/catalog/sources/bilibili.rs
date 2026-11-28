//! B 站番剧 / 影视目录。
//!
//! 两条数据通道，同一套媒体键（`bilibili:{season_id}`）：
//!   1. `pgc/season/index/result` 按热度列目录，`pgc/view/web/season` 补中文简介、
//!      类型、地区、总集数、评分与每集时间——这是媒体库的主要内容来源。
//!   2. `pgc/web/timeline` 给前后 7 日的精确更新日，用于发行日历。
//!
//! 两条通道都只读公开元数据，不取播放线路。

use super::{cn_region, episode_numbers, normalize_date, non_empty, pace, publish, year_of};
use crate::app::catalog::types::{
    CatalogMedia, CatalogRating, CatalogRelease, IngestionBatch, MediaDraft,
};
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::HttpRequest;
use crate::app::catalog::sources::AdapterContext;
use chrono::{DateTime, Datelike, FixedOffset, TimeZone, Utc};
use futures_util::future::join_all;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use tokio::sync::mpsc::Sender;

const INDEX_URL: &str = "https://api.bilibili.com/pgc/season/index/result";
const SEASON_URL: &str = "https://api.bilibili.com/pgc/view/web/season";
const TIMELINE_URL: &str = "https://api.bilibili.com/pgc/web/timeline";

/// `season_type`：1 番剧、2 电影、3 纪录片、4 国创、5 电视剧、7 综艺。
/// 国创并入动漫：对用户来说都是追番。
const KINDS: &[(u8, &str)] = &[
    (1, "anime"),
    (2, "movie"),
    (3, "documentary"),
    (4, "anime"),
    (5, "tv"),
    (7, "variety"),
];

/// 每类取热门前 12 条：详情要逐条补，条数直接决定一次采集的请求量。
const PAGE_SIZE: u32 = 12;
const UA: &str = super::DESKTOP_UA;

pub async fn fetch(ctx: AdapterContext, tx: Sender<IngestionBatch>) -> Result<()> {
    let catalog = collect_catalog(&ctx).await;
    let timeline = collect_timeline(&ctx).await;
    // 目录批 complete 与否取决于时间线批是否会来：最后一批必须是 complete=true，
    // 否则整源永远停在「分页未完成」。
    match catalog {
        Ok(items) => {
            let timeline_follows = timeline.as_ref().map(|i| !i.is_empty()).unwrap_or(false);
            publish(&tx, IngestionBatch {
                items,
                page: 1,
                complete: !timeline_follows,
                http_status: Some(200),
                warning: Some("目录按热度分页：频道首页条目带详情，深入页只有索引字段；没有剧集确认日期的条目只进媒体库，不伪造发行日历".into()),
                ..Default::default()
            }).await?;
        }
        Err(err) if timeline.is_err() => return Err(err),
        Err(_) => {}
    }
    if let Ok(items) = timeline {
        if !items.is_empty() {
            publish(&tx, IngestionBatch {
                items,
                page: 2,
                complete: true,
                http_status: Some(200),
                ..Default::default()
            }).await?;
        }
    }
    Ok(())
}

/// 目录：每个频道按热度分页（页预算 = max_pages 扣掉时间线后均分到 6 个频道），
/// 频道首页条目逐条补详情，深入页只用索引字段进库。
/// 热增量（页预算 ≤ 6）只拉时间线：目录翻页留给常规采集轮。
async fn collect_catalog(ctx: &AdapterContext) -> Result<Vec<MediaDraft>> {
    let pages_per_type = if ctx.max_pages <= 6 {
        0
    } else {
        ((ctx.max_pages - 6) / 6).clamp(1, 24)
    };
    let mut index: Vec<IndexItem> = Vec::new();
    let mut detail_ids: BTreeSet<String> = BTreeSet::new();
    let mut first_error = None;
    if pages_per_type > 0 {
        for (kind_id, kind) in KINDS {
            for page in 1..=pages_per_type {
                let url = format!(
                    "{INDEX_URL}?season_type={kind_id}&is_finish=-1&order=3&page={page}&pagesize={PAGE_SIZE}&type=1"
                );
                let req = HttpRequest::get(url)
                    .header("User-Agent", UA)
                    .header("Referer", "https://www.bilibili.com/");
                let resp = match ctx.http.send(req).await {
                    Ok(resp) => resp,
                    Err(err) => { first_error.get_or_insert(err); break; }
                };
                if !resp.is_ok() {
                    first_error.get_or_insert_with(|| AppError::new(ErrorKind::Provider, "哔哩哔哩目录返回错误状态"));
                    break;
                }
                let rows = match parse_index(kind, &resp.body) {
                    Ok(rows) => rows,
                    Err(err) => { first_error.get_or_insert(err); break; }
                };
                let exhausted = rows.len() < PAGE_SIZE as usize;
                if page == 1 {
                    for item in &rows { detail_ids.insert(item.season_id.clone()); }
                }
                index.extend(rows);
                if exhausted { break; }
                pace().await;
            }
        }
    }
    if index.is_empty() {
        if let Some(err) = first_error {
            return Err(err);
        }
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(index.len());
    let mut detail_error = None;
    for item in index {
        if !detail_ids.contains(&item.season_id) {
            // 深入页只有索引字段：标题/封面/评分/完结状态足够进库与展示
            out.push(index_draft(item));
            continue;
        }
        pace().await;
        let url = format!("{SEASON_URL}?season_id={}", item.season_id);
        let req = HttpRequest::get(url)
            .header("User-Agent", UA)
            .header("Referer", "https://www.bilibili.com/");
        match ctx.http.send(req).await {
            Ok(resp) if resp.is_ok() => match parse_season(ctx, &item, &resp.body) {
                Ok(draft) => out.push(draft),
                Err(_) => out.push(index_draft(item)),
            },
            Ok(_) => {
                detail_error.get_or_insert_with(|| {
                    AppError::new(ErrorKind::Provider, "哔哩哔哩详情返回错误状态")
                });
                out.push(index_draft(item));
            }
            Err(err) => {
                detail_error.get_or_insert(err);
                out.push(index_draft(item));
            }
        }
    }
    if out.is_empty() {
        return Err(detail_error.unwrap_or_else(|| {
            AppError::new(ErrorKind::Provider, "哔哩哔哩没有可用的目录条目")
        }));
    }
    Ok(out)
}

/// 时间线：只补发行日历，字段以剧集为准。
async fn collect_timeline(ctx: &AdapterContext) -> Result<Vec<MediaDraft>> {
    let requests = KINDS.iter().map(|(kind_id, kind)| {
        let http = ctx.http.clone();
        async move {
            let url = format!("{TIMELINE_URL}?types={kind_id}&before=7&after=7");
            let req = HttpRequest::get(url)
                .header("User-Agent", UA)
                .header("Referer", "https://www.bilibili.com/");
            let resp = http.send(req).await?;
            Ok::<_, AppError>((*kind, resp))
        }
    });
    let mut out = Vec::new();
    let mut first_error = None;
    for result in join_all(requests).await {
        let (kind, resp) = match result {
            Ok(value) => value,
            Err(err) => {
                first_error.get_or_insert(err);
                continue;
            }
        };
        match parse_timeline(kind, &resp.body) {
            Ok(mut items) => out.append(&mut items),
            Err(err) => {
                first_error.get_or_insert(err);
            }
        }
    }
    if out.is_empty() {
        if let Some(err) = first_error {
            return Err(err);
        }
    }
    Ok(out)
}

/// 索引页的一条：详情拿不到时也能单独入库。
#[derive(Debug, Clone, PartialEq)]
struct IndexItem {
    season_id: String,
    kind: &'static str,
    title: String,
    poster: Option<String>,
    score: Option<f64>,
    finished: bool,
    index_show: Option<String>,
    link: Option<String>,
}

fn parse_index(kind: &'static str, body: &str) -> Result<Vec<IndexItem>> {
    let value: Value = serde_json::from_str(body)
        .map_err(|_| AppError::new(ErrorKind::Provider, "哔哩哔哩目录不是 JSON"))?;
    if let Some(code) = value.get("code").and_then(|v| v.as_i64()) {
        if code != 0 {
            let message = value.get("message").and_then(|v| v.as_str()).unwrap_or("未知错误");
            return Err(AppError::new(
                ErrorKind::Provider,
                format!("哔哩哔哩目录返回错误：{message}"),
            ));
        }
    }
    let rows = value
        .pointer("/data/list")
        .and_then(|v| v.as_array())
        .ok_or_else(|| AppError::new(ErrorKind::Provider, "哔哩哔哩目录缺少 data.list"))?;
    let mut out = Vec::new();
    for row in rows {
        let Some(season_id) = row.get("season_id").and_then(|v| v.as_i64()).filter(|id| *id > 0) else {
            continue;
        };
        let Some(title) = non_empty(row.get("title").and_then(|v| v.as_str())) else {
            continue;
        };
        out.push(IndexItem {
            season_id: season_id.to_string(),
            kind,
            title,
            poster: non_empty(row.get("cover").and_then(|v| v.as_str())),
            score: row
                .get("score")
                .and_then(|v| v.as_str())
                .and_then(|s| s.trim().parse::<f64>().ok())
                .filter(|score| *score > 0.0),
            finished: row.get("is_finish").and_then(|v| v.as_i64()) == Some(1),
            index_show: non_empty(row.get("index_show").and_then(|v| v.as_str())),
            link: non_empty(row.get("link").and_then(|v| v.as_str())),
        });
    }
    Ok(out)
}

/// 只有索引字段时的兜底：不给简介/类型/评分来源，只写确实拿到的结论。
fn index_draft(item: IndexItem) -> MediaDraft {
    let (episode_count, aired_episodes) = item
        .index_show
        .as_deref()
        .map(episode_numbers)
        .unwrap_or((None, None));
    let status = if item.finished {
        "completed"
    } else if item.index_show.is_some() {
        "airing"
    } else {
        "unknown"
    };
    MediaDraft {
        source: "bilibili".into(),
        external_id: item.season_id.clone(),
        media: CatalogMedia {
            key: format!("bilibili:{}", item.season_id),
            source: "bilibili".into(),
            title: item.title,
            kind: item.kind.into(),
            status: status.into(),
            platforms: vec!["哔哩哔哩".into()],
            poster: item.poster,
            source_url: item.link,
            episode_count,
            aired_episodes,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn parse_season(ctx: &AdapterContext, item: &IndexItem, body: &str) -> Result<MediaDraft> {
    let value: Value = serde_json::from_str(body)
        .map_err(|_| AppError::new(ErrorKind::Provider, "哔哩哔哩详情不是 JSON"))?;
    if let Some(code) = value.get("code").and_then(|v| v.as_i64()) {
        if code != 0 {
            return Err(AppError::new(
                ErrorKind::Provider,
                format!("哔哩哔哩详情返回错误：{code}"),
            ));
        }
    }
    let result = value
        .get("result")
        .and_then(|v| v.as_object())
        .ok_or_else(|| AppError::new(ErrorKind::Provider, "哔哩哔哩详情缺少 result"))?;
    let season_title = non_empty(result.get("title").and_then(|v| v.as_str()));
    let title = season_title.unwrap_or_else(|| item.title.clone());
    let finished = result
        .get("publish")
        .and_then(|v| v.get("is_finish"))
        .and_then(|v| v.as_i64())
        .map(|v| v == 1)
        .unwrap_or(item.finished);
    let started = result
        .get("publish")
        .and_then(|v| v.get("is_started"))
        .and_then(|v| v.as_i64())
        .map(|v| v == 1)
        .unwrap_or(true);
    let premiered = result
        .get("publish")
        .and_then(|v| v.get("pub_time"))
        .and_then(|v| v.as_str())
        .filter(|_| started)
        .and_then(|s| s.get(..10))
        .map(str::to_string);
    let total = result
        .get("total")
        .and_then(|v| v.as_u64())
        .and_then(|n| u32::try_from(n).ok());
    let episodes: Vec<&Value> = result
        .get("episodes")
        .and_then(|v| v.as_array())
        .map(|list| list.iter().collect())
        .unwrap_or_default();
    let now = Utc::now().timestamp();
    let aired = episodes
        .iter()
        .filter(|ep| ep.get("pub_time").and_then(|v| v.as_i64()).unwrap_or(i64::MAX) <= now)
        .count() as u32;
    let status = if finished {
        "completed"
    } else if started {
        "airing"
    } else {
        "upcoming"
    };
    let score = result
        .get("rating")
        .and_then(|v| v.get("score"))
        .and_then(|v| v.as_f64())
        .filter(|score| *score > 0.0);
    let votes = result
        .get("rating")
        .and_then(|v| v.get("count"))
        .and_then(|v| v.as_u64())
        .and_then(|n| u32::try_from(n).ok());
    let mut media = CatalogMedia {
        key: format!("bilibili:{}", item.season_id),
        source: "bilibili".into(),
        title,
        kind: item.kind.into(),
        subkind: None,
        platforms: vec!["哔哩哔哩".into()],
        poster: non_empty(result.get("cover").and_then(|v| v.as_str())).or_else(|| item.poster.clone()),
        summary: non_empty(result.get("evaluate").and_then(|v| v.as_str())),
        status: status.into(),
        premiered: premiered.clone(),
        year: year_of(premiered.as_deref()),
        episode_count: total,
        aired_episodes: if episodes.is_empty() {
            None
        } else {
            // 详情集数里可能含特别篇/花絮，已发行不能超过总集数。
            Some(total.map_or(aired, |total| aired.min(total)))
        },
        source_url: Some(format!(
            "https://www.bilibili.com/bangumi/media/md{}/",
            result
                .get("media_id")
                .and_then(|v| v.as_i64())
                .map(|id| id.to_string())
                .unwrap_or_else(|| item.season_id.clone())
        )),
        ..Default::default()
    };
    if let Some(styles) = result.get("styles").and_then(|v| v.as_array()) {
        for style in styles.iter().filter_map(|v| v.as_str()) {
            if let Some(style) = non_empty(Some(style)) {
                if !media.genres.contains(&style) {
                    media.genres.push(style);
                }
            }
        }
    }
    if let Some(areas) = result.get("areas").and_then(|v| v.as_array()) {
        for area in areas.iter().filter_map(|v| v.get("name")).filter_map(|v| v.as_str()) {
            if let Some(code) = cn_region(area) {
                if !media.regions.iter().any(|r| r == code) {
                    media.regions.push(code.into());
                }
            }
        }
    }
    if let Some(actors) = non_empty(result.get("actors").and_then(|v| v.as_str())) {
        for line in actors.lines().take(12) {
            let name = line.split(['：', ':']).next_back().unwrap_or(line).trim();
            if !name.is_empty() && !media.cast.iter().any(|c| c == name) {
                media.cast.push(name.to_string());
            }
        }
    }
    if let Some(staff) = non_empty(result.get("staff").and_then(|v| v.as_str())) {
        for line in staff.lines() {
            if let Some(rest) = line.strip_prefix("动画制作：").or_else(|| line.strip_prefix("动画制作:")) {
                let name = rest.trim();
                if !name.is_empty() {
                    media.studios.push(name.to_string());
                }
            }
        }
    }
    let mut out = MediaDraft {
        source: "bilibili".into(),
        external_id: item.season_id.clone(),
        media,
        ..Default::default()
    };
    if let Some(score) = score {
        out.ratings.push(CatalogRating {
            source: "bilibili".into(),
            category: "audience".into(),
            value: Some(score),
            scale: 10.0,
            votes,
            url: out.media.source_url.clone(),
            ..Default::default()
        });
    }
    // 详情里的每集时间同样进日历：时间线只覆盖前后 7 日，详情能补齐历史。
    let start = ctx.start.to_string();
    let end = ctx.end.to_string();
    for ep in episodes {
        let Some(seconds) = ep.get("pub_time").and_then(|v| v.as_i64()).filter(|s| *s > 0) else {
            continue;
        };
        let Some(date) = date_from_ts(seconds) else {
            continue;
        };
        if date < start || date >= end {
            continue;
        }
        let episode = non_empty(ep.get("show_title").and_then(|v| v.as_str()))
            .or_else(|| non_empty(ep.get("long_title").and_then(|v| v.as_str())))
            .or_else(|| non_empty(ep.get("title").and_then(|v| v.as_str())));
        let external_id = ep
            .get("id")
            .or_else(|| ep.get("ep_id"))
            .and_then(|v| v.as_i64())
            .map(|id| id.to_string());
        let url = non_empty(ep.get("link").and_then(|v| v.as_str()));
        out.releases.push(CatalogRelease {
            source: "bilibili".into(),
            external_id: external_id.unwrap_or_else(|| format!("{}:{date}", item.season_id)),
            date,
            local_time: DateTime::from_timestamp(seconds, 0)
                .map(|dt| dt.with_timezone(&plus_eight()).format("%H:%M").to_string()),
            timezone: Some("Asia/Shanghai".into()),
            episode,
            platform: Some("哔哩哔哩".into()),
            release_kind: "episode".into(),
            url,
            ..Default::default()
        });
    }
    Ok(out)
}

fn plus_eight() -> FixedOffset {
    FixedOffset::east_opt(8 * 3600).unwrap_or_else(|| FixedOffset::east_opt(0).expect("UTC 偏移"))
}

/// 时间线：与详情共用剧集 id，重复的日历行由入库层按 id 去重。
fn parse_timeline(kind: &'static str, body: &str) -> Result<Vec<MediaDraft>> {
    let value: Value = serde_json::from_str(body)
        .map_err(|_| AppError::new(ErrorKind::Provider, "哔哩哔哩时间线不是 JSON"))?;
    if let Some(code) = value.get("code").and_then(|v| v.as_i64()) {
        if code != 0 {
            let message = value.get("message").and_then(|v| v.as_str()).unwrap_or("未知错误");
            return Err(AppError::new(
                ErrorKind::Provider,
                format!("哔哩哔哩时间线返回错误：{message}"),
            ));
        }
    }
    let days = value
        .get("result")
        .and_then(|v| v.as_array())
        .ok_or_else(|| AppError::new(ErrorKind::Provider, "哔哩哔哩时间线缺少 result 数组"))?;
    let now_year = chrono::Local::now().year();
    let mut collected: Vec<(String, i64, MediaDraft)> = Vec::new();
    for day in days {
        let day_ts = day.get("date_ts").and_then(|v| v.as_i64()).unwrap_or(0);
        let day_date = day.get("date").and_then(|v| v.as_str()).unwrap_or("");
        let Some(episodes) = day.get("episodes").and_then(|v| v.as_array()) else {
            continue;
        };
        for ep in episodes {
            let Some(title) = non_empty(ep.get("title").and_then(|v| v.as_str())) else {
                continue;
            };
            let episode_id = ep.get("episode_id").and_then(|v| v.as_i64()).unwrap_or(0);
            if episode_id == 0 {
                continue;
            }
            let pub_ts = ep.get("pub_ts").and_then(|v| v.as_i64()).unwrap_or(day_ts);
            let air_date = date_from_ts(if day_ts > 0 { day_ts } else { pub_ts })
                .or_else(|| normalize_date(day_date, now_year));
            let season_id = ep
                .get("season_id")
                .and_then(|v| v.as_i64())
                .filter(|id| *id > 0)
                .map(|id| id.to_string())
                .unwrap_or_else(|| episode_id.to_string());
            let note = non_empty(ep.get("pub_index").and_then(|v| v.as_str()));
            let (episode_count, aired) = note.as_deref().map(episode_numbers).unwrap_or((None, None));
            let mut media = CatalogMedia {
                key: format!("bilibili:{season_id}"),
                source: "bilibili".into(),
                title,
                kind: kind.into(),
                platforms: vec!["哔哩哔哩".into()],
                status: "airing".into(),
                episode_count,
                aired_episodes: aired,
                year: year_of(air_date.as_deref()),
                poster: non_empty(ep.get("cover").and_then(|v| v.as_str())),
                ..Default::default()
            };
            // 时间线本身不说明制作地区；B 站同时分发海外作品。
            media.regions.clear();
            let mut draft = MediaDraft {
                source: "bilibili".into(),
                external_id: season_id.clone(),
                media,
                ..Default::default()
            };
            if let Some(date) = air_date {
                let external_id = if episode_id > 0 {
                    episode_id.to_string()
                } else {
                    format!("{season_id}:{date}")
                };
                draft.releases.push(CatalogRelease {
                    source: "bilibili".into(),
                    external_id,
                    date,
                    local_time: non_empty(ep.get("pub_time").and_then(|v| v.as_str())),
                    timezone: Some("Asia/Shanghai".into()),
                    episode: note,
                    platform: Some("哔哩哔哩".into()),
                    release_kind: "episode".into(),
                    ..Default::default()
                });
            }
            collected.push((draft.media.premiered.clone().unwrap_or_default(), pub_ts, draft));
        }
    }
    collected.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let mut map: BTreeMap<String, MediaDraft> = BTreeMap::new();
    for (_, _, draft) in collected {
        map.entry(draft.external_id.clone()).or_insert(draft);
    }
    Ok(map.into_values().collect())
}

/// B 站给的是东八区自然日的时间戳，按 +08 取日期才是用户看到的那一天。
fn date_from_ts(ts: i64) -> Option<String> {
    if ts <= 0 {
        return None;
    }
    let dt = Utc.timestamp_opt(ts, 0).single()?.with_timezone(&plus_eight());
    Some(dt.format("%Y-%m-%d").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    const INDEX_FIXTURE: &str = r#"{
      "code": 0,
      "data": {"has_next": 1, "list": [
        {"season_id": 47836, "title": "鬼灭之刃 柱训练篇", "cover": "https://i0.hdslb.com/x.png",
         "score": "9.7", "is_finish": 1, "index_show": "全8话",
         "link": "https://www.bilibili.com/bangumi/play/ss47836"},
        {"season_id": 0, "title": "无 id"},
        {"season_id": 9, "title": ""}
      ], "total": 4280}
    }"#;

    #[test]
    fn index_rows_are_filtered_and_scored() {
        let items = parse_index("anime", INDEX_FIXTURE).unwrap();
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.season_id, "47836");
        assert_eq!(item.title, "鬼灭之刃 柱训练篇");
        assert_eq!(item.score, Some(9.7));
        assert!(item.finished);
        assert_eq!(item.index_show.as_deref(), Some("全8话"));
    }

    const SEASON_FIXTURE: &str = r#"{
      "code": 0,
      "result": {
        "title": "鬼灭之刃 柱训练篇",
        "cover": "https://i0.hdslb.com/cover.png",
        "evaluate": "以集英社漫画为原作。",
        "styles": ["漫画改", "奇幻", "热血"],
        "areas": [{"id": 2, "name": "日本"}],
        "actors": "灶门炭治郎：花江夏树\n灶门祢豆子：鬼头明里",
        "staff": "导演：外崎春雄\n动画制作：ufotable",
        "total": 8,
        "media_id": 21226899,
        "publish": {"is_finish": 1, "is_started": 1, "pub_time": "2024-06-22 20:00:00"},
        "rating": {"count": 7614, "score": 9.7},
        "episodes": [
          {"id": 830494, "pub_time": 1719057600, "show_title": "第1话 为了消灭鬼舞辻无惨", "link": "https://www.bilibili.com/bangumi/play/ep830494"}
        ]
      }
    }"#;

    fn ctx() -> AdapterContext {
        AdapterContext {
            http: std::sync::Arc::new(crate::app::net::MockHttp::default()),
            start: NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            end: NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
            mode: "full".into(),
            cursor: None,
            max_pages: 1,
            countries: vec![],
            token: None,
        }
    }

    #[test]
    fn season_detail_fills_chinese_metadata_rating_and_release() {
        let item = IndexItem {
            season_id: "47836".into(),
            kind: "anime",
            title: "鬼灭之刃 柱训练篇".into(),
            poster: None,
            score: Some(9.7),
            finished: true,
            index_show: Some("全8话".into()),
            link: None,
        };
        let draft = parse_season(&ctx(), &item, SEASON_FIXTURE).unwrap();
        assert_eq!(draft.media.title, "鬼灭之刃 柱训练篇");
        assert_eq!(draft.media.summary.as_deref(), Some("以集英社漫画为原作。"));
        assert_eq!(draft.media.genres, vec!["漫画改", "奇幻", "热血"]);
        assert_eq!(draft.media.regions, vec!["JP"]);
        assert_eq!(draft.media.episode_count, Some(8));
        assert_eq!(draft.media.aired_episodes, Some(1));
        assert_eq!(draft.media.status, "completed");
        assert_eq!(draft.media.premiered.as_deref(), Some("2024-06-22"));
        assert_eq!(draft.media.studios, vec!["ufotable"]);
        assert_eq!(draft.ratings.len(), 1);
        assert_eq!(draft.ratings[0].value, Some(9.7));
        assert_eq!(draft.ratings[0].votes, Some(7614));
        assert_eq!(draft.releases.len(), 1);
        assert_eq!(draft.releases[0].date, "2024-06-22");
    }

    #[test]
    fn timeline_keeps_each_episode_and_marks_airing() {
        let body = r#"{"code":0,"result":[{"date":"11-17","date_ts":1794844800,"episodes":[
            {"episode_id":7702237,"pub_index":"第15话~第16话","pub_time":"10:00","pub_ts":1794880800,"season_id":355139,"title":"小羊肖恩 第七季"}
        ]}]}"#;
        let items = parse_timeline("anime", body).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].external_id, "355139");
        assert_eq!(items[0].media.status, "airing");
        assert_eq!(items[0].releases.len(), 1);
        assert_eq!(items[0].releases[0].local_time.as_deref(), Some("10:00"));
    }
}
