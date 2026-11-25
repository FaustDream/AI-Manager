mod sources;
mod genres;
mod ratings;
pub(crate) mod store;
pub mod types;
pub use types::*;

use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::Http;
use crate::app::runtime::blocking;
use chrono::{Datelike, NaiveDate};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct Service {
    db: Arc<Db>,
    http: Arc<dyn Http>,
    ingestion: Arc<Mutex<()>>,
}

impl Service {
    pub fn new(db: Arc<Db>, http: Arc<dyn Http>) -> Self {
        Self { db, http, ingestion: Arc::new(Mutex::new(())) }
    }

    async fn ensure_sources(&self) -> Result<()> {
        let db = self.db.clone();
        blocking(move || {
            for s in sources::SOURCES { store::ensure_source(&db, s.id, s.default_enabled)?; }
            Ok::<_, AppError>(())
        }).await?
    }

    pub async fn status(&self) -> Result<Vec<CatalogSourceStatus>> {
        self.ensure_sources().await?;
        let db = self.db.clone();
        blocking(move || {
            sources::SOURCES.iter().map(|s| {
                let mut state = store::source_state(&db, s.id)?.unwrap_or_default();
                state.source = s.id.into(); state.label = s.label.into();
                state.enabled = store::source_enabled(&db, s.id)?;
                state.implemented = s.implemented; state.requires_key = s.requires_key;
                state.configured = !s.requires_key || store::source_key(&db, s.id)?.is_some();
                state.coverage = s.coverage.into(); state.license_note = s.license_note.into();
                state.item_count = store::count_source(&db, s.id)?;
                if state.status.is_empty() { state.status = "idle".into(); }
                if !s.implemented { state.status = "unavailable".into(); }
                else if !state.configured { state.status = "unconfigured".into(); }
                Ok(state)
            }).collect()
        }).await?
    }

    pub async fn set_source(&self, source: String, enabled: bool, token: Option<String>, clear_key: bool) -> Result<Vec<CatalogSourceStatus>> {
        let spec = sources::spec(&source)?;
        if enabled && !spec.implemented { return Err(AppError::new(ErrorKind::Unsupported, "该数据源尚未获得合法接入条件")); }
        if token.is_some() && !spec.requires_key { return Err(AppError::new(ErrorKind::Invalid, "此数据源不需要密钥")); }
        let blob = match token {
            Some(token) => {
                let value = token.trim();
                if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
                    return Err(AppError::new(ErrorKind::Invalid, "API 密钥不能为空或包含控制字符"));
                }
                Some(crate::app::secret::protect(value.as_bytes())?)
            }
            None => None,
        };
        self.ensure_sources().await?;
        let db = self.db.clone();
        blocking(move || {
            if clear_key { store::set_source_key(&db, &source, None)?; }
            if let Some(blob) = blob { store::set_source_key(&db, &source, Some(&blob))?; }
            store::set_source_enabled(&db, &source, enabled)
        }).await??;
        self.status().await
    }

    pub async fn refresh(&self, request: CatalogSyncRequest) -> Result<CatalogRefreshReport> {
        let _guard = self.ingestion.try_lock().map_err(|_| AppError::new(ErrorKind::Conflict, "资料库正在采集，请等待当前任务完成"))?;
        let mode = request.mode.as_deref().unwrap_or("incremental");
        if !matches!(mode, "incremental" | "full" | "history") {
            return Err(AppError::new(ErrorKind::Invalid, "采集模式必须是 incremental、full 或 history"));
        }
        if let Some(ids) = &request.sources { for id in ids { sources::spec(id)?; } }
        let config = crate::app::config::get().catalog;
        if config.countries.iter().any(|c| c.len() != 2 || !c.bytes().all(|b| b.is_ascii_uppercase())) {
            return Err(AppError::new(ErrorKind::Invalid, "采集国家应使用大写双字母ISO地区代码"));
        }
        let today = chrono::Local::now().date_naive();
        let start = parse_date(request.start.as_deref())?.unwrap_or(today - chrono::Duration::days(i64::from(config.history_days)));
        let end = parse_date(request.end.as_deref())?.unwrap_or(today + chrono::Duration::days(i64::from(config.future_days) + 1));
        if start >= end || (end - start).num_days() > 366 {
            return Err(AppError::new(ErrorKind::Invalid, "补采日期范围应大于零且不超过366天（结束日期不包含在内）"));
        }
        let statuses = self.status().await?;
        let mut report = CatalogRefreshReport::default();
        for mut state in statuses {
            let selected = request.sources.as_ref().map_or(state.enabled, |list| list.contains(&state.source));
            if !selected || !state.implemented || !state.configured { continue; }
            let source = state.source.clone();
            let range = format!("{}:{}:{}", mode, start, end);
            let db = self.db.clone();
            let s = source.clone();
            let (blob, previous_range) = blocking(move || Ok::<_, AppError>((store::source_key(&db, &s)?, db.kv_get(&format!("catalog.range.{s}"))?))).await??;
            let token = match blob {
                Some(blob) => Some(String::from_utf8(crate::app::secret::unprotect(&blob)?).map_err(|_| AppError::new(ErrorKind::Crypto, "API 密钥解密失败，请重新配置"))?),
                None => None,
            };
            let cursor = if !state.pagination_complete && previous_range.as_deref() == Some(&range) { state.cursor.clone() } else { None };
            let mut job = CatalogJob {
                id: format!("{}:{}:{}", source, crate::app::timeutil::now_ms(), crate::app::timeutil::now_ms().unsigned_abs()),
                source: source.clone(), mode: mode.into(), started_at: now_text(), status: "running".into(), cursor: cursor.clone(),
                ..Default::default()
            };
            state.last_attempt = Some(job.started_at.clone());
            state.status = "running".into(); state.error = None; state.warning = None;
            let db = self.db.clone(); let j = job.clone(); let st = state.clone();
            let range_stamp = range.clone(); let r = CatalogSyncRequest { sources: Some(vec![source.clone()]), mode: Some(mode.into()), start: Some(start.to_string()), end: Some(end.to_string()) };
            blocking(move || {
                store::save_job(&db, &j)?; store::set_source_state(&db, &st)?;
                db.kv_set(&format!("catalog.range.{}", j.source), &range_stamp)?;
                db.kv_set(&format!("catalog.job.request.{}", j.id), &serde_json::to_string(&r)?)
            }).await??;
            let ctx = sources::AdapterContext {
                http: self.http.clone(), start, end, mode: mode.into(), cursor,
                max_pages: config.max_pages_per_job.max(1), countries: config.countries.clone(), token,
            };
            let (tx, mut rx) = tokio::sync::mpsc::channel(2);
            let source_clone = source.clone();
            let timeout = config.source_timeout_secs.max(1);
            let adapter_db = self.db.clone();
            let handle = tokio::spawn(async move {
                let adapter = async move {
                    if source_clone == "omdb" { ratings::omdb(ctx, adapter_db, tx).await }
                    else if source_clone == "mal" { ratings::mal(ctx, adapter_db, tx).await }
                    else { sources::fetch(&source_clone, ctx, tx).await }
                };
                tokio::time::timeout(std::time::Duration::from_secs(timeout), adapter).await
                    .map_err(|_| AppError::new(ErrorKind::Io, format!("采集超时（{timeout}秒），已成功的分页仍保留")))?
            });
            let mut storage_error = None;
            while let Some(batch) = rx.recv().await {
                let fetched = batch.items.len() as u32;
                let db = self.db.clone(); let now = now_text();
                let counts = match blocking(move || store::upsert(&db, &batch.items, &now)).await {
                    Ok(Ok(counts)) => counts,
                    Ok(Err(err)) | Err(err) => {
                        storage_error = Some(err);
                        handle.abort();
                        break;
                    }
                };
                job.fetched += fetched; job.added += counts.added; job.updated += counts.updated;
                job.deduplicated += counts.deduplicated; job.releases += counts.releases;
                job.pages += 1; job.cursor = batch.cursor; job.pagination_complete = batch.complete;
                job.http_status = batch.http_status.or(job.http_status);
                if batch.warning.is_some() { job.warning = batch.warning; }
                let db = self.db.clone(); let j = job.clone();
                let mut progress = state.clone();
                progress.fetched = job.fetched; progress.added = job.added; progress.updated = job.updated;
                progress.deduplicated = job.deduplicated; progress.releases = job.releases;
                progress.pages = job.pages; progress.cursor = job.cursor.clone();
                progress.pagination_complete = job.pagination_complete; progress.http_status = job.http_status;
                blocking(move || { store::save_job(&db, &j)?; store::set_source_state(&db, &progress) }).await??;
                if job.pages.is_multiple_of(10) { crate::app::events::emit("catalog:updated", &job); }
            }
            let result = match storage_error {
                Some(err) => { let _ = handle.await; Err(err) },
                None => match handle.await {
                    Ok(result) => result,
                    Err(_) => Err(AppError::new(ErrorKind::Io, "采集任务异常终止")),
                },
            };
            if let Err(err) = result {
                job.pagination_complete = false;
                job.failed += 1; job.error = Some(err.message.clone()); job.status = "failed".into();
                if let Some(status) = err.message.split("HTTP ").nth(1).and_then(|s| s.split_whitespace().next()).and_then(|s| s.parse().ok()) { job.http_status = Some(status); }
                crate::app::log::fail("catalog", "catalog_ingestion", format!("{source}采集失败"), err.evidence());
            } else if job.pagination_complete {
                job.status = "success".into(); state.last_success = Some(now_text());
            } else {
                job.status = "partial".into();
                if job.warning.is_none() { job.warning = Some("本轮资源预算已用完，分页未完成；使用相同日期重试可继续，不能视为全量数据".into()); }
            }
            if job.fetched == 0 && job.status == "success" {
                job.warning.get_or_insert("来源在当前覆盖窗口未返回数据，不代表全球没有作品或更新".into());
            }
            job.finished_at = Some(now_text());
            state.status = job.status.clone(); state.fetched = job.fetched; state.added = job.added;
            state.updated = job.updated; state.deduplicated = job.deduplicated; state.releases = job.releases;
            state.failed = job.failed; state.pages = job.pages; state.pagination_complete = job.pagination_complete;
            state.cursor = job.cursor.clone(); state.http_status = job.http_status; state.error = job.error.clone(); state.warning = job.warning.clone();
            let db = self.db.clone(); let j = job.clone(); let st = state;
            blocking(move || { store::save_job(&db, &j)?; store::set_source_state(&db, &st) }).await??;
            report.added += job.added; report.updated += job.updated; report.releases += job.releases;
            crate::app::events::emit("catalog:updated", &job);
        }
        let db = self.db.clone(); report.total = blocking(move || store::count_all(&db)).await??;
        report.sources = self.status().await?;
        crate::app::events::emit("catalog:updated", &report);
        Ok(report)
    }

    pub async fn retry(&self, id: String) -> Result<CatalogRefreshReport> {
        let db = self.db.clone();
        let key = format!("catalog.job.request.{id}");
        let raw = blocking(move || db.kv_get(&key)).await??.ok_or_else(|| AppError::new(ErrorKind::Invalid, "找不到该采集任务"))?;
        self.refresh(serde_json::from_str(&raw)?).await
    }

    pub async fn library(&self, query: CatalogQuery) -> Result<CatalogPage> {
        validate_query(&query)?;
        let db = self.db.clone(); blocking(move || store::library(&db, &query)).await?
    }
    pub async fn releases(&self, query: CatalogQuery) -> Result<CatalogReleasePage> {
        validate_query(&query)?;
        if query.start.is_none() || query.end.is_none() { return Err(AppError::new(ErrorKind::Invalid, "更新日历必须指定开始与结束日期")); }
        let db = self.db.clone(); blocking(move || store::releases(&db, &query)).await?
    }
    pub async fn detail(&self, key: String) -> Result<Option<CatalogDetail>> {
        let db = self.db.clone(); blocking(move || store::detail(&db, &key)).await?
    }
    pub async fn facets(&self) -> Result<CatalogFacets> {
        let db = self.db.clone(); blocking(move || store::facets(&db)).await?
    }
    pub async fn jobs(&self) -> Result<Vec<CatalogJob>> {
        let db = self.db.clone(); blocking(move || store::jobs(&db)).await?
    }
    pub async fn overview(&self) -> Result<CatalogOverview> {
        let today = chrono::Local::now().date_naive();
        let monday = today - chrono::Duration::days(i64::from(today.weekday().num_days_from_monday()));
        let end = monday + chrono::Duration::days(7);
        let db = self.db.clone(); blocking(move || store::overview(&db, &today.to_string(), &monday.to_string(), &end.to_string())).await?
    }

    pub async fn auto_sync(&self) -> Result<CatalogRefreshReport> {
        let config = crate::app::config::get().catalog;
        let db = self.db.clone();
        let last = blocking(move || db.kv_get("catalog.full_check")).await??;
        let full_due = last.and_then(|s| s.parse::<i64>().ok()).is_none_or(|stamp| crate::app::timeutil::now_ms() - stamp >= (config.full_check_days.clamp(1, 365) * 86_400_000) as i64);
        let db = self.db.clone();
        let detail_stamp = blocking(move || db.kv_get("catalog.detail_sync")).await??;
        let detail_due = detail_stamp.and_then(|s| s.parse::<i64>().ok()).is_none_or(|stamp| crate::app::timeutil::now_ms() - stamp >= (config.detail_interval_hours.clamp(1, 8760) * 3_600_000) as i64);
        let states = self.status().await?;
        let jobs = self.jobs().await?;
        let mut resumed = Vec::new();
        let mut report = CatalogRefreshReport::default();
        for state in states.iter().filter(|s| s.enabled && s.implemented && s.configured && s.cursor.is_some() && matches!(s.status.as_str(), "partial" | "failed" | "running")) {
            if let Some(job) = jobs.iter().find(|j| j.source == state.source) {
                let result = self.retry(job.id.clone()).await?;
                report.added += result.added; report.updated += result.updated; report.releases += result.releases;
                resumed.push(state.source.clone());
            }
        }
        let selected = states.iter().filter(|s| s.enabled && s.implemented && s.configured && !resumed.contains(&s.source))
            .filter(|s| detail_due || full_due || !matches!(s.source.as_str(), "bangumi" | "kitsu" | "omdb" | "mal") || s.last_success.is_none())
            .map(|s| s.source.clone()).collect();
        let fresh = self.refresh(CatalogSyncRequest { sources: Some(selected), mode: Some(if full_due { "full" } else { "incremental" }.into()), ..Default::default() }).await?;
        report.added += fresh.added; report.updated += fresh.updated; report.releases += fresh.releases;
        report.total = fresh.total; report.sources = fresh.sources;
        if detail_due && report.sources.iter().filter(|s| s.enabled && s.implemented && s.configured && matches!(s.source.as_str(), "bangumi" | "kitsu" | "omdb" | "mal")).all(|s| s.status == "success") {
            let db = self.db.clone(); blocking(move || db.kv_set("catalog.detail_sync", &crate::app::timeutil::now_ms().to_string())).await??;
        }
        if full_due && report.sources.iter().filter(|s| s.enabled && s.implemented && s.configured).all(|s| s.status == "success") {
            let db = self.db.clone(); blocking(move || db.kv_set("catalog.full_check", &crate::app::timeutil::now_ms().to_string())).await??;
        }
        Ok(report)
    }
}

fn parse_date(raw: Option<&str>) -> Result<Option<NaiveDate>> {
    raw.map(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| AppError::new(ErrorKind::Invalid, "日期格式应为YYYY-MM-DD"))).transpose()
}
fn validate_query(query: &CatalogQuery) -> Result<()> {
    let start = parse_date(query.start.as_deref())?; let end = parse_date(query.end.as_deref())?;
    if matches!((start, end), (Some(s), Some(e)) if s >= e) { return Err(AppError::new(ErrorKind::Invalid, "结束日期必须晚于开始日期")); }
    if query.weekday.is_some_and(|d| !(1..=7).contains(&d)) { return Err(AppError::new(ErrorKind::Invalid, "星期范围为1至7")); }
    if query.search.as_ref().is_some_and(|s| s.len() > 1024) || query.min_rating.is_some_and(|r| !r.is_finite() || !(0.0..=10.0).contains(&r)) {
        return Err(AppError::new(ErrorKind::Invalid, "搜索过长或评分超出0至10"));
    }
    Ok(())
}
fn now_text() -> String { chrono::Local::now().to_rfc3339() }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::net::MockHttp;

    #[tokio::test]
    async fn invalid_range_is_rejected_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let http = Arc::new(MockHttp::new());
        let s = Service::new(Arc::new(Db::open_at(&dir.path().join("catalog.db")).unwrap()), http.clone());
        assert!(s.refresh(CatalogSyncRequest { start: Some("2026-11-25".into()), end: Some("2026-11-24".into()), ..Default::default() }).await.is_err());
        assert!(http.calls().is_empty());
    }

    #[tokio::test]
    async fn keys_are_encrypted_and_never_returned() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open_at(&dir.path().join("catalog.db")).unwrap());
        let s = Service::new(db.clone(), Arc::new(MockHttp::new()));
        let status = s.set_source("tmdb".into(), true, Some("private-test-token".into()), false).await.unwrap();
        let raw = store::source_key(&db, "tmdb").unwrap().unwrap();
        assert_ne!(raw, b"private-test-token");
        assert!(!serde_json::to_string(&status).unwrap().contains("private-test-token"));
        assert!(status.iter().find(|s| s.source == "tmdb").unwrap().configured);
    }

    #[tokio::test]
    #[ignore]
    async fn live_catalog_sources_use_only_temporary_database() {
        let dir = tempfile::tempdir().unwrap();
        let s = Service::new(Arc::new(Db::open_at(&dir.path().join("live.db")).unwrap()), Arc::new(crate::app::net::RealHttp::new()));
        let today = chrono::Local::now().date_naive();
        let report = s.refresh(CatalogSyncRequest { sources: Some(vec!["tvmaze".into(), "anilist".into(), "mangadex".into(), "simkl".into(), "bangumi".into(), "bilibili".into()]), start: Some(today.to_string()), end: Some((today + chrono::Duration::days(1)).to_string()), ..Default::default() }).await.unwrap();
        println!("LIVE_CATALOG {}", serde_json::to_string(&report).unwrap());
        assert!(report.total > 0);
        let page = s.library(CatalogQuery::default()).await.unwrap();
        assert!(!page.items.is_empty());
    }
}
