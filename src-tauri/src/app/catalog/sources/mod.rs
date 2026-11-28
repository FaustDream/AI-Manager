mod bilibili;
mod iqiyi;
mod mgtv;
mod netflix;
mod tencent;
mod youku;
mod global;
mod supplemental;

use crate::app::catalog::types::{IngestionBatch, MediaDraft};
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::{Http, HttpRequest, HttpResponse};
use chrono::NaiveDate;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

pub struct SourceSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// 国内 / 国外页签归属；空串 = 评分回填源，不参与页签划分
    pub scope: &'static str,
    pub implemented: bool,
    pub requires_key: bool,
    pub default_enabled: bool,
    pub coverage: &'static str,
    pub license_note: &'static str,
}

/// 国内页签 = 这五个国内平台采集的条目；同作品若同时被国际源采集，两边页签都出现。
pub(crate) use crate::app::catalog::store::valid_date;

pub const DOMESTIC_SOURCES: &[&str] = &["bilibili", "tencent", "iqiyi", "youku", "mgtv"];
/// 国外页签 = 国际源采集的条目。
pub const FOREIGN_SOURCES: &[&str] = &["tvmaze", "anilist", "mangadex", "bangumi", "simkl", "kitsu", "tmdb"];

/// 页签 → 该页签包含的采集源集合。
pub fn scope_sources(scope: &str) -> &'static [&'static str] {
    match scope {
        "domestic" => DOMESTIC_SOURCES,
        "foreign" => FOREIGN_SOURCES,
        _ => &[],
    }
}

pub const SOURCES: &[SourceSpec] = &[
    SourceSpec { id: "tvmaze", label: "TVmaze", scope: "foreign", implemented: true, requires_key: false, default_enabled: true, coverage: "指定国家电视台每日节目 + 全球网络平台日历；非全部平台全集。保留节目当地日期与时区。", license_note: "公开 API，CC BY-SA；使用须署名 TVmaze，按其条款分享。" },
    SourceSpec { id: "anilist", label: "AniList", scope: "foreign", implemented: true, requires_key: false, default_enabled: true, coverage: "动漫实际播出时间（完整分页）及连载漫画元数据；漫画无章节发布日期时不进入日历。", license_note: "公开 GraphQL，遵守 AniList API 条款；禁止大规模备份/商业竞争用途。" },
    SourceSpec { id: "mangadex", label: "MangaDex", scope: "foreign", implemented: true, requires_key: false, default_enabled: true, coverage: "公开漫画目录及站内章节发布时间；属于译本/发行发布时间，不代表原作官方更新。默认排除成人分类。", license_note: "只读取公开元数据，不获取图片页、扫描内容或播放资源；遵守 API 限流与分页上限。" },
    SourceSpec { id: "bangumi", label: "Bangumi", scope: "foreign", implemented: true, requires_key: false, default_enabled: true, coverage: "当前动漫周历与公开作品资料；固定星期不能确认单集日期，不据此虚构更新记录。", license_note: "公开 API，仅元数据；署名 Bangumi，遵守服务条款。" },
    SourceSpec { id: "simkl", label: "SIMKL", scope: "foreign", implemented: true, requires_key: false, default_enabled: true, coverage: "公开动漫/电影/电视日历快照（含 MAL/TMDb/IMDb ID）；电影记为首映、电视带季集号，保留原始时区偏移。需授权的 API 端点不采集。", license_note: "公开日历资料，署名 SIMKL；不绕过需要 API key 的端点。" },
    SourceSpec { id: "kitsu", label: "Kitsu", scope: "foreign", implemented: true, requires_key: false, default_enabled: false, coverage: "当前动漫/漫画元数据，按 next 完整分页；无单集日期时不进入日历。", license_note: "公开 JSON:API；只读取作品资料，遵守 Kitsu 服务条款。" },
    SourceSpec { id: "tmdb", label: "TMDb", scope: "foreign", implemented: true, requires_key: true, default_enabled: false, coverage: "授权后按发现接口分页采集指定首播日期范围的影视元数据；发现接口不保证所有单集播出日。", license_note: "需要个人 API Read Access Token；署名 TMDb，非商业用途须遵守条款，商业另行授权。" },
    SourceSpec { id: "bilibili", label: "哔哩哔哩", scope: "domestic", implemented: true, requires_key: false, default_enabled: true, coverage: "公开番剧/国创/综艺/影视时间线前后7日；没有平台全量索引，保留每一集。", license_note: "现有公开接口，只读节目元数据；不获取播放线路。" },
    SourceSpec { id: "tencent", label: "腾讯视频", scope: "domestic", implemented: true, requires_key: false, default_enabled: true, coverage: "公开移动首页 + 频道列表分页；中文剧名、封面、演员与评分；无逐集日期，不生成更新日历。", license_note: "现有公开页面，只读节目资料；页面改版可能无法采集。" },
    SourceSpec { id: "iqiyi", label: "爱奇艺", scope: "domestic", implemented: true, requires_key: false, default_enabled: true, coverage: "推荐页 + 频道关键词分页搜索；中文简介、集数、地区/题材、封面与评分；仍非全量目录。", license_note: "现有公开页面，只读节目资料；不获取播放线路。" },
    SourceSpec { id: "youku", label: "优酷", scope: "domestic", implemented: true, requires_key: false, default_enabled: true, coverage: "公开电视剧频道卡片；中文剧名与集数状态；页面不提供年份/简介/封面，不生成更新日历。", license_note: "现有公开页面，只读资料；不获取播放线路。" },
    SourceSpec { id: "mgtv", label: "芒果TV", scope: "domestic", implemented: true, requires_key: false, default_enabled: true, coverage: "公开频道列表（综艺/电视剧/动漫），带中文简介、类型、年份、集数与知乎评分；无确认单集日期，不进入更新日历。", license_note: "现有公开列表接口，只读节目元数据；不获取播放线路。" },
    SourceSpec { id: "omdb", label: "OMDb / IMDb / Rotten Tomatoes", scope: "", implemented: true, requires_key: true, default_enabled: false, coverage: "授权后按库内IMDb ID逐项获取原始评分；覆盖取决于OMDb授权，不推测观众好评率或评分人数。",  license_note: "需要 OMDb API key，评分字段取决于账户与授权；不抓取评价网站。" },
    SourceSpec { id: "tvdb", label: "TheTVDB", scope: "foreign", implemented: false, requires_key: true, default_enabled: false, coverage: "未接入：需要 API v4 授权/订阅。", license_note: "需要遵守 TheTVDB 许可，不能借用他人 key。" },
    SourceSpec { id: "trakt", label: "Trakt", scope: "foreign", implemented: false, requires_key: true, default_enabled: false, coverage: "未接入：需要注册应用 client ID；当前无授权。", license_note: "遵守 Trakt API 授权及速率限制。" },
    SourceSpec { id: "jikan", label: "Jikan / MyAnimeList", scope: "foreign", implemented: false, requires_key: false, default_enabled: false, coverage: "未接入：开发机实测 api.jikan.moe 持续超时（网络不可达）。MAL 评分改走官方 API（见 MyAnimeList 源），MAL ID 已由 AniList/SIMKL 关联。", license_note: "Jikan 非官方只读 API；不冒用 MyAnimeList 授权。" },
    SourceSpec { id: "mal", label: "MyAnimeList 官方", scope: "", implemented: true, requires_key: true, default_enabled: false, coverage: "授权后按库内 MAL ID 逐项取动漫/漫画 mean 评分、排名与评分人数；覆盖取决于已关联 MAL ID 的作品。", license_note: "需要 MAL 官方 API 客户端 ID（免费申请）；遵守 MyAnimeList API 条款，不抓取受限页面。" },
    SourceSpec { id: "douban", label: "豆瓣", scope: "domestic", implemented: false, requires_key: true, default_enabled: false, coverage: "未接入：没有可验证的公开批量评分接口或授权，不爬取受限页面。", license_note: "须获得合法数据访问授权；不虚构评分/好评率。" },
    SourceSpec { id: "netflix", label: "奈飞", scope: "foreign", implemented: false, requires_key: false, default_enabled: false, coverage: netflix::NOTE, license_note: "Netflix 不提供公开更新列表接口；其原创剧集由 TVmaze 网络日历覆盖。" },
];

pub fn spec(id: &str) -> Result<&'static SourceSpec> {
    SOURCES.iter().find(|s| s.id == id).ok_or_else(|| AppError::new(ErrorKind::Invalid, "未知资料库数据源"))
}

#[derive(Clone)]
pub struct AdapterContext {
    pub http: Arc<dyn Http>,
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub mode: String,
    pub cursor: Option<String>,
    pub max_pages: u32,
    pub countries: Vec<String>,
    pub token: Option<String>,
}

pub async fn fetch(source: &str, ctx: AdapterContext, tx: Sender<IngestionBatch>) -> Result<()> {
    match source {
        "tvmaze" => global::tvmaze(ctx, tx).await,
        "anilist" => global::anilist(ctx, tx).await,
        "mangadex" => global::mangadex(ctx, tx).await,
        "bangumi" => supplemental::bangumi(ctx, tx).await,
        "simkl" => supplemental::simkl(ctx, tx).await,
        "kitsu" => supplemental::kitsu(ctx, tx).await,
        "tmdb" => supplemental::tmdb(ctx, tx).await,
        "bilibili" => bilibili::fetch(ctx, tx).await,
        "iqiyi" => iqiyi::fetch(ctx, tx).await,
        "mgtv" => mgtv::fetch(ctx, tx).await,
        "tencent" => {
            let drafts = tencent::fetch(ctx).await?;
            let items = drafts.into_iter().map(legacy_draft).collect();
            publish(&tx, IngestionBatch { items, page: 1, complete: true, http_status: Some(200), ..Default::default() }).await
        }
        "youku" => {
            let drafts = youku::fetch(ctx.http).await?;
            let items = drafts.into_iter().map(legacy_draft).collect();
            publish(&tx, IngestionBatch { items, page: 1, complete: true, http_status: Some(200), ..Default::default() }).await
        }
        _ => Err(AppError::new(ErrorKind::Invalid, "该数据源尚未接入")),
    }
}

pub async fn publish(tx: &Sender<IngestionBatch>, batch: IngestionBatch) -> Result<()> {
    tx.send(batch).await.map_err(|_| AppError::new(ErrorKind::Io, "采集任务已停止"))
}

pub async fn json(http: &Arc<dyn Http>, req: HttpRequest) -> Result<Value> {
    let config = crate::app::config::get().catalog;
    let retries = config.retry_count.min(5);
    for attempt in 0..=retries {
        let result = http.send(req.clone()).await;
        let mut rate_limited = false;
        match result {
            Ok(resp) if resp.is_ok() => return resp.json(),
            Ok(resp) if (resp.status == 429 || resp.status >= 500) && attempt < retries => {
                rate_limited = resp.status == 429;
            }
            Ok(resp) => return Err(AppError::new(ErrorKind::Provider, format!("数据源返回 HTTP {}", resp.status))),
            Err(_) if attempt < retries => {},
            Err(err) => return Err(err),
        }
        // 429 是分钟级配额窗口，短间隔重试只会继续撞墙；用更长的退避等待配额恢复。
        let delay = if rate_limited {
            (20_000u64.saturating_mul(u64::from(attempt + 1))).min(60_000)
        } else {
            config.retry_delay_ms.saturating_mul(u64::from(attempt + 1)).min(60_000)
        };
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    }
    unreachable!()
}

pub async fn pace() {
    tokio::time::sleep(std::time::Duration::from_millis(crate::app::config::get().catalog.request_gap_ms)).await;
}

pub fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => non_empty(Some(s)),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

pub fn strip_html(value: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for ch in value.chars() {
        match ch {
            '<' => tag = true,
            '>' => { tag = false; out.push(' '); },
            _ if !tag => out.push(ch),
            _ => {},
        }
    }
    out.replace("&amp;", "&").replace("&quot;", "\"").replace("&#39;", "'").replace("&nbsp;", " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Draft {
    pub platform: &'static str,
    pub native_id: String,
    pub kind: &'static str,
    pub title: String,
    pub update_note: Option<String>,
    pub year: Option<i32>,
    pub air_date: Option<String>,
    pub finished: bool,
    pub url: Option<String>,
    pub episode_id: Option<String>,
    pub local_time: Option<String>,
    pub poster: Option<String>,
    pub summary: Option<String>,
    pub cast: Vec<String>,
    pub genres: Vec<String>,
    pub regions: Vec<String>,
    pub rating: Option<f64>,
}

fn legacy_draft(d: Draft) -> MediaDraft {
    use crate::app::catalog::types::{CatalogMedia, CatalogRating, CatalogRelease};
    let note = d.update_note.as_deref().unwrap_or("");
    let (episode_count, aired_episodes) = episode_numbers(note);
    let status = if d.finished {
        "completed"
    } else if note.contains("更新至") || aired_episodes.is_some() {
        "airing"
    } else {
        "unknown"
    };
    let mut out = MediaDraft {
        source: d.platform.into(), external_id: d.native_id,
        media: CatalogMedia {
            title: d.title.clone(), kind: d.kind.into(), regions: d.regions,
            platforms: vec![spec(d.platform).map(|s| s.label.to_string()).unwrap_or_default()],
            summary: d.summary,
            status: status.into(),
            year: d.year, premiered: if d.platform != "bilibili" { d.air_date.clone() } else { None },
            episode_count, aired_episodes,
            cast: d.cast, genres: d.genres,
            poster: d.poster, source_url: d.url.clone(), ..Default::default()
        },
        ratings: d.rating.map(|value| vec![CatalogRating {
            source: d.platform.into(),
            category: "audience".into(),
            value: Some(value),
            scale: 10.0,
            url: d.url.clone(),
            ..Default::default()
        }]).unwrap_or_default(),
        ..Default::default()
    };
    if d.platform == "bilibili" {
        if let (Some(date), Some(id)) = (d.air_date, d.episode_id) {
            out.releases.push(CatalogRelease {
                source: d.platform.into(), external_id: id, date, local_time: d.local_time,
                timezone: Some("Asia/Shanghai".into()), episode: d.update_note,
                platform: Some("哔哩哔哩".into()), release_kind: "episode".into(), url: d.url,
                ..Default::default()
            });
        }
        // B站同时分发海外作品，不能从发行平台推断制作国家。
        out.media.regions.clear();
    }
    out
}

pub const DESKTOP_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
pub const MOBILE_UA: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";

pub fn check(resp: HttpResponse, label: &str) -> Result<HttpResponse> {
    if resp.is_ok() { return Ok(resp); }
    Err(AppError::new(ErrorKind::Provider, format!("{label}返回 HTTP {}", resp.status)))
}

pub fn normalize_date(text: &str, fallback_year: i32) -> Option<String> {
    let parts: Vec<&str> = text.trim().split('-').collect();
    let (year, month, day) = match parts.as_slice() {
        [y, m, d] => (y.parse().ok()?, m.parse().ok()?, d.parse().ok()?),
        [m, d] => (fallback_year, m.parse().ok()?, d.parse().ok()?),
        _ => return None,
    };
    Some(NaiveDate::from_ymd_opt(year, month, day)?.format("%Y-%m-%d").to_string())
}
pub fn year_of(date: Option<&str>) -> Option<i32> { date.and_then(|d| d.get(..4)).and_then(|y| y.parse().ok()) }
pub fn is_finished(note: &str) -> bool { let n = note.trim(); !n.is_empty() && (n.starts_with('全') || n.contains("集全")) }
/// 从「全40集 / 24集全 / 更新至第17集」里取（总集数，已发行集数）。
/// 只解析平台原样的集数说明，不把年份、期数换算成集数。
pub fn episode_numbers(note: &str) -> (Option<u32>, Option<u32>) {
    fn number_after(text: &str, marker: &str) -> Option<u32> {
        let (_, rest) = text.split_once(marker)?;
        let rest = rest.trim_start().trim_start_matches('第');
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse().ok()
    }
    fn number_before(text: &str, marker: &str) -> Option<u32> {
        let (head, _) = text.split_once(marker)?;
        let digits: String = head
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        digits.parse().ok()
    }
    if let Some(total) = number_after(note, "全").or_else(|| number_before(note, "集全")) {
        return (Some(total), Some(total));
    }
    if let Some(aired) = number_after(note, "更新至") {
        return (None, Some(aired));
    }
    (None, None)
}
/// 国内源的中文地区/产地标签映射到 ISO 码；未识别返回 None，不猜。
pub fn cn_region(value: &str) -> Option<&'static str> {
    match value.trim() {
        "内地" | "中国大陆" | "中国" | "大陆" => Some("CN"),
        "香港" | "中国香港" => Some("HK"),
        "台湾" | "中国台湾" => Some("TW"),
        "日本" | "日本国" => Some("JP"),
        "韩国" | "南韩" => Some("KR"),
        "美国" => Some("US"),
        "英国" => Some("GB"),
        "泰国" => Some("TH"),
        "新加坡" => Some("SG"),
        _ => None,
    }
}

pub fn non_empty(value: Option<&str>) -> Option<String> { value.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn episode_notes_are_parsed_without_guessing() {
        assert_eq!(episode_numbers("全40集"), (Some(40), Some(40)));
        assert_eq!(episode_numbers("24集全"), (Some(24), Some(24)));
        assert_eq!(episode_numbers("更新至第17集"), (None, Some(17)));
        assert_eq!(episode_numbers("更新至168集"), (None, Some(168)));
        assert_eq!(episode_numbers("2026-11-20期"), (None, None));
        assert_eq!(episode_numbers("第15话~第16话"), (None, None));
    }

    #[test]
    fn legacy_drafts_mark_airing_and_completed_from_the_note() {
        let airing = legacy_draft(Draft {
            platform: "tencent",
            native_id: "c1".into(),
            kind: "tv",
            title: "更新中的剧".into(),
            update_note: Some("更新至12集".into()),
            year: None,
            air_date: None,
            finished: false,
            url: None,
            episode_id: None,
            local_time: None,
            poster: None,
            ..Default::default()
        });
        assert_eq!(airing.media.status, "airing");
        assert_eq!(airing.media.aired_episodes, Some(12));

        let done = legacy_draft(Draft {
            platform: "youku",
            native_id: "c2".into(),
            kind: "tv",
            title: "完结的剧".into(),
            update_note: Some("全40集".into()),
            year: None,
            air_date: None,
            finished: true,
            url: None,
            episode_id: None,
            local_time: None,
            poster: None,
            ..Default::default()
        });
        assert_eq!(done.media.status, "completed");
        assert_eq!(done.media.episode_count, Some(40));
    }

    #[test]
    fn chinese_region_labels_map_to_iso_or_nothing() {
        assert_eq!(cn_region("内地"), Some("CN"));
        assert_eq!(cn_region("中国香港"), Some("HK"));
        assert_eq!(cn_region("日本"), Some("JP"));
        assert_eq!(cn_region("火星"), None);
    }
}
