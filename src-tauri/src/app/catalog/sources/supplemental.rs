use super::{json, non_empty, pace, publish, text, year_of, AdapterContext};
use crate::app::catalog::types::{
    CatalogExternalId, CatalogMedia, CatalogRating, CatalogRelease, CatalogSeason, IngestionBatch,
    MediaDraft,
};
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::HttpRequest;
use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc::Sender;

const BANGUMI_BROWSE: &str = "https://api.bgm.tv/v0/subjects";
const SIMKL_CALENDAR: &str = "https://data.simkl.in/calendar/anime.json";
const SIMKL_MOVIE_CALENDAR: &str = "https://data.simkl.in/calendar/movie_release.json";
const SIMKL_TV_CALENDAR: &str = "https://data.simkl.in/calendar/tv.json";
const METADATA_BATCH: usize = 20;
const BANGUMI_BATCH: usize = 50;
const SNAPSHOT_BATCH: usize = 100;
const TMDB_PAGE_LIMIT: u64 = 500;
const BANGUMI_NOTE: &str = "覆盖 Bangumi 全站动漫目录（含已完结与在播），中文名、中文简介、类型与评分来自作品页；放送星期不是单集日期，不生成推测日历。";
const SIMKL_NOTE: &str = "覆盖 SIMKL 公开动漫/电影/电视日历快照；按原始固定偏移保留当地日期，电影记为首映、电视带季集号。快照会变化，续传按当前位置继续；制作国家未知时留空，需授权的 API 端点不采集。";
const KITSU_NOTE: &str = "仅覆盖 Kitsu 当前动漫及漫画目录；元数据更新时间不是单集或章节发布时间，不生成推测日历。";
const TMDB_NOTE: &str = "仅覆盖指定首播日期范围的 TMDb 发现目录；电视只保留详情中日期确认的最近/下一集，不展开历史季。电影首映与单集更新分开记录；日期资料没有时区。";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    source: String,
    phase: String,
    offset: u64,
    #[serde(default)]
    next: Option<String>,
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    end: Option<String>,
}

fn provider(source: &str, field: &str) -> AppError {
    // 不回显响应或游标：外部数据可能含敏感字段，错误只指出契约位置。
    AppError::new(ErrorKind::Provider, format!("{source} 返回资料缺失或格式错误：{field}"))
}

fn resume(ctx: &AdapterContext, source: &str, phase: &str, offset: u64, dated: bool) -> Result<Cursor> {
    let start = dated.then(|| ctx.start.to_string());
    let end = dated.then(|| ctx.end.to_string());
    let Some(raw) = ctx.cursor.as_deref() else {
        return Ok(Cursor {
            source: source.into(), phase: phase.into(), offset, next: None, start, end,
        });
    };
    let cursor: Cursor = serde_json::from_str(raw)
        .map_err(|_| AppError::new(ErrorKind::Invalid, "资料库续传游标格式错误"))?;
    if cursor.source != source || cursor.start != start || cursor.end != end
        || (source != "kitsu" && cursor.next.is_some())
    {
        return Err(AppError::new(ErrorKind::Invalid, "资料库续传游标与本次数据源或日期范围不符"));
    }
    Ok(cursor)
}

fn range(ctx: &AdapterContext) -> Result<()> {
    if ctx.start >= ctx.end {
        return Err(AppError::new(ErrorKind::Invalid, "资料库结束日期必须晚于开始日期"));
    }
    Ok(())
}

async fn get(ctx: &AdapterContext, req: HttpRequest) -> Result<Value> {
    let value = json(&ctx.http, req).await?;
    pace().await;
    Ok(value)
}

async fn save(
    tx: &Sender<IngestionBatch>, items: Vec<MediaDraft>, page: u32, next: Option<&Cursor>,
    note: &str, budget: bool,
) -> Result<()> {
    let cursor = next.map(serde_json::to_string).transpose()
        .map_err(|_| AppError::new(ErrorKind::Invalid, "资料库续传游标无法编码"))?;
    let warning = if budget && next.is_some() {
        format!("{note} 本次分页预算已用尽，可从游标继续；分页尚未完成。")
    } else {
        note.to_string()
    };
    publish(tx, IngestionBatch {
        items, page, cursor, complete: next.is_none(),
        http_status: (page > 0).then_some(200), warning: Some(warning),
    }).await
}

fn array<'a>(value: &'a Value, source: &str, field: &str) -> Result<&'a Vec<Value>> {
    value.as_array().ok_or_else(|| provider(source, field))
}

fn optional_array<'a>(value: &'a Value, source: &str, field: &str) -> Result<Option<&'a Vec<Value>>> {
    if value.is_null() { return Ok(None); }
    array(value, source, field).map(Some)
}

fn object(value: &Value, source: &str, field: &str) -> Result<()> {
    if value.is_object() { Ok(()) } else { Err(provider(source, field)) }
}

fn string(value: &Value) -> Option<String> { non_empty(value.as_str()) }

fn id(value: &Value, source: &str, field: &str) -> Result<String> {
    let raw = text(value).ok_or_else(|| provider(source, field))?;
    if !raw.bytes().all(|b| b.is_ascii_digit()) { return Err(provider(source, field)); }
    let number: u64 = raw.parse().map_err(|_| provider(source, field))?;
    if number == 0 { return Err(provider(source, field)); }
    Ok(number.to_string())
}

fn integer(value: &Value, source: &str, field: &str) -> Result<u64> {
    value.as_u64().or_else(|| value.as_str().and_then(|v| v.parse().ok()))
        .ok_or_else(|| provider(source, field))
}

fn count(value: &Value, source: &str, field: &str) -> Result<Option<u32>> {
    if value.is_null() || value.as_str().is_some_and(|s| s.trim().is_empty()) { return Ok(None); }
    let number = integer(value, source, field)?;
    u32::try_from(number).map(Some).map_err(|_| provider(source, field))
}

fn score(value: &Value, scale: f64, source: &str, field: &str) -> Result<Option<f64>> {
    if value.is_null() || value.as_str().is_some_and(|s| s.trim().is_empty()) { return Ok(None); }
    let number = value.as_f64().or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .filter(|n| n.is_finite() && *n >= 0.0 && *n <= scale)
        .ok_or_else(|| provider(source, field))?;
    Ok(Some(number))
}

fn date(value: &Value, source: &str, field: &str) -> Result<Option<String>> {
    if value.is_null() { return Ok(None); }
    let raw = value.as_str().ok_or_else(|| provider(source, field))?.trim();
    if raw.is_empty() { return Ok(None); }
    if raw.len() != 10 { return Err(provider(source, field)); }
    let parsed = NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| provider(source, field))?;
    Ok(Some(parsed.to_string()))
}

fn date_or_timestamp(value: &Value, source: &str, field: &str) -> Result<Option<String>> {
    if let Some(raw) = value.as_str().filter(|s| s.len() > 10) {
        return DateTime::parse_from_rfc3339(raw).map(|v| Some(v.date_naive().to_string()))
            .map_err(|_| provider(source, field));
    }
    date(value, source, field)
}

fn unique(values: &mut Vec<String>, value: Option<String>) {
    if let Some(value) = value {
        if !values.contains(&value) { values.push(value); }
    }
}

fn draft(source: &str, external_id: String, mut media: CatalogMedia) -> MediaDraft {
    media.key = format!("{source}:{external_id}");
    media.source = source.into();
    MediaDraft { source: source.into(), external_id, media, ..Default::default() }
}

fn external(out: &mut MediaDraft, source: &str, external_id: String) {
    let value = CatalogExternalId { source: source.into(), external_id };
    if !out.external_ids.contains(&value) { out.external_ids.push(value); }
}

fn public_url(value: &Value, base: &str) -> Option<String> {
    let raw = string(value)?;
    let root = Url::parse(base).ok()?;
    let url = root.join(&raw).ok()?;
    (url.scheme() == "https" && url.host_str() == root.host_str()
        && url.username().is_empty() && url.password().is_none()
        && url.port_or_known_default() == Some(443))
        .then(|| url.to_string())
}

fn infobox_values(value: &Value) -> Vec<String> {
    if let Some(value) = string(value) { return vec![value]; }
    value.as_array().map(|values| values.iter().filter_map(|v| {
        string(v).or_else(|| string(&v["v"])).or_else(|| string(&v["value"]))
    }).collect()).unwrap_or_default()
}

fn country(raw: &str) -> Option<String> {
    let code = match raw.trim() {
        "日本" | "Japan" | "JP" => "JP",
        "中国" | "中国大陆" | "中國" | "China" | "CN" => "CN",
        "美国" | "美國" | "United States" | "USA" | "US" => "US",
        "英国" | "英國" | "United Kingdom" | "UK" | "GB" => "GB",
        "韩国" | "韓國" | "South Korea" | "KR" => "KR",
        "台湾" | "台灣" | "Taiwan" | "TW" => "TW",
        "香港" | "Hong Kong" | "HK" => "HK",
        "法国" | "法國" | "France" | "FR" => "FR",
        "德国" | "德國" | "Germany" | "DE" => "DE",
        "加拿大" | "Canada" | "CA" => "CA",
        "澳大利亚" | "Australia" | "AU" => "AU",
        _ => return None,
    };
    Some(code.into())
}

fn bangumi_item(value: &Value) -> Result<MediaDraft> {
    object(value, "Bangumi", "subject")?;
    let native_id = id(&value["id"], "Bangumi", "subject.id")?;
    let native = string(&value["name"]);
    let chinese = string(&value["name_cn"]);
    let title = chinese.clone().or(native.clone()).ok_or_else(|| provider("Bangumi", "subject.name"))?;
    let premiered = date(value.get("date").unwrap_or(&value["air_date"]), "Bangumi", "subject.date")?;
    let mut media = CatalogMedia {
        title, original_title: native.clone(), kind: "anime".into(),
        subkind: text(&value["platform"]), summary: string(&value["summary"]),
        poster: string(&value["images"]["large"]).or_else(|| string(&value["images"]["common"])),
        year: year_of(premiered.as_deref()), premiered,
        episode_count: bangumi_episodes(value)?,
        status: "unknown".into(), source_url: Some(format!("https://bgm.tv/subject/{native_id}")),
        ..Default::default()
    };
    unique(&mut media.aliases, native);
    unique(&mut media.aliases, chinese);
    let today = chrono::Local::now().date_naive().to_string();
    let mut ended = false;
    let mut genres = Vec::new();
    if let Some(infobox) = optional_array(&value["infobox"], "Bangumi", "subject.infobox")? {
        for entry in infobox {
            // infobox 是可选富化：v0 用 key、旧接口用 name，缺字段的条目跳过而非整源失败。
            let Some(name) = string(&entry["name"]).or_else(|| string(&entry["key"])) else { continue; };
            let values = infobox_values(&entry["value"]);
            match name.as_str() {
                "国家" | "国家/地区" | "country" | "Country" | "产地" => {
                    for value in values {
                        for part in value.split(['/', '、', ',', '，', ';', '；']) {
                            unique(&mut media.regions, country(part));
                        }
                    }
                }
                "类型" | "题材" | "类型/题材" | "genre" | "Genre" => {
                    for value in values {
                        for part in value.split(['、', '/', ',', '，', ';', '；']) {
                            let part = part.trim();
                            if !part.is_empty() && !genres.iter().any(|g| g == part) {
                                genres.push(part.to_string());
                            }
                        }
                    }
                }
                "放送结束" | "结束" | "完结" => {
                    ended = values.iter().any(|v| !v.trim().is_empty());
                }
                "导演" | "監督" => for value in values { unique(&mut media.directors, Some(value)); },
                "脚本" | "系列构成" => for value in values { unique(&mut media.writers, Some(value)); },
                "动画制作" | "動畫製作" | "制作公司" => for value in values { unique(&mut media.studios, Some(value)); },
                "别名" | "別名" => for value in values { unique(&mut media.aliases, Some(value)); },
                _ => {},
            }
        }
    }
    if genres.is_empty() {
        if let Some(tags) = value["meta_tags"].as_array() {
            for tag in tags.iter().filter_map(|v| v.as_str()) {
                if bangumi_noise_tag(tag) || genres.iter().any(|g| g == tag) {
                    continue;
                }
                genres.push(tag.to_string());
            }
        }
    }
    media.genres = genres;
    // meta_tags 里的国家名同样映射成地区；与 infobox 的「国家」去重。
    if let Some(tags) = value["meta_tags"].as_array() {
        for tag in tags.iter().filter_map(|v| v.as_str()) {
            if let Some(code) = country(tag) {
                if !media.regions.iter().any(|r| r == &code) {
                    media.regions.push(code);
                }
            }
        }
    }
    media.status = if ended {
        "completed".into()
    } else if media.premiered.as_deref().is_some_and(|d| d > today.as_str()) {
        "upcoming".into()
    } else if media
        .premiered
        .as_deref()
        .is_some_and(|d| bangumi_older_than(d, &today, 730))
    {
        // Bangumi 没有状态字段，只有 infobox 的「放送结束」。两年以前开播又没写结束的
        // 绝大多数是已完结旧作（OVA/特别篇常见）；长连载是少数例外，宁可判完结也不谎报在播。
        "completed".into()
    } else if media.premiered.is_some() {
        "airing".into()
    } else {
        "unknown".into()
    };
    let mut out = draft("bangumi", native_id, media);
    if !value["rating"].is_null() {
        object(&value["rating"], "Bangumi", "subject.rating")?;
        out.ratings.push(CatalogRating {
            source: "bangumi".into(), category: "audience".into(),
            value: score(&value["rating"]["score"], 10.0, "Bangumi", "rating.score")?, scale: 10.0,
            votes: count(&value["rating"]["total"], "Bangumi", "rating.total")?,
            rank: count(&value["rating"]["rank"], "Bangumi", "rating.rank")?,
            url: out.media.source_url.clone(), ..Default::default()
        });
    }
    Ok(out)
}

/// `total_episodes` 在浏览接口里常是 0，`eps` 才是话数；两个都不给才留空。
fn bangumi_episodes(value: &Value) -> Result<Option<u32>> {
    for field in ["total_episodes", "eps"] {
        if let Some(count) = count(&value[field], "Bangumi", "subject.episodes")? {
            if count > 0 {
                return Ok(Some(count));
            }
        }
    }
    Ok(None)
}

fn bangumi_older_than(date: &str, today: &str, days: i64) -> bool {
    let (Ok(date), Ok(today)) = (
        NaiveDate::parse_from_str(date, "%Y-%m-%d"),
        NaiveDate::parse_from_str(today, "%Y-%m-%d"),
    ) else {
        return false;
    };
    today.signed_duration_since(date).num_days() > days
}

/// Bangumi 的标签里混着放送形态、国家和改编方式，不是题材。
fn bangumi_noise_tag(tag: &str) -> bool {
    matches!(
        tag,
        "TV" | "WEB" | "OVA" | "OAD" | "剧场版" | "动画" | "日本" | "中国" | "韩国" | "美国"
            | "原创" | "漫画改" | "小说改" | "游戏改" | "音乐" | "CM"
    )
}

pub async fn bangumi(ctx: AdapterContext, tx: Sender<IngestionBatch>) -> Result<()> {
    let mut cursor = resume(&ctx, "bangumi", "browse", 0, false)?;
    // 旧版本按周历采集，游标阶段不同：换新口径时从头开始，不误报整源失败。
    if cursor.phase != "browse" {
        cursor.phase = "browse".into();
        cursor.offset = 0;
    }
    if ctx.max_pages == 0 {
        return save(&tx, vec![], 0, Some(&cursor), BANGUMI_NOTE, true).await;
    }
    for page in 1..=ctx.max_pages {
        let mut url = Url::parse(BANGUMI_BROWSE)
            .map_err(|_| AppError::new(ErrorKind::Invalid, "Bangumi 地址配置无效"))?;
        url.query_pairs_mut()
            .append_pair("type", "2")
            .append_pair("limit", &BANGUMI_BATCH.to_string())
            .append_pair("offset", &cursor.offset.to_string());
        let payload = get(&ctx, HttpRequest::get(url.as_str())).await?;
        let rows = array(&payload["data"], "Bangumi", "data")?;
        if rows.is_empty() {
            return save(&tx, vec![], page, None, BANGUMI_NOTE, false).await;
        }
        let mut items = Vec::with_capacity(rows.len());
        for subject in rows {
            if subject["nsfw"].as_bool() == Some(true) {
                continue;
            }
            items.push(bangumi_item(subject)?);
        }
        let total = payload["total"].as_u64().unwrap_or(0);
        let next_offset = cursor.offset + rows.len() as u64;
        let more = next_offset < total;
        let mut next = cursor.clone();
        next.offset = next_offset;
        save(&tx, items, page, more.then_some(&next), BANGUMI_NOTE, page == ctx.max_pages).await?;
        if !more {
            return Ok(());
        }
        cursor = next;
    }
    Ok(())
}

fn simkl_item(value: &Value, ctx: &AdapterContext, kind: &str) -> Result<Option<MediaDraft>> {
    object(value, "SIMKL", "calendar.item")?;
    let native_id = id(&value["ids"]["simkl_id"], "SIMKL", "ids.simkl_id")?;
    let title = string(&value["title"]).ok_or_else(|| provider("SIMKL", "title"))?;
    let raw_date = value["date"].as_str().ok_or_else(|| provider("SIMKL", "date"))?;
    let timestamp = DateTime::parse_from_rfc3339(raw_date).map_err(|_| provider("SIMKL", "date"))?;
    let local_date = timestamp.date_naive();
    let premiered = date_or_timestamp(&value["release_date"], "SIMKL", "release_date")?;
    let url = public_url(&value["url"], "https://simkl.com/")
        .or_else(|| Some(format!("https://simkl.com/{kind}/{native_id}/")));
    let mut media = CatalogMedia {
        title, kind: kind.into(), year: year_of(premiered.as_deref()), premiered,
        status: "unknown".into(), source_url: url.clone(), ..Default::default()
    };
    if kind == "anime" { media.subkind = text(&value["anime_type"]); }
    let mut out = draft("simkl", native_id.clone(), media);
    // MAL 仅动漫；IMDb 电影/电视常有；TMDb 按类型区分电视/电影命名空间，避免同号串类。
    if kind == "anime" && !value["ids"]["mal"].is_null() {
        external(&mut out, "mal", id(&value["ids"]["mal"], "SIMKL", "ids.mal")?);
    }
    if !value["ids"]["tmdb"].is_null() {
        let tmdb = id(&value["ids"]["tmdb"], "SIMKL", "ids.tmdb")?;
        external(&mut out, "tmdb", tmdb.clone());
        external(&mut out, if kind == "movie" { "tmdb_movie" } else { "tmdb_tv" }, tmdb);
    }
    if let Some(imdb) = string(&value["ids"]["imdb"]) {
        if imdb.strip_prefix("tt").is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())) {
            external(&mut out, "imdb", imdb);
        }
    }
    if !value["ratings"].is_null() { object(&value["ratings"], "SIMKL", "ratings")?; }
    if !value["ratings"]["simkl"].is_null() {
        let rating = &value["ratings"]["simkl"];
        object(rating, "SIMKL", "ratings.simkl")?;
        // 无实际分值不建评分行，避免「暂无评分」占位被当成一条来源评分。
        if let Some(score_value) = score(&rating["rating"], 10.0, "SIMKL", "ratings.simkl.rating")? {
            out.ratings.push(CatalogRating {
                source: "simkl".into(), category: "user".into(),
                value: Some(score_value), scale: 10.0,
                votes: count(&rating["votes"], "SIMKL", "ratings.simkl.votes")?,
                rank: count(&value["rank"], "SIMKL", "rank")?, url: url.clone(), ..Default::default()
            });
        }
    }
    if local_date < ctx.start || local_date >= ctx.end { return Ok(None); }
    let timezone = Some(if raw_date.ends_with('Z') { "Z".into() } else { timestamp.offset().to_string() });
    let utc_time = Some(timestamp.with_timezone(&Utc).to_rfc3339_opts(SecondsFormat::AutoSi, true));
    let local_time = Some(timestamp.format("%H:%M:%S%.f").to_string());
    // 稳定身份不含日期：改期更新原行。电影是首映，电视/动漫是单集。
    let release = match kind {
        "movie" => CatalogRelease {
            key: format!("simkl:{native_id}:premiere"), media_key: out.media.key.clone(),
            source: "simkl".into(), external_id: format!("{native_id}:premiere"),
            date: local_date.to_string(), local_time, timezone, utc_time,
            release_kind: "premiere".into(), url, ..Default::default()
        },
        "tv" => {
            let season = count(&value["episode"]["season"], "SIMKL", "episode.season")?;
            let episode = text(&value["episode"]["episode"]).ok_or_else(|| provider("SIMKL", "episode.episode"))?;
            let release_id = format!("{native_id}:s{}:{episode}", season.map(|s| s.to_string()).unwrap_or_default());
            CatalogRelease {
                key: format!("simkl:{release_id}"), media_key: out.media.key.clone(),
                source: "simkl".into(), external_id: release_id, date: local_date.to_string(),
                local_time, timezone, utc_time, season, episode: Some(episode),
                release_kind: "episode".into(),
                url: public_url(&value["episode"]["url"], "https://simkl.com/").or(url), ..Default::default()
            }
        },
        _ => {
            let episode = text(&value["episode"]["episode"]).ok_or_else(|| provider("SIMKL", "episode.episode"))?;
            let release_id = format!("{native_id}:{episode}");
            CatalogRelease {
                key: format!("simkl:{release_id}"), media_key: out.media.key.clone(),
                source: "simkl".into(), external_id: release_id, date: local_date.to_string(),
                local_time, timezone, utc_time, episode: Some(episode),
                release_kind: "episode".into(),
                url: public_url(&value["episode"]["url"], "https://simkl.com/").or(url), ..Default::default()
            }
        }
    };
    out.releases.push(release);
    Ok(Some(out))
}

fn simkl_phase_url(phase: &str) -> &'static str {
    match phase { "movie" => SIMKL_MOVIE_CALENDAR, "tv" => SIMKL_TV_CALENDAR, _ => SIMKL_CALENDAR }
}
fn simkl_next_phase(phase: &str) -> &'static str {
    match phase { "anime" => "movie", "movie" => "tv", _ => "done" }
}

pub async fn simkl(ctx: AdapterContext, tx: Sender<IngestionBatch>) -> Result<()> {
    range(&ctx)?;
    let mut cursor = resume(&ctx, "simkl", "anime", 0, true)?;
    if !matches!(cursor.phase.as_str(), "anime" | "movie" | "tv") {
        return Err(AppError::new(ErrorKind::Invalid, "SIMKL 续传阶段错误"));
    }
    if ctx.max_pages == 0 { return save(&tx, vec![], 0, Some(&cursor), SIMKL_NOTE, true).await; }
    // 每个相快照只取一次，按批落库；相位推进时再取下一份，避免重复下载大快照。
    let mut rows: Vec<Value> = Vec::new();
    let mut loaded = String::new();
    for page in 1..=ctx.max_pages {
        let kind = cursor.phase.clone();
        if loaded != kind {
            let payload = get(&ctx, HttpRequest::get(simkl_phase_url(&kind))).await?;
            rows = array(&payload, "SIMKL", "calendar")?.clone();
            loaded = kind.clone();
        }
        let offset = usize::try_from(cursor.offset)
            .map_err(|_| AppError::new(ErrorKind::Invalid, "SIMKL 续传位置超出范围"))?;
        let end = offset.saturating_add(SNAPSHOT_BATCH).min(rows.len());
        let mut items = Vec::new();
        if offset < rows.len() {
            for row in &rows[offset..end] {
                if let Some(item) = simkl_item(row, &ctx, &kind)? { items.push(item); }
            }
        }
        let mut next = cursor.clone();
        if end < rows.len() {
            next.offset = end as u64;
        } else {
            next.phase = simkl_next_phase(&kind).into();
            next.offset = 0;
        }
        let done = next.phase == "done";
        save(&tx, items, page, (!done).then_some(&next), SIMKL_NOTE, page == ctx.max_pages).await?;
        if done { return Ok(()); }
        cursor = next;
    }
    Ok(())
}

fn kitsu_first(phase: &str) -> String {
    format!("https://kitsu.io/api/edge/{phase}?filter%5Bstatus%5D=current&page%5Blimit%5D=20")
}

fn kitsu_link(raw: &str, phase: &str) -> Result<(String, u64)> {
    let root = Url::parse("https://kitsu.io/").map_err(|_| provider("Kitsu", "pagination base"))?;
    let url = root.join(raw).map_err(|_| provider("Kitsu", "links.next"))?;
    if url.scheme() != "https" || url.host_str() != Some("kitsu.io")
        || url.port_or_known_default() != Some(443) || !url.username().is_empty()
        || url.password().is_some() || url.fragment().is_some()
        || url.path() != format!("/api/edge/{phase}")
    {
        return Err(provider("Kitsu", "links.next trusted origin/path"));
    }
    let mut offset = None;
    let mut limit = None;
    let mut current = false;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "page[offset]" if offset.is_none() => offset = Some(value.parse::<u64>().map_err(|_| provider("Kitsu", "links.next.offset"))?),
            "page[limit]" if limit.is_none() => limit = Some(value.parse::<usize>().map_err(|_| provider("Kitsu", "links.next.limit"))?),
            "filter[status]" if !current && value == "current" => current = true,
            _ => return Err(provider("Kitsu", "links.next.query")),
        }
    }
    if !current || !limit.is_some_and(|v| (1..=METADATA_BATCH).contains(&v)) {
        return Err(provider("Kitsu", "links.next.current/limit"));
    }
    Ok((url.to_string(), offset.unwrap_or(0)))
}

fn kitsu_item(value: &Value, phase: &str) -> Result<MediaDraft> {
    object(value, "Kitsu", "data.item")?;
    if value["type"].as_str() != Some(phase) { return Err(provider("Kitsu", "data.type")); }
    let native_id = id(&value["id"], "Kitsu", "data.id")?;
    let attrs = &value["attributes"];
    object(attrs, "Kitsu", "data.attributes")?;
    if !attrs["titles"].is_null() { object(&attrs["titles"], "Kitsu", "attributes.titles")?; }
    let english = string(&attrs["titles"]["en"]).or_else(|| string(&attrs["titles"]["en_us"]))
        .or_else(|| string(&attrs["titles"]["en_gb"]));
    let native = string(&attrs["titles"]["ja_jp"]).or_else(|| string(&attrs["titles"]["en_jp"]));
    let title = string(&attrs["canonicalTitle"]).or(english.clone()).or(native.clone())
        .ok_or_else(|| provider("Kitsu", "attributes.canonicalTitle/titles"))?;
    let premiered = date(&attrs["startDate"], "Kitsu", "attributes.startDate")?;
    let status = match attrs["status"].as_str() {
        // 应用统一用「更新中」表达连载；ongoing 是 Kitsu 自己的词，不直接透传给界面。
        Some("current") => "airing",
        Some("finished") => "completed",
        Some("upcoming" | "unreleased") => "upcoming",
        Some("tba") => "unknown",
        _ => "unknown",
    };
    let mut media = CatalogMedia {
        title, original_title: native, english_title: english, kind: phase.into(),
        subkind: string(&attrs["subtype"]), summary: string(&attrs["synopsis"]),
        poster: string(&attrs["posterImage"]["original"]), backdrop: string(&attrs["coverImage"]["original"]),
        status: status.into(), year: year_of(premiered.as_deref()), premiered,
        ended: date(&attrs["endDate"], "Kitsu", "attributes.endDate")?,
        episode_count: count(&attrs["episodeCount"], "Kitsu", "attributes.episodeCount")?,
        chapter_count: count(&attrs["chapterCount"], "Kitsu", "attributes.chapterCount")?,
        runtime: count(&attrs["episodeLength"], "Kitsu", "attributes.episodeLength")?,
        source_url: Some(format!("https://kitsu.io/{phase}/{native_id}")), ..Default::default()
    };
    if let Some(titles) = attrs["titles"].as_object() {
        for value in titles.values() { unique(&mut media.aliases, string(value)); }
    }
    unique(&mut media.aliases, string(&attrs["canonicalTitle"]));
    if let Some(values) = optional_array(&attrs["abbreviatedTitles"], "Kitsu", "attributes.abbreviatedTitles")? {
        for value in values {
            unique(&mut media.aliases, Some(string(value).ok_or_else(|| provider("Kitsu", "attributes.abbreviatedTitles.item"))?));
        }
    }
    let mut out = draft("kitsu", format!("{phase}:{native_id}"), media);
    external(&mut out, &format!("kitsu_{phase}"), native_id);
    let rating = score(&attrs["averageRating"], 100.0, "Kitsu", "attributes.averageRating")?;
    let votes = if attrs["ratingFrequencies"].is_null() { None } else {
        let frequencies = attrs["ratingFrequencies"].as_object()
            .ok_or_else(|| provider("Kitsu", "attributes.ratingFrequencies"))?;
        let mut total = 0u32;
        for value in frequencies.values() {
            let number = count(value, "Kitsu", "attributes.ratingFrequencies.count")?
                .ok_or_else(|| provider("Kitsu", "attributes.ratingFrequencies.count"))?;
            total = total.checked_add(number).ok_or_else(|| provider("Kitsu", "attributes.ratingFrequencies.total"))?;
        }
        Some(total)
    };
    if rating.is_some() || votes.is_some() {
        out.ratings.push(CatalogRating {
            source: "kitsu".into(), category: "user".into(), value: rating, scale: 100.0,
            votes, url: out.media.source_url.clone(), ..Default::default()
        });
    }
    Ok(out)
}

pub async fn kitsu(ctx: AdapterContext, tx: Sender<IngestionBatch>) -> Result<()> {
    let mut cursor = resume(&ctx, "kitsu", "anime", 0, false)?;
    if !matches!(cursor.phase.as_str(), "anime" | "manga") {
        return Err(AppError::new(ErrorKind::Invalid, "Kitsu 续传阶段错误"));
    }
    if ctx.max_pages == 0 { return save(&tx, vec![], 0, Some(&cursor), KITSU_NOTE, true).await; }
    for page in 1..=ctx.max_pages {
        let url = match cursor.next.as_deref() {
            Some(raw) => {
                let (url, offset) = kitsu_link(raw, &cursor.phase)?;
                if offset != cursor.offset { return Err(AppError::new(ErrorKind::Invalid, "Kitsu 续传位置不符")); }
                url
            }
            None if cursor.offset == 0 => kitsu_first(&cursor.phase),
            None => return Err(AppError::new(ErrorKind::Invalid, "Kitsu 续传缺少来源分页链接")),
        };
        let payload = get(&ctx, HttpRequest::get(url)).await?;
        object(&payload, "Kitsu", "response")?;
        let rows = array(&payload["data"], "Kitsu", "data")?;
        if rows.len() > METADATA_BATCH { return Err(provider("Kitsu", "data page limit")); }
        object(&payload["links"], "Kitsu", "links")?;
        let items = rows.iter().map(|v| kitsu_item(v, &cursor.phase)).collect::<Result<Vec<_>>>()?;
        let next_value = &payload["links"]["next"];
        let next = if !next_value.is_null() {
            let raw = next_value.as_str().or_else(|| next_value["href"].as_str())
                .ok_or_else(|| provider("Kitsu", "links.next"))?;
            let (url, offset) = kitsu_link(raw, &cursor.phase)?;
            if rows.is_empty() || offset <= cursor.offset { return Err(provider("Kitsu", "links.next must advance")); }
            let mut next = cursor.clone();
            next.offset = offset;
            next.next = Some(url);
            Some(next)
        } else if cursor.phase == "anime" {
            let mut next = cursor.clone();
            next.phase = "manga".into(); next.offset = 0; next.next = None;
            Some(next)
        } else { None };
        save(&tx, items, page, next.as_ref(), KITSU_NOTE, page == ctx.max_pages).await?;
        match next { Some(next) => cursor = next, None => return Ok(()) }
    }
    Ok(())
}

fn tmdb_request(ctx: &AdapterContext, url: String) -> Result<HttpRequest> {
    let token = ctx.token.as_deref().map(str::trim).filter(|v| !v.is_empty())
        .ok_or_else(|| AppError::new(ErrorKind::Invalid, "TMDb 需要 API Read Access Token"))?;
    if token.bytes().any(|b| b.is_ascii_control() || b.is_ascii_whitespace()) {
        return Err(AppError::new(ErrorKind::Invalid, "TMDb API Read Access Token 格式错误"));
    }
    Ok(HttpRequest::get(url).header("Authorization", format!("Bearer {token}")))
}

fn tmdb_discover(ctx: &AdapterContext, phase: &str, page: u64) -> Result<String> {
    let end = ctx.end.pred_opt().ok_or_else(|| AppError::new(ErrorKind::Invalid, "TMDb 日期范围错误"))?;
    let field = if phase == "tv" { "first_air_date" } else { "primary_release_date" };
    Ok(format!(
        "https://api.themoviedb.org/3/discover/{phase}?language=zh-CN&{field}.gte={}&{field}.lte={end}&sort_by={field}.asc&page={page}",
        ctx.start
    ))
}

fn tmdb_detail(phase: &str, native_id: &str) -> String {
    format!("https://api.themoviedb.org/3/{phase}/{native_id}?language=zh-CN&append_to_response=external_ids,credits,watch/providers,translations")
}

fn tmdb_image(value: &Value) -> Result<Option<String>> {
    let Some(path) = string(value) else { return Ok(None); };
    if !path.starts_with('/') || path.starts_with("//") || path.contains(['?', '#', '\\'])
        || path.split('/').any(|p| p == "..")
    {
        return Err(provider("TMDb", "image path"));
    }
    Ok(Some(format!("https://image.tmdb.org/t/p/w500{path}")))
}

fn named_values(value: &Value, field: &str) -> Result<Vec<String>> {
    let mut names = Vec::new();
    if let Some(values) = optional_array(value, "TMDb", field)? {
        for value in values {
            object(value, "TMDb", field)?;
            unique(&mut names, string(&value["name"]));
        }
    }
    Ok(names)
}

fn tmdb_episode(out: &mut MediaDraft, value: &Value, ctx: &AdapterContext) -> Result<()> {
    if value.is_null() { return Ok(()); }
    object(value, "TMDb", "episode")?;
    let Some(air_date) = date(&value["air_date"], "TMDb", "episode.air_date")? else { return Ok(()); };
    let parsed = NaiveDate::parse_from_str(&air_date, "%Y-%m-%d").map_err(|_| provider("TMDb", "episode.air_date"))?;
    if parsed < ctx.start || parsed >= ctx.end { return Ok(()); }
    let native_id = id(&value["id"], "TMDb", "episode.id")?;
    let season = count(&value["season_number"], "TMDb", "episode.season_number")?;
    let number = count(&value["episode_number"], "TMDb", "episode.episode_number")?;
    // TMDb 单集 ID 全局唯一且跨改期稳定，日期只作为字段更新。
    let release_id = format!("{}:episode:{native_id}", out.external_id);
    if out.releases.iter().any(|r| r.external_id == release_id) { return Ok(()); }
    out.releases.push(CatalogRelease {
        key: format!("tmdb:{release_id}"), media_key: out.media.key.clone(), source: "tmdb".into(),
        external_id: release_id, date: air_date, season, episode: number.map(|v| v.to_string()),
        title: string(&value["name"]), release_kind: "episode".into(),
        url: out.media.source_url.clone(), ..Default::default()
    });
    Ok(())
}

fn tmdb_item(value: &Value, phase: &str, ctx: &AdapterContext) -> Result<MediaDraft> {
    object(value, "TMDb", "detail")?;
    let native_id = id(&value["id"], "TMDb", "detail.id")?;
    let tv = phase == "tv";
    let title_field = if tv { "name" } else { "title" };
    let original_field = if tv { "original_name" } else { "original_title" };
    let date_field = if tv { "first_air_date" } else { "release_date" };
    let native = string(&value[original_field]);
    let title = string(&value[title_field]).or(native.clone()).ok_or_else(|| provider("TMDb", "detail.title/name"))?;
    let premiered = date(&value[date_field], "TMDb", "detail.premiere date")?;
    let status = match value["status"].as_str() {
        Some("Returning Series") => "airing",
        Some("Ended" | "Released") => "completed",
        Some("Canceled") => "cancelled",
        Some("Planned" | "In Production" | "Post Production") => "upcoming",
        _ => "unknown",
    };
    let mut media = CatalogMedia {
        title, original_title: native.clone(), kind: phase.into(), status: status.into(),
        summary: string(&value["overview"]), poster: tmdb_image(&value["poster_path"])? ,
        backdrop: tmdb_image(&value["backdrop_path"])?, year: year_of(premiered.as_deref()), premiered,
        ended: if tv && matches!(status, "completed" | "cancelled") { date(&value["last_air_date"], "TMDb", "detail.last_air_date")? } else { None },
        season_count: count(&value["number_of_seasons"], "TMDb", "detail.number_of_seasons")?,
        episode_count: count(&value["number_of_episodes"], "TMDb", "detail.number_of_episodes")?,
        studios: named_values(&value["production_companies"], "detail.production_companies")?,
        platforms: named_values(&value["networks"], "detail.networks")?,
        genres: named_values(&value["genres"], "detail.genres")?,
        source_url: Some(format!("https://www.themoviedb.org/{phase}/{native_id}")), ..Default::default()
    };
    unique(&mut media.aliases, native);
    if value["original_language"].as_str() == Some("en") { media.english_title = media.original_title.clone(); }
    if let Some(genres) = value["genres"].as_array() {
        if genres.iter().any(|g| g["id"].as_u64() == Some(16)) { media.subkind = Some("animation".into()); }
    }
    if let Some(countries) = optional_array(&value["origin_country"], "TMDb", "detail.origin_country")? {
        for value in countries {
            let code = string(value).ok_or_else(|| provider("TMDb", "detail.origin_country.code"))?;
            unique(&mut media.regions, Some(code));
        }
    }
    if let Some(countries) = optional_array(&value["production_countries"], "TMDb", "detail.production_countries")? {
        for value in countries {
            object(value, "TMDb", "detail.production_countries.item")?;
            unique(&mut media.regions, string(&value["iso_3166_1"]));
        }
    }
    media.runtime = if tv {
        if let Some(values) = optional_array(&value["episode_run_time"], "TMDb", "detail.episode_run_time")? {
            values.first().map(|v| count(v, "TMDb", "detail.episode_run_time.item")).transpose()?.flatten()
        } else { None }
    } else { count(&value["runtime"], "TMDb", "detail.runtime")? };
    if !value["translations"].is_null() {
        object(&value["translations"], "TMDb", "detail.translations")?;
        for translation in array(&value["translations"]["translations"], "TMDb", "detail.translations.translations")? {
            object(translation, "TMDb", "detail.translation")?;
            let translated = string(&translation["data"][title_field]);
            unique(&mut media.aliases, translated.clone());
            if translation["iso_639_1"].as_str() == Some("en") && media.english_title.is_none() {
                media.english_title = translated;
            }
        }
    }
    if !value["credits"].is_null() {
        object(&value["credits"], "TMDb", "detail.credits")?;
        media.cast = named_values(&value["credits"]["cast"], "detail.credits.cast")?;
        if let Some(crew) = optional_array(&value["credits"]["crew"], "TMDb", "detail.credits.crew")? {
            for person in crew {
                object(person, "TMDb", "detail.credits.crew.item")?;
                match person["job"].as_str() {
                    Some("Director") => unique(&mut media.directors, string(&person["name"])),
                    Some("Writer" | "Screenplay" | "Story" | "Teleplay" | "Novel" | "Original Story") => unique(&mut media.writers, string(&person["name"])),
                    _ => {},
                }
            }
        }
    }
    if !value["watch/providers"].is_null() {
        object(&value["watch/providers"], "TMDb", "detail.watch/providers")?;
        let providers = value["watch/providers"]["results"].as_object()
            .ok_or_else(|| provider("TMDb", "detail.watch/providers.results"))?;
        for (region, providers) in providers {
            if !ctx.countries.is_empty() && !ctx.countries.iter().any(|c| c == region) { continue; }
            object(providers, "TMDb", "detail.watch/providers.region")?;
            for category in ["flatrate", "free", "ads", "rent", "buy"] {
                if let Some(values) = optional_array(&providers[category], "TMDb", "detail.watch/providers.category")? {
                    for value in values { unique(&mut media.platforms, string(&value["provider_name"])); }
                }
            }
        }
    }
    let mut out = draft("tmdb", format!("{phase}:{native_id}"), media);
    // 同一数值可同时是电影与电视 ID，绝不写无类型的 TMDb 外部别名。
    external(&mut out, &format!("tmdb_{phase}"), native_id);
    if !value["external_ids"].is_null() { object(&value["external_ids"], "TMDb", "detail.external_ids")?; }
    if let Some(imdb) = string(&value["external_ids"]["imdb_id"]).or_else(|| string(&value["imdb_id"])) {
        if !imdb.starts_with("tt") || !imdb[2..].bytes().all(|b| b.is_ascii_digit()) || imdb.len() <= 2 {
            return Err(provider("TMDb", "detail.imdb_id"));
        }
        external(&mut out, "imdb", imdb);
    }
    let rating = score(&value["vote_average"], 10.0, "TMDb", "detail.vote_average")?;
    let votes = count(&value["vote_count"], "TMDb", "detail.vote_count")?;
    if rating.is_some() || votes.is_some() {
        out.ratings.push(CatalogRating {
            source: "tmdb".into(), category: "user".into(), value: rating, scale: 10.0,
            votes, url: out.media.source_url.clone(), ..Default::default()
        });
    }
    if tv {
        if let Some(seasons) = optional_array(&value["seasons"], "TMDb", "detail.seasons")? {
            for season in seasons {
                object(season, "TMDb", "detail.seasons.item")?;
                let id = id(&season["id"], "TMDb", "detail.seasons.id")?;
                out.seasons.push(CatalogSeason {
                    source: "tmdb".into(), external_id: format!("{}:season:{id}", out.external_id),
                    number: count(&season["season_number"], "TMDb", "detail.seasons.season_number")?,
                    title: string(&season["name"]), episode_count: count(&season["episode_count"], "TMDb", "detail.seasons.episode_count")?,
                    premiered: date(&season["air_date"], "TMDb", "detail.seasons.air_date")?, ended: None,
                });
            }
        }
        tmdb_episode(&mut out, &value["last_episode_to_air"], ctx)?;
        tmdb_episode(&mut out, &value["latest_episode_to_air"], ctx)?;
        tmdb_episode(&mut out, &value["next_episode_to_air"], ctx)?;
    } else if let Some(premiere) = out.media.premiered.clone() {
        let parsed = NaiveDate::parse_from_str(&premiere, "%Y-%m-%d").map_err(|_| provider("TMDb", "detail.release_date"))?;
        if parsed >= ctx.start && parsed < ctx.end {
            let release_id = format!("{}:premiere", out.external_id);
            out.releases.push(CatalogRelease {
                key: format!("tmdb:{release_id}"), media_key: out.media.key.clone(), source: "tmdb".into(),
                external_id: release_id, date: premiere, release_kind: "premiere".into(),
                url: out.media.source_url.clone(), ..Default::default()
            });
        }
    }
    Ok(out)
}

pub async fn tmdb(ctx: AdapterContext, tx: Sender<IngestionBatch>) -> Result<()> {
    range(&ctx)?;
    // 即使预算为零也先校验授权，避免把未配置状态伪装成成功。
    tmdb_request(&ctx, "https://api.themoviedb.org/3/discover/tv".into())?;
    let mut cursor = resume(&ctx, "tmdb", "tv", 1, true)?;
    if !matches!(cursor.phase.as_str(), "tv" | "movie") || cursor.offset == 0 {
        return Err(AppError::new(ErrorKind::Invalid, "TMDb 续传阶段或页码错误"));
    }
    if cursor.offset > TMDB_PAGE_LIMIT {
        return Err(AppError::new(ErrorKind::Provider, "TMDb 发现接口最多可读取 500 页；当前目录未遍历完，请缩小日期范围重新同步"));
    }
    if ctx.max_pages == 0 { return save(&tx, vec![], 0, Some(&cursor), TMDB_NOTE, true).await; }
    for page in 1..=ctx.max_pages {
        let url = tmdb_discover(&ctx, &cursor.phase, cursor.offset)?;
        let payload = get(&ctx, tmdb_request(&ctx, url)?).await?;
        object(&payload, "TMDb", "discover")?;
        if integer(&payload["page"], "TMDb", "discover.page")? != cursor.offset {
            return Err(provider("TMDb", "discover.page mismatch"));
        }
        let total_pages = integer(&payload["total_pages"], "TMDb", "discover.total_pages")?;
        let rows = array(&payload["results"], "TMDb", "discover.results")?;
        if rows.len() > METADATA_BATCH || (rows.is_empty() && cursor.offset < total_pages)
            || (!rows.is_empty() && cursor.offset > total_pages)
        {
            return Err(provider("TMDb", "discover.results/total_pages"));
        }
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            object(row, "TMDb", "discover.results.item")?;
            let native_id = id(&row["id"], "TMDb", "discover.results.id")?;
            let detail = get(&ctx, tmdb_request(&ctx, tmdb_detail(&cursor.phase, &native_id))?).await?;
            if id(&detail["id"], "TMDb", "detail.id")? != native_id {
                return Err(provider("TMDb", "detail.id mismatch"));
            }
            items.push(tmdb_item(&detail, &cursor.phase, &ctx)?);
        }
        let mut next = cursor.clone();
        let more = if cursor.offset < total_pages {
            next.offset = cursor.offset.checked_add(1).ok_or_else(|| provider("TMDb", "discover.page overflow"))?;
            true
        } else if cursor.phase == "tv" {
            next.phase = "movie".into(); next.offset = 1;
            true
        } else { false };
        let capped = more && next.phase == cursor.phase && next.offset > TMDB_PAGE_LIMIT;
        let note = if total_pages > TMDB_PAGE_LIMIT {
            format!("{TMDB_NOTE} 此范围超过 TMDb 发现接口 500 页上限，无法遍历全部目录；请缩小日期范围重新同步。")
        } else { TMDB_NOTE.into() };
        save(&tx, items, page, more.then_some(&next), &note, page == ctx.max_pages).await?;
        if !more || capped { return Ok(()); }
        cursor = next;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::net::MockHttp;
    use serde_json::json;
    use std::sync::Arc;

    fn context(http: Arc<MockHttp>, max_pages: u32) -> AdapterContext {
        AdapterContext {
            http, start: NaiveDate::from_ymd_opt(2026, 11, 23).unwrap(),
            end: NaiveDate::from_ymd_opt(2026, 11, 30).unwrap(), mode: "incremental".into(),
            cursor: None, max_pages, countries: vec![], token: None,
        }
    }

    fn simkl_row(episode: u32, date: &str) -> Value {
        json!({"title":"Same show", "ids":{"simkl_id":9,"mal":42,"tmdb":81},
            "date":date, "release_date":"2026-10-01", "anime_type":"tv", "poster":"unverified-hash",
            "episode":{"episode":episode}, "ratings":{"simkl":{"rating":8.1,"votes":24}}, "rank":5})
    }

    #[test]
    fn simkl_preserves_source_local_midnight_and_offset() {
        let ctx = context(Arc::new(MockHttp::new()), 1);
        let item = simkl_item(&simkl_row(1, "2026-11-23T00:00:00+09:00"), &ctx, "anime").unwrap().unwrap();
        let release = &item.releases[0];
        assert_eq!(release.date, "2026-11-23");
        assert_eq!(release.local_time.as_deref(), Some("00:00:00"));
        assert_eq!(release.timezone.as_deref(), Some("+09:00"));
        assert_eq!(release.utc_time.as_deref(), Some("2026-11-22T15:00:00Z"));
        assert!(item.media.regions.is_empty());
        assert!(item.media.poster.is_none());
        assert_eq!((item.ratings[0].scale, item.ratings[0].votes), (10.0, Some(24)));
        assert!(item.external_ids.iter().any(|id| id.source == "mal" && id.external_id == "42"));
        assert!(simkl_item(&simkl_row(2, "2026-11-30T00:00:00+09:00"), &ctx, "anime").unwrap().is_none());
    }

    fn simkl_empty_calendars(http: &Arc<MockHttp>) {
        http.on(SIMKL_MOVIE_CALENDAR, 200, "[]");
        http.on(SIMKL_TV_CALENDAR, 200, "[]");
    }

    #[tokio::test]
    async fn simkl_keeps_distinct_episode_rows_and_resumes_budget() {
        let http = Arc::new(MockHttp::new());
        let mut rows = vec![simkl_row(1, "2026-11-23T00:00:00+09:00"), simkl_row(2, "2026-11-24T00:00:00+09:00")];
        for episode in 3..=101 { rows.push(simkl_row(episode, "2026-11-25T00:00:00+09:00")); }
        http.on(SIMKL_CALENDAR, 200, &Value::Array(rows).to_string());
        simkl_empty_calendars(&http);
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        simkl(context(http.clone(), 1), tx).await.unwrap();
        let first = rx.recv().await.unwrap();
        assert!(!first.complete);
        assert_eq!(first.items.len(), SNAPSHOT_BATCH);
        assert_eq!(first.items[0].external_id, first.items[1].external_id);
        assert_ne!(first.items[0].releases[0].key, first.items[1].releases[0].key);
        let mut ctx = context(http, 5); ctx.cursor = first.cursor;
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        simkl(ctx, tx).await.unwrap();
        let mut last = rx.recv().await.unwrap();
        assert_eq!(last.items.len(), 1);
        while let Some(batch) = rx.recv().await { last = batch; }
        assert!(last.complete && last.cursor.is_none());
    }

    #[tokio::test]
    async fn simkl_movie_and_tv_phases_map_ids_and_release_kinds() {
        let http = Arc::new(MockHttp::new());
        http.on(SIMKL_CALENDAR, 200, "[]");
        let movie = json!({"title":"A Film","ids":{"simkl_id":70,"tmdb":1810479,"imdb":"tt50224459"},
            "date":"2026-11-25T00:00:00-05:00","release_date":"2026-11-25",
            "ratings":{"simkl":{"rating":null,"votes":null}}});
        let tv = json!({"title":"A Show","ids":{"simkl_id":80,"tmdb":136804,"imdb":null},
            "date":"2026-11-25T00:00:00-05:00","release_date":"2018-08-02",
            "episode":{"season":2026,"episode":52},"ratings":{"simkl":{"rating":10,"votes":1}}});
        http.on(SIMKL_MOVIE_CALENDAR, 200, &Value::Array(vec![movie]).to_string());
        http.on(SIMKL_TV_CALENDAR, 200, &Value::Array(vec![tv]).to_string());
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        simkl(context(http, 8), tx).await.unwrap();
        let mut items = Vec::new();
        let mut complete = false;
        while let Some(batch) = rx.recv().await { items.extend(batch.items); complete = batch.complete; }
        assert!(complete);
        let film = items.iter().find(|i| i.media.kind == "movie").unwrap();
        assert_eq!(film.releases[0].release_kind, "premiere");
        assert!(film.external_ids.iter().any(|id| id.source == "imdb" && id.external_id == "tt50224459"));
        assert!(film.external_ids.iter().any(|id| id.source == "tmdb_movie"));
        assert!(film.ratings.is_empty(), "null 评分不入库");
        let show = items.iter().find(|i| i.media.kind == "tv").unwrap();
        assert_eq!(show.releases[0].release_kind, "episode");
        assert_eq!(show.releases[0].season, Some(2026));
        assert_eq!(show.releases[0].episode.as_deref(), Some("52"));
        assert!(show.external_ids.iter().any(|id| id.source == "tmdb_tv"));
    }

    #[tokio::test]
    async fn simkl_schedule_correction_updates_one_row_not_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::app::db::Db::open_at(&dir.path().join("catalog.db")).unwrap();
        let http = Arc::new(MockHttp::new());
        http.on(SIMKL_CALENDAR, 200, &Value::Array(vec![simkl_row(8, "2026-11-23T00:00:00+09:00")]).to_string());
        simkl_empty_calendars(&http);
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        simkl(context(http.clone(), 4), tx).await.unwrap();
        let batch = rx.recv().await.unwrap();
        crate::app::catalog::store::upsert(&db, &batch.items, "2026-11-23T10:00:00+08:00").unwrap();
        let http2 = Arc::new(MockHttp::new());
        http2.on(SIMKL_CALENDAR, 200, &Value::Array(vec![simkl_row(8, "2026-11-24T00:00:00+09:00")]).to_string());
        simkl_empty_calendars(&http2);
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        simkl(context(http2, 4), tx).await.unwrap();
        let batch = rx.recv().await.unwrap();
        crate::app::catalog::store::upsert(&db, &batch.items, "2026-11-24T10:00:00+08:00").unwrap();
        let page = crate::app::catalog::store::releases(&db, &crate::app::catalog::types::CatalogQuery {
            start: Some("2026-11-20".into()), end: Some("2026-12-01".into()), ..Default::default()
        }).unwrap();
        assert_eq!(page.total, 1, "改期必须更新原行，不能留下旧日期的重复行");
        assert_eq!(page.items[0].release.date, "2026-11-24");
    }

    #[test]
    fn bangumi_infobox_uses_key_and_skips_entries_without_name() {
        let value = json!({"id":7,"type":2,"name":"Native","name_cn":"中文","date":"2026-10-01",
            "infobox":[
                {"key":"国家/地区","value":[{"v":"日本"}]},
                {"key":"动画制作","value":"Studio"},
                {"value":"没有 name 也没有 key 的条目"},
                {"key":"别名","value":["别名A"]}
            ]});
        let item = bangumi_item(&value).unwrap();
        assert_eq!(item.media.regions, vec!["JP"]);
        assert_eq!(item.media.studios, vec!["Studio"]);
        assert!(item.media.aliases.contains(&"别名A".into()));
    }

    #[tokio::test]
    async fn bangumi_browse_keeps_chinese_metadata_and_never_fabricates_dates() {
        let http = Arc::new(MockHttp::new());
        http.on(&format!("{BANGUMI_BROWSE}?type=2&limit=50&offset=0"), 200, &json!({
            "data": [{
                "id":7,"type":2,"name":"Native","name_cn":"中文","date":"2026-10-01",
                "summary":"中文简介","platform":"TV","eps":12,
                "meta_tags":["科幻","日本","TV"],
                "infobox":[
                    {"key":"放送结束","value":"2026-12-20"},
                    {"key":"类型","value":["科幻","冒险"]}
                ],
                "rating":{"score":7.2,"total":90,"rank":100}
            }],
            "total": 1
        }).to_string());
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        bangumi(context(http, 1), tx).await.unwrap();
        let batch = rx.recv().await.unwrap();
        let item = &batch.items[0];
        assert!(batch.complete);
        assert!(item.releases.is_empty(), "星期不是单集日期，不生成日历");
        assert_eq!(item.media.title, "中文");
        assert_eq!(item.media.summary.as_deref(), Some("中文简介"));
        assert_eq!(item.media.genres, vec!["科幻", "冒险"]);
        assert_eq!(item.media.regions, vec!["JP"]);
        assert_eq!(item.media.status, "completed", "有放送结束就是已完结");
        assert_eq!(item.media.episode_count, Some(12));
        assert_eq!((item.ratings[0].category.as_str(), item.ratings[0].votes), ("audience", Some(90)));
        assert!(item.media.aliases.contains(&"Native".into()) && item.media.aliases.contains(&"中文".into()));
    }

    fn kitsu_row(id: u32, phase: &str) -> Value {
        json!({"id":id.to_string(),"type":phase,"attributes":{"canonicalTitle":"Current work",
            "titles":{"en":"English","ja_jp":"原名"},"status":"current","startDate":"2026-10-01",
            "averageRating":"81.2","ratingFrequencies":{"2":"3","20":"4"},"updatedAt":"2026-11-23T12:00:00Z"}})
    }

    #[tokio::test]
    async fn kitsu_follows_two_actual_pages_and_resumes_into_manga() {
        let http = Arc::new(MockHttp::new());
        let next = "https://kitsu.io/api/edge/anime?filter%5Bstatus%5D=current&page%5Blimit%5D=20&page%5Boffset%5D=20";
        http.on(&kitsu_first("anime"), 200, &json!({"data":[kitsu_row(1,"anime")],"links":{"next":next}}).to_string());
        http.on(next, 200, &json!({"data":[kitsu_row(2,"anime")],"links":{"next":null}}).to_string());
        http.on(&kitsu_first("manga"), 200, &json!({"data":[kitsu_row(1,"manga")],"links":{"next":null}}).to_string());
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        kitsu(context(http.clone(), 1), tx).await.unwrap();
        let first = rx.recv().await.unwrap();
        assert!(!first.complete);
        assert!(first.items[0].releases.is_empty() && first.items[0].media.regions.is_empty());
        assert_eq!((first.items[0].ratings[0].scale, first.items[0].ratings[0].votes), (100.0, Some(7)));
        let mut ctx = context(http.clone(), 2); ctx.cursor = first.cursor;
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        kitsu(ctx, tx).await.unwrap();
        let second = rx.recv().await.unwrap();
        let last = rx.recv().await.unwrap();
        assert!(!second.complete && last.complete && last.cursor.is_none());
        assert_ne!(second.items[0].external_id, last.items[0].external_id);
        assert_eq!(http.call_count(next), 1);
        assert_eq!(http.call_count(&kitsu_first("anime")), 1);
    }

    #[test]
    fn kitsu_rejects_untrusted_nonadvancing_or_unbounded_links() {
        for link in [
            "https://evil.example/api/edge/anime?filter[status]=current&page[limit]=20&page[offset]=20",
            "https://kitsu.io@evil.example/api/edge/anime?filter[status]=current&page[limit]=20&page[offset]=20",
            "http://kitsu.io/api/edge/anime?filter[status]=current&page[limit]=20&page[offset]=20",
            "https://kitsu.io/api/edge/users?filter[status]=current&page[limit]=20&page[offset]=20",
            "https://kitsu.io/api/edge/manga?filter[status]=current&page[limit]=20&page[offset]=20",
            "https://kitsu.io/api/edge/anime?filter[status]=current&page[limit]=100&page[offset]=20",
        ] { assert_eq!(kitsu_link(link, "anime").unwrap_err().kind, ErrorKind::Provider); }
    }

    #[tokio::test]
    async fn malformed_source_payloads_are_provider_errors_not_empty_success() {
        for source in ["bangumi", "simkl", "kitsu", "tmdb"] {
            let http = Arc::new(MockHttp::new());
            let mut ctx = context(http.clone(), 1);
            let url = match source {
                "bangumi" => format!("{BANGUMI_BROWSE}?type=2&limit=50&offset=0"),
                "simkl" => SIMKL_CALENDAR.into(),
                "kitsu" => kitsu_first("anime"), _ => tmdb_discover(&ctx, "tv", 1).unwrap(),
            };
            http.on(&url, 200, "{}");
            ctx.token = Some("test-only-read-token".into());
            let (tx, mut rx) = tokio::sync::mpsc::channel(4);
            let err = match source {
                "bangumi" => bangumi(ctx, tx).await, "simkl" => simkl(ctx, tx).await,
                "kitsu" => kitsu(ctx, tx).await, _ => tmdb(ctx, tx).await,
            }.unwrap_err();
            assert_eq!(err.kind, ErrorKind::Provider);
            assert!(rx.try_recv().is_err());
            assert!(!err.message.contains("test-only-read-token"));
        }
    }

    fn tmdb_row(id: u32, tv: bool) -> Value {
        json!({"id":id,"name":"Series","title":"Movie","original_name":"Original series","original_title":"Original movie",
            "first_air_date":"2026-11-23","release_date":"2026-11-23","vote_average":7.5,"vote_count":32,
            "origin_country":["US"],"genres":[],"status":if tv {"Returning Series"} else {"Released"},
            "last_episode_to_air":if tv {json!({"id":90,"air_date":"2026-11-24","season_number":1,"episode_number":2})} else {Value::Null}})
    }

    #[tokio::test]
    async fn tmdb_traverses_real_pages_and_uses_only_bearer_authorization() {
        let http = Arc::new(MockHttp::new());
        let mut ctx = context(http.clone(), 1); ctx.token = Some("test-only-read-token".into());
        let tv1 = tmdb_discover(&ctx, "tv", 1).unwrap();
        let tv2 = tmdb_discover(&ctx, "tv", 2).unwrap();
        let movie1 = tmdb_discover(&ctx, "movie", 1).unwrap();
        assert!(tv1.contains("first_air_date.lte=2026-11-29"));
        http.on(&tv1, 200, &json!({"page":1,"total_pages":2,"results":[{"id":1}]}).to_string());
        http.on(&tv2, 200, &json!({"page":2,"total_pages":2,"results":[{"id":2}]}).to_string());
        http.on(&movie1, 200, &json!({"page":1,"total_pages":1,"results":[{"id":1}]}).to_string());
        for id in [1, 2] { http.on(&tmdb_detail("tv", &id.to_string()), 200, &tmdb_row(id, true).to_string()); }
        http.on(&tmdb_detail("movie", "1"), 200, &tmdb_row(1, false).to_string());
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        tmdb(ctx.clone(), tx).await.unwrap();
        let first = rx.recv().await.unwrap();
        assert!(!first.complete);
        let release = &first.items[0].releases[0];
        assert_eq!(release.date, "2026-11-24");
        assert!(release.local_time.is_none() && release.timezone.is_none() && release.utc_time.is_none());
        ctx.cursor = first.cursor.clone(); ctx.max_pages = 2;
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        tmdb(ctx, tx).await.unwrap();
        assert!(!rx.recv().await.unwrap().complete);
        let last = rx.recv().await.unwrap();
        assert!(last.complete && last.cursor.is_none());
        assert_ne!(first.items[0].external_id, last.items[0].external_id);
        assert_eq!(first.items[0].external_ids[0].source, "tmdb_tv");
        assert_eq!(last.items[0].external_ids[0].source, "tmdb_movie");
        assert_eq!(last.items[0].releases[0].release_kind, "premiere");
        for url in http.calls() {
            assert!(!url.contains("test-only-read-token") && !url.contains("api_key"));
            assert_eq!(http.last_header(&url, "Authorization").as_deref(), Some("Bearer test-only-read-token"));
        }
        assert!(!first.cursor.unwrap().contains("test-only-read-token"));
    }

    #[tokio::test]
    async fn tmdb_api_cap_never_means_complete() {
        let http = Arc::new(MockHttp::new());
        let mut ctx = context(http.clone(), 2); ctx.token = Some("test-only-read-token".into());
        let mut cursor = resume(&ctx, "tmdb", "tv", 1, true).unwrap(); cursor.offset = 500;
        ctx.cursor = Some(serde_json::to_string(&cursor).unwrap());
        http.on(&tmdb_discover(&ctx, "tv", 500).unwrap(), 200,
            &json!({"page":500,"total_pages":501,"results":[{"id":1}]}).to_string());
        http.on(&tmdb_detail("tv", "1"), 200, &tmdb_row(1, true).to_string());
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        tmdb(ctx.clone(), tx).await.unwrap();
        let batch = rx.recv().await.unwrap();
        assert!(!batch.complete);
        assert!(batch.warning.unwrap().contains("500"));
        ctx.cursor = batch.cursor;
        let (tx, _) = tokio::sync::mpsc::channel(4);
        assert_eq!(tmdb(ctx, tx).await.unwrap_err().kind, ErrorKind::Provider);
        assert!(!http.calls().iter().any(|url| url.contains("page=501")));
    }

    #[tokio::test]
    async fn tmdb_missing_token_is_invalid_without_http() {
        let http = Arc::new(MockHttp::new());
        let (tx, _) = tokio::sync::mpsc::channel(4);
        assert_eq!(tmdb(context(http.clone(), 1), tx).await.unwrap_err().kind, ErrorKind::Invalid);
        assert!(http.calls().is_empty());
    }
}
