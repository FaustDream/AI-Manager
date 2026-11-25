use super::sources::{json, pace, publish, AdapterContext};
use super::types::*;
use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::HttpRequest;
use crate::app::runtime::blocking;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

pub async fn omdb(ctx: AdapterContext, db: Arc<Db>, tx: Sender<IngestionBatch>) -> Result<()> {
    let token = ctx.token.as_deref().ok_or_else(|| AppError::new(ErrorKind::Invalid, "OMDb需要配置合法API密钥"))?;
    let mut page = ctx.cursor.as_deref().and_then(|s| s.parse::<u32>().ok()).unwrap_or(1);
    for count in 0..ctx.max_pages {
        let d = db.clone();
        let library = blocking(move || super::store::library(&d, &CatalogQuery { page: Some(page), page_size: Some(20), sort: Some("title".into()), ..Default::default() })).await??;
        let complete = i64::from(page * library.page_size) >= library.total;
        let mut items = Vec::new();
        for media in library.items {
            let d = db.clone(); let key = media.key.clone();
            let detail = blocking(move || super::store::detail(&d, &key)).await??;
            let Some(imdb) = detail.and_then(|d| d.external_ids.into_iter().find(|i| i.source == "imdb")) else { continue; };
            if !imdb.external_id.starts_with("tt") || !imdb.external_id[2..].bytes().all(|b| b.is_ascii_digit()) { continue; }
            let mut url = reqwest::Url::parse("https://www.omdbapi.com/").expect("固定API地址有效");
            url.query_pairs_mut().append_pair("apikey", token).append_pair("i", &imdb.external_id);
            let value = json(&ctx.http, HttpRequest::get(url.as_str())).await.map_err(|e| AppError::new(e.kind, "OMDb请求失败，请检查网络、API密钥和配额；密钥不会记录到日志"))?;
            if value.get("Response").and_then(Value::as_str) == Some("False") {
                return Err(AppError::new(ErrorKind::Provider, "OMDb未返回作品资料，请核对密钥、配额和IMDb ID"));
            }
            let mut draft = MediaDraft { source: "omdb".into(), external_id: imdb.external_id.clone(), external_ids: vec![imdb], media: media.clone(), ratings: parse_ratings(&value), ..Default::default() };
            draft.media.rating = None; draft.media.rating_source = None;
            items.push(draft);
            pace().await;
        }
        page += 1;
        publish(&tx, IngestionBatch { items, page: count + 1, cursor: (!complete).then(|| page.to_string()), complete, http_status: Some(200), ..Default::default() }).await?;
        if complete { return Ok(()); }
    }
    Ok(())
}

pub async fn mal(ctx: AdapterContext, db: Arc<Db>, tx: Sender<IngestionBatch>) -> Result<()> {
    let token = ctx.token.as_deref().ok_or_else(|| AppError::new(ErrorKind::Invalid, "MyAnimeList需要配置官方API客户端ID"))?;
    let mut page = ctx.cursor.as_deref().and_then(|s| s.parse::<u32>().ok()).unwrap_or(1);
    for count in 0..ctx.max_pages {
        let d = db.clone();
        let library = blocking(move || super::store::library(&d, &CatalogQuery { page: Some(page), page_size: Some(20), sort: Some("title".into()), ..Default::default() })).await??;
        let complete = i64::from(page * library.page_size) >= library.total;
        let mut items = Vec::new();
        for media in library.items {
            // MAL ID 在动漫与漫画间同号不同义，按作品类型选端点，避免把漫画 ID 当动漫查。
            let path = match media.kind.as_str() { "anime" => "anime", "manga" => "manga", _ => continue };
            let d = db.clone(); let key = media.key.clone();
            let detail = blocking(move || super::store::detail(&d, &key)).await??;
            let Some(mal) = detail.and_then(|d| d.external_ids.into_iter().find(|i| i.source == "mal")) else { continue; };
            if mal.external_id.is_empty() || !mal.external_id.bytes().all(|b| b.is_ascii_digit()) { continue; }
            let url = format!("https://api.myanimelist.net/v2/{path}/{}?fields=mean,rank,num_scoring_users", mal.external_id);
            let req = HttpRequest::get(url).header("X-MAL-CLIENT-ID", token);
            let value = json(&ctx.http, req).await.map_err(|e| AppError::new(e.kind, "MyAnimeList请求失败，请检查网络与客户端ID；密钥不会记录到日志"))?;
            let mut draft = MediaDraft { source: "mal".into(), external_id: mal.external_id.clone(), external_ids: vec![mal], media: media.clone(), ..Default::default() };
            draft.media.rating = None; draft.media.rating_source = None;
            if let Some(rating) = mal_rating(&value, path, &draft.external_id) { draft.ratings.push(rating); }
            items.push(draft);
            pace().await;
        }
        page += 1;
        publish(&tx, IngestionBatch { items, page: count + 1, cursor: (!complete).then(|| page.to_string()), complete, http_status: Some(200), ..Default::default() }).await?;
        if complete { return Ok(()); }
    }
    Ok(())
}

fn mal_rating(value: &Value, path: &str, id: &str) -> Option<CatalogRating> {
    let mean = value.get("mean").and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0 && *v <= 10.0)?;
    Some(CatalogRating {
        source: "mal".into(), category: "user".into(), value: Some(mean), scale: 10.0,
        votes: value.get("num_scoring_users").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()),
        rank: value.get("rank").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()),
        url: Some(format!("https://myanimelist.net/{path}/{id}")), ..Default::default()
    })
}

fn numeric(raw: Option<&str>) -> Option<f64> { raw?.replace(',', "").parse().ok() }
fn parse_ratings(value: &Value) -> Vec<CatalogRating> {
    let mut out = Vec::new();
    if let Some(score) = numeric(value.get("imdbRating").and_then(Value::as_str)) {
        out.push(CatalogRating { source: "imdb".into(), category: "user".into(), value: Some(score), scale: 10.0,
            votes: numeric(value.get("imdbVotes").and_then(Value::as_str)).map(|n| n as u32), ..Default::default() });
    }
    if let Some(score) = numeric(value.get("Metascore").and_then(Value::as_str)) {
        out.push(CatalogRating { source: "metacritic".into(), category: "critic".into(), value: Some(score), scale: 100.0, ..Default::default() });
    }
    if let Some(ratings) = value.get("Ratings").and_then(Value::as_array) {
        for r in ratings {
            if r.get("Source").and_then(Value::as_str) != Some("Rotten Tomatoes") { continue; }
            if let Some(score) = r.get("Value").and_then(Value::as_str).and_then(|s| s.strip_suffix('%')).and_then(|s| s.parse::<f64>().ok()) {
                out.push(CatalogRating { source: "rotten_tomatoes".into(), category: "critic".into(), value: Some(score), scale: 100.0, positive_percent: Some(score), ..Default::default() });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mal_rating_uses_mean_scale_ten_and_scoring_users() {
        let r = mal_rating(&serde_json::json!({"mean":8.41,"rank":120,"num_scoring_users":53000}), "anime", "1").unwrap();
        assert_eq!((r.source.as_str(), r.category.as_str(), r.scale), ("mal", "user", 10.0));
        assert_eq!(r.value, Some(8.41));
        assert_eq!(r.votes, Some(53000));
        assert_eq!(r.rank, Some(120));
        assert_eq!(r.url.as_deref(), Some("https://myanimelist.net/anime/1"));
        assert!(mal_rating(&serde_json::json!({"mean":null}), "anime", "1").is_none());
        assert!(mal_rating(&serde_json::json!({"mean":99.0}), "anime", "1").is_none());
    }

    #[test]
    fn original_scales_and_critic_user_categories_stay_separate() {
        let scores = parse_ratings(&serde_json::json!({"imdbRating":"8.4","imdbVotes":"12,345","Metascore":"81","Ratings":[{"Source":"Rotten Tomatoes","Value":"94%"}]}));
        assert_eq!(scores.len(), 3);
        assert_eq!(scores[0].votes, Some(12345));
        assert_eq!(scores[0].category, "user");
        assert_eq!(scores[1].scale, 100.0);
        assert_eq!(scores[2].positive_percent, Some(94.0));
        assert!(scores[2].votes.is_none());
        assert!(parse_ratings(&serde_json::json!({"imdbRating":"N/A","Metascore":"N/A"})).is_empty());
    }
}
