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
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CatalogSourceStatus {
    pub source: String,
    pub label: String,
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
