use super::*;
use crate::app::catalog::*;

pub async fn catalog_status(state: &AppState) -> std::result::Result<Vec<CatalogSourceStatus>, String> {
    state.catalog.status().await.map_err(err_text)
}
pub async fn catalog_refresh(state: &AppState, request: CatalogSyncRequest) -> std::result::Result<CatalogRefreshReport, String> {
    state.catalog.refresh(request).await.map_err(err_text)
}
pub async fn catalog_library(state: &AppState, query: CatalogQuery) -> std::result::Result<CatalogPage, String> {
    state.catalog.library(query).await.map_err(err_text)
}
pub async fn catalog_releases(state: &AppState, query: CatalogQuery) -> std::result::Result<CatalogReleasePage, String> {
    state.catalog.releases(query).await.map_err(err_text)
}
pub async fn catalog_detail(state: &AppState, key: String) -> std::result::Result<Option<CatalogDetail>, String> {
    state.catalog.detail(key).await.map_err(err_text)
}
pub async fn catalog_overview(state: &AppState, scope: Option<String>) -> std::result::Result<CatalogOverview, String> {
    state.catalog.overview(scope).await.map_err(err_text)
}
pub async fn catalog_facets(state: &AppState, query: CatalogQuery) -> std::result::Result<CatalogFacets, String> {
    state.catalog.facets(query).await.map_err(err_text)
}
pub async fn catalog_merge(state: &AppState, request: CatalogMergeRequest) -> std::result::Result<String, String> {
    state.catalog.merge(request).await.map_err(err_text)
}
pub async fn catalog_follow_list(state: &AppState, states: Option<Vec<String>>) -> std::result::Result<Vec<CatalogFollowRow>, String> {
    state.catalog.follow_list(states).await.map_err(err_text)
}
pub async fn catalog_follow_set(app: &AppState, key: String, follow_state: Option<String>) -> std::result::Result<String, String> {
    app.catalog.follow_set(key, follow_state).await.map(|_| "追剧状态已更新".to_string()).map_err(err_text)
}
pub async fn catalog_jobs(state: &AppState) -> std::result::Result<Vec<CatalogJob>, String> {
    state.catalog.jobs().await.map_err(err_text)
}
pub async fn catalog_source_set(state: &AppState, source: String, enabled: bool, token: Option<String>, clear_key: Option<bool>) -> std::result::Result<Vec<CatalogSourceStatus>, String> {
    state.catalog.set_source(source, enabled, token, clear_key.unwrap_or(false)).await.map_err(err_text)
}
pub async fn catalog_retry(state: &AppState, id: String) -> std::result::Result<CatalogRefreshReport, String> {
    state.catalog.retry(id).await.map_err(err_text)
}
