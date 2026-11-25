//! 爱奇艺频道推荐列表。
//!
//! `pcw-api.iqiyi.com/search/recommend/list` 一条记录已经带中文简介、总集数、
//! 已更新集数、类型/地区标签与评分，不需要逐条详情请求。列表是推荐口径而非
//! 全量目录，覆盖说明里必须写清，不能冒充完整日历。

use super::{cn_region, publish};
use crate::app::catalog::sources::AdapterContext;
use crate::app::catalog::types::{CatalogMedia, CatalogRating, IngestionBatch, MediaDraft};
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::HttpRequest;
use futures_util::future::join_all;
use serde_json::Value;
use tokio::sync::mpsc::Sender;

const LIST_URL: &str = "https://pcw-api.iqiyi.com/search/recommend/list";
const UA: &str = super::DESKTOP_UA;
const RET_NUM: u32 = 30;

/// 频道号沿用爱奇艺频道枚举：1 电影、2 电视剧、4 动漫、6 综艺。
const CHANNELS: &[(u8, &str)] = &[
    (1, "movie"),
    (2, "tv"),
    (4, "anime"),
    (6, "variety"),
];

pub async fn fetch(ctx: AdapterContext, tx: Sender<IngestionBatch>) -> Result<()> {
    let requests = CHANNELS.iter().map(|(channel_id, kind)| {
        let http = ctx.http.clone();
        async move {
            let url = format!("{LIST_URL}?channel_id={channel_id}&mode=24&pageId=1&ret_num={RET_NUM}");
            let req = HttpRequest::get(url).header("User-Agent", UA);
            let resp = http.send(req).await?;
            Ok::<_, AppError>((*kind, resp))
        }
    });
    let mut items = Vec::new();
    let mut first_error = None;
    for result in join_all(requests).await {
        let (kind, resp) = match result {
            Ok(value) => value,
            Err(err) => {
                first_error.get_or_insert(err);
                continue;
            }
        };
        match resp {
            resp if resp.is_ok() => match parse(kind, &resp.body) {
                Ok(mut rows) => items.append(&mut rows),
                Err(err) => {
                    first_error.get_or_insert(err);
                }
            },
            resp => {
                first_error.get_or_insert_with(|| {
                    AppError::new(
                        ErrorKind::Provider,
                        format!("爱奇艺返回 HTTP {}", resp.status),
                    )
                });
            }
        }
    }
    if items.is_empty() {
        if let Some(err) = first_error {
            return Err(err);
        }
    }
    publish(&tx, IngestionBatch {
        items,
        page: 1,
        complete: true,
        http_status: Some(200),
        warning: Some("爱奇艺只取各频道推荐第一页；推荐排序会变，不是全量目录，也不生成单集发行日历".into()),
        ..Default::default()
    })
    .await
}

fn parse(kind: &'static str, body: &str) -> Result<Vec<MediaDraft>> {
    let value: Value = serde_json::from_str(body)
        .map_err(|_| AppError::new(ErrorKind::Provider, "爱奇艺返回内容不是 JSON"))?;
    if value.get("code").and_then(|v| v.as_str()).is_some_and(|code| code != "A00000") {
        return Err(AppError::new(ErrorKind::Provider, "爱奇艺返回错误码"));
    }
    let rows = value
        .pointer("/data/list")
        .and_then(|v| v.as_array())
        .ok_or_else(|| AppError::new(ErrorKind::Provider, "爱奇艺返回内容缺少 data.list"))?;
    let mut out = Vec::new();
    for row in rows {
        let Some(album_id) = row
            .get("albumId")
            .and_then(|v| v.as_i64())
            .filter(|id| *id > 0)
            .map(|id| id.to_string())
        else {
            continue;
        };
        let Some(title) = super::non_empty(row.get("name").and_then(|v| v.as_str())) else {
            continue;
        };
        let video_count = number(row.get("videoCount")).and_then(count_value);
        let latest = number(row.get("latestOrder")).and_then(count_value);
        let status = match (video_count, latest) {
            (Some(total), Some(aired)) if total > 0 && aired >= total => "completed",
            (_, Some(aired)) if aired > 0 => "airing",
            _ => "unknown",
        };
        let mut media = CatalogMedia {
            key: format!("iqiyi:{album_id}"),
            source: "iqiyi".into(),
            title,
            kind: kind.into(),
            platforms: vec!["爱奇艺".into()],
            summary: super::non_empty(row.get("description").and_then(|v| v.as_str())),
            poster: super::non_empty(row.get("imageUrl").and_then(|v| v.as_str())),
            status: status.into(),
            episode_count: video_count,
            aired_episodes: latest.filter(|n| *n > 0),
            source_url: super::non_empty(row.get("playUrl").and_then(|v| v.as_str())),
            ..Default::default()
        };
        if let Some(categories) = row.get("categories").and_then(|v| v.as_array()) {
            for category in categories.iter().filter_map(|v| v.as_str()) {
                let category = category.trim();
                if category.is_empty() || is_language(category) {
                    continue;
                }
                if let Some(code) = cn_region(category) {
                    if !media.regions.iter().any(|r| r == code) {
                        media.regions.push(code.into());
                    }
                    continue;
                }
                if !media.genres.iter().any(|g| g == category) {
                    media.genres.push(category.to_string());
                }
            }
        }
        if let Some(cast) = row
            .pointer("/people/main_charactor")
            .and_then(|v| v.as_array())
        {
            for person in cast.iter().take(12).filter_map(|v| v.get("name")).filter_map(|v| v.as_str()) {
                let name = person.trim();
                if !name.is_empty() && !media.cast.iter().any(|c| c == name) {
                    media.cast.push(name.to_string());
                }
            }
        }
        let mut draft = MediaDraft {
            source: "iqiyi".into(),
            external_id: album_id,
            media,
            ..Default::default()
        };
        if let Some(score) = number(row.get("score")).filter(|v| *v > 0.0) {
            draft.ratings.push(CatalogRating {
                source: "iqiyi".into(),
                category: "audience".into(),
                value: Some(score),
                scale: 10.0,
                url: draft.media.source_url.clone(),
                ..Default::default()
            });
        }
        out.push(draft);
    }
    Ok(out)
}

/// JSON 里的集数可能是数字也可能是字符串；分数是小数，所以统一先取 f64。
fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn count_value(value: f64) -> Option<u32> {
    (value >= 0.0 && value <= u32::MAX as f64).then_some(value as u32)
}

fn is_language(value: &str) -> bool {
    matches!(
        value,
        "普通话" | "英语" | "日语" | "韩语" | "粤语" | "国语" | "原声" | "其它"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "code": "A00000",
      "data": {"has_next": 1, "list": [
        {"albumId": 3190714930072901, "name": "两京十五日", "imageUrl": "http://pic9.iqiyipic.com/a.jpg",
         "description": "改编自马伯庸同名小说。", "videoCount": 26, "latestOrder": 12,
         "categories": ["内地", "普通话", "剧情"], "score": 0,
         "people": {"main_charactor": [{"name": "成毅"}, {"name": "王和"}]},
         "playUrl": "http://www.iqiyi.com/v_1ewmjgivkm4.html"},
        {"albumId": 8848390653908101, "name": "罪恶拼图", "videoCount": 24, "latestOrder": 24,
         "categories": ["内地", "悬疑"], "score": 8.6, "people": {"main_charactor": []}},
        {"albumId": 0, "name": "无专辑号"}
      ]}
    }"#;

    #[test]
    fn ongoing_and_finished_are_distinguished() {
        let items = parse("tv", FIXTURE).unwrap();
        assert_eq!(items.len(), 2, "缺 albumId 的条目要丢掉");
        let ongoing = items.iter().find(|i| i.external_id == "3190714930072901").unwrap();
        assert_eq!(ongoing.media.status, "airing");
        assert_eq!(ongoing.media.episode_count, Some(26));
        assert_eq!(ongoing.media.aired_episodes, Some(12));
        assert_eq!(ongoing.media.summary.as_deref(), Some("改编自马伯庸同名小说。"));
        assert_eq!(ongoing.media.regions, vec!["CN"]);
        assert_eq!(ongoing.media.genres, vec!["剧情"]);
        assert_eq!(ongoing.media.cast, vec!["成毅", "王和"]);
        assert!(ongoing.ratings.is_empty(), "score=0 不造评分");

        let done = items.iter().find(|i| i.external_id == "8848390653908101").unwrap();
        assert_eq!(done.media.status, "completed");
        assert_eq!(done.media.aired_episodes, Some(24));
        assert_eq!(done.media.genres, vec!["悬疑"]);
        assert_eq!(done.ratings.len(), 1);
        assert_eq!(done.ratings[0].value, Some(8.6));
    }

    #[test]
    fn bad_code_is_a_provider_error() {
        let err = parse("tv", r#"{"code":"A00001"}"#).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Provider);
    }

    #[test]
    fn language_tokens_are_not_genres() {
        let body = r#"{"code":"A00000","data":{"list":[
          {"albumId": 9, "name": "测试", "categories": ["内地", "普通话", "纪录片"], "videoCount": 1, "latestOrder": 1}
        ]}}"#;
        let items = parse("documentary", body).unwrap();
        assert_eq!(items[0].media.genres, vec!["纪录片"]);
    }
}
