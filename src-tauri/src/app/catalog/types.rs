use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CatalogMedia {
    pub key: String,
    pub title: String,
    pub original_title: Option<String>,
    pub english_title: Option<String>,
    pub aliases: Vec<String>,
    pub kind: String,
    pub subkind: Option<String>,
    pub regions: Vec<String>,
    pub genres: Vec<String>,
    pub poster: Option<String>,
    pub backdrop: Option<String>,
    pub summary: Option<String>,
    pub directors: Vec<String>,
    pub writers: Vec<String>,
    pub cast: Vec<String>,
    pub studios: Vec<String>,
    pub distributors: Vec<String>,
    pub platforms: Vec<String>,
    pub status: String,
    pub year: Option<i32>,
    pub premiered: Option<String>,
    pub ended: Option<String>,
    pub season_count: Option<u32>,
    pub episode_count: Option<u32>,
    pub aired_episodes: Option<u32>,
    pub runtime: Option<u32>,
    pub chapter_count: Option<u32>,
    pub latest_chapter: Option<String>,
    pub rating: Option<f64>,
    pub rating_source: Option<String>,
    pub source: String,
    pub source_url: Option<String>,
    pub first_seen: String,
    pub updated_at: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CatalogExternalId {
    pub source: String,
    pub external_id: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CatalogRating {
    pub source: String,
    pub category: String,
    pub value: Option<f64>,
    pub scale: f64,
    pub votes: Option<u32>,
    pub reviews: Option<u32>,
    pub positive_percent: Option<f64>,
    pub rank: Option<u32>,
    pub url: Option<String>,
    pub updated_at: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CatalogSeason {
    pub source: String,
    pub external_id: String,
    pub number: Option<u32>,
    pub title: Option<String>,
    pub episode_count: Option<u32>,
    pub premiered: Option<String>,
    pub ended: Option<String>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CatalogRelease {
    pub key: String,
    pub media_key: String,
    pub source: String,
    pub external_id: String,
    pub date: String,
    pub local_time: Option<String>,
    pub timezone: Option<String>,
    pub utc_time: Option<String>,
    pub season: Option<u32>,
    pub episode: Option<String>,
    pub title: Option<String>,
    pub platform: Option<String>,
    pub release_kind: String,
    pub url: Option<String>,
    pub updated_at: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogReleaseRow {
    pub release: CatalogRelease,
    pub media: CatalogMedia,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogDetail {
    pub media: CatalogMedia,
    pub external_ids: Vec<CatalogExternalId>,
    pub ratings: Vec<CatalogRating>,
    pub seasons: Vec<CatalogSeason>,
    pub releases: Vec<CatalogRelease>,
    /// 同一作品在各采集源的落地信息（标题/网址/集数文案），合并后仍逐源保留
    pub source_infos: Vec<CatalogSourceInfo>,
    /// 疑似同一作品但证据不足的候选（标题一致、年份缺失或冲突），供人工合并
    pub dupe_candidates: Vec<CatalogDupeCandidate>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogSourceInfo {
    pub source: String,
    pub title: Option<String>,
    pub url: Option<String>,
    pub episode_text: Option<String>,
    pub updated_at: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogDupeCandidate {
    pub key_a: String,
    pub key_b: String,
    pub reason: String,
    pub title_a: String,
    pub title_b: String,
    pub year_a: Option<i32>,
    pub year_b: Option<i32>,
    pub created_at: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CatalogQuery {
    pub search: Option<String>,
    pub kind: Option<String>,
    pub region: Option<String>,
    pub genre: Option<String>,
    pub platform: Option<String>,
    pub status: Option<String>,
    pub year: Option<i32>,
    pub min_rating: Option<f64>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub weekday: Option<u32>,
    pub sort: Option<String>,
    pub page: Option<u32>,
    pub page_size: Option<u32>,
    /// 国内 / 国外页签：domestic=国内源采集的条目，foreign=国际源。空 = 全部
    pub scope: Option<String>,
    /// 地区多选（任一命中即通过；与 region 单选并存，UI 目前用单选）
    pub regions: Vec<String>,
    /// 题材多选，配合 genre_mode 决定任一/全部
    pub genres: Vec<String>,
    pub genre_mode: Option<String>,
    /// 演员多选，配合 cast_mode 决定任一/全部（「两个演员同框」用 all）
    pub cast: Vec<String>,
    pub cast_mode: Option<String>,
    /// 年份区间（与单值 year 并存；区间优先语义上与 year 相互独立）
    pub year_from: Option<i32>,
    pub year_to: Option<i32>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogFollowRow {
    pub media: CatalogMedia,
    /// want=想看 / watching=在追 / shelve=搁置 / done=看完
    pub state: String,
    pub updated_at: String,
    /// 最近一次确认发行的日期（无发行记录为空）
    pub latest_release: Option<String>,
    /// 最近一次确认发行的集数文案
    pub latest_episode: Option<String>,
    /// 未来最近一次计划发行日期（无为空）
    pub next_release: Option<String>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CatalogMergeRequest {
    /// 保留的条目（当前查看的资料库 ID）
    pub keep: String,
    /// 被合并的条目（疑似重复的另一条）
    pub drop: String,
    /// true=执行合并；false=标记「不是同一作品」忽略候选
    pub merge: bool,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct CatalogPage {
    pub items: Vec<CatalogMedia>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total: i64,
    pub page: u32,
    pub page_size: u32,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct CatalogReleasePage {
    pub items: Vec<CatalogReleaseRow>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total: i64,
    pub page: u32,
    pub page_size: u32,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize)]
pub struct CatalogOverview {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub today_added: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub today_releases: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub week_added: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub week_updated: i64,
    pub upcoming: Vec<CatalogMedia>,
    pub latest: Vec<CatalogMedia>,
    pub highly_rated: Vec<CatalogMedia>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize)]
pub struct CatalogFacetCount {
    pub key: String,
    pub label: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub count: i64,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize)]
pub struct CatalogFacets {
    pub kinds: Vec<CatalogFacetCount>,
    pub regions: Vec<CatalogFacetCount>,
    pub genres: Vec<CatalogFacetCount>,
    pub platforms: Vec<CatalogFacetCount>,
    pub years: Vec<i32>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CatalogSyncRequest {
    pub sources: Option<Vec<String>>,
    pub mode: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    /// 本轮每源页预算覆盖（热增量用小预算，只拉最新页面；缺省用 config.max_pages_per_job）
    pub max_pages: Option<u32>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CatalogSourceStatus {
    pub source: String,
    pub label: String,
    /// 国内 / 国外页签归属（评分回填源为空串）
    pub scope: String,
    pub enabled: bool,
    pub implemented: bool,
    pub requires_key: bool,
    pub configured: bool,
    pub coverage: String,
    pub license_note: String,
    pub status: String,
    pub last_attempt: Option<String>,
    pub last_success: Option<String>,
    pub fetched: u32,
    pub added: u32,
    pub updated: u32,
    pub deduplicated: u32,
    pub releases: u32,
    pub failed: u32,
    pub pages: u32,
    pub pagination_complete: bool,
    pub cursor: Option<String>,
    pub http_status: Option<u16>,
    pub error: Option<String>,
    pub warning: Option<String>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub item_count: i64,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CatalogRefreshReport {
    pub sources: Vec<CatalogSourceStatus>,
    pub added: u32,
    pub updated: u32,
    pub releases: u32,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total: i64,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CatalogJob {
    pub id: String,
    pub source: String,
    pub mode: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub status: String,
    pub fetched: u32,
    pub added: u32,
    pub updated: u32,
    pub deduplicated: u32,
    pub releases: u32,
    pub failed: u32,
    pub pages: u32,
    pub pagination_complete: bool,
    pub cursor: Option<String>,
    pub http_status: Option<u16>,
    pub error: Option<String>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct MediaDraft {
    pub source: String,
    pub external_id: String,
    pub media: CatalogMedia,
    pub external_ids: Vec<CatalogExternalId>,
    pub ratings: Vec<CatalogRating>,
    pub seasons: Vec<CatalogSeason>,
    pub releases: Vec<CatalogRelease>,
}

#[derive(Debug, Clone, Default)]
pub struct IngestionBatch {
    pub items: Vec<MediaDraft>,
    pub page: u32,
    pub cursor: Option<String>,
    pub complete: bool,
    pub http_status: Option<u16>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct IngestionCounts {
    pub added: u32,
    pub updated: u32,
    pub deduplicated: u32,
    pub releases: u32,
}
