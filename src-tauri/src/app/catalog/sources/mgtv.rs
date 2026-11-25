//! 芒果TV 频道列表。
//!
//! 公开列表接口一条记录已带中文简介、类型、年份、更新进度与知乎评分，
//! 无需逐条详情。列表按频道推荐排序，不是全量目录，也不生成单集日历。

use super::{episode_numbers, publish};
use crate::app::catalog::sources::AdapterContext;
use crate::app::catalog::types::{CatalogMedia, CatalogRating, IngestionBatch, MediaDraft};
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::HttpRequest;
use futures_util::future::join_all;
use serde_json::Value;
use tokio::sync::mpsc::Sender;

const LIST_URL: &str = "https://pianku.api.mgtv.com/rider/list/pcweb/v3";
const UA: &str = super::MOBILE_UA;

/// 频道号：1 综艺、2 电视剧、3 动漫。电影频道实测返回 400，不猜参数。
const CHANNELS: &[(u8, &str)] = &[(1, "variety"), (2, "tv"), (3, "anime")];

pub async fn fetch(ctx: AdapterContext, tx: Sender<IngestionBatch>) -> Result<()> {
    let requests = CHANNELS.iter().map(|(channel_id, kind)| {
        let http = ctx.http.clone();
        async move {
            let url = format!("{LIST_URL}?channelId={channel_id}&pn=1&pc=30");
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
                        format!("芒果TV返回 HTTP {}", resp.status),
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
        warning: Some("芒果TV只取综艺/电视剧/动漫频道第一页；列表不含确认单集日期，不进入发行日历".into()),
        ..Default::default()
    })
    .await
}

fn parse(kind: &'static str, body: &str) -> Result<Vec<MediaDraft>> {
    let value: Value = serde_json::from_str(body)
        .map_err(|_| AppError::new(ErrorKind::Provider, "芒果TV返回内容不是 JSON"))?;
    if value.get("code").and_then(|v| v.as_i64()) != Some(200) {
        return Err(AppError::new(ErrorKind::Provider, "芒果TV返回错误码"));
    }
    let rows = value
        .pointer("/data/hitDocs")
        .and_then(|v| v.as_array())
        .ok_or_else(|| AppError::new(ErrorKind::Provider, "芒果TV返回内容缺少 data.hitDocs"))?;
    let mut out = Vec::new();
    for row in rows {
        let Some(clip_id) = row
            .get("clipId")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
        else {
            continue;
        };
        let Some(title) = super::non_empty(row.get("title").and_then(|v| v.as_str())) else {
            continue;
        };
        let update_info = super::non_empty(row.get("updateInfo").and_then(|v| v.as_str()));
        let (episode_count, aired_episodes) = update_info
            .as_deref()
            .map(episode_numbers)
            .unwrap_or((None, None));
        let status = if update_info.as_deref().is_some_and(super::is_finished) {
            "completed"
        } else if update_info
            .as_deref()
            .is_some_and(|info| info.contains("更新至") || info.ends_with('期'))
        {
            "airing"
        } else {
            "unknown"
        };
        let mut media = CatalogMedia {
            key: format!("mgtv:{clip_id}"),
            source: "mgtv".into(),
            title,
            kind: kind.into(),
            platforms: vec!["芒果TV".into()],
            summary: super::non_empty(row.get("story").and_then(|v| v.as_str())),
            poster: super::non_empty(row.get("img").and_then(|v| v.as_str())),
            status: status.into(),
            year: row
                .get("year")
                .and_then(|v| v.as_str())
                .and_then(|y| y.trim().parse::<i32>().ok()),
            episode_count,
            aired_episodes,
            source_url: Some(format!("https://www.mgtv.com/b/{clip_id}.html")),
            ..Default::default()
        };
        if let Some(kinds) = row.get("kind").and_then(|v| v.as_array()) {
            for genre in kinds.iter().filter_map(|v| v.as_str()) {
                let genre = genre.trim();
                if !genre.is_empty() && !media.genres.iter().any(|g| g == genre) {
                    media.genres.push(genre.to_string());
                }
            }
        }
        if let Some(subtitle) = row.get("subtitle").and_then(|v| v.as_str()) {
            // 电视剧的 subtitle 是演员表；综艺的 subtitle 是一句定位文案，不能当演员。
            let names: Vec<&str> = subtitle.split([',', '，']).map(str::trim).filter(|s| !s.is_empty()).collect();
            if names.len() >= 2 && names.iter().all(|name| name.chars().count() <= 10) {
                for name in names.into_iter().take(12) {
                    if !media.cast.iter().any(|c| c == name) {
                        media.cast.push(name.to_string());
                    }
                }
            }
        }
        let mut draft = MediaDraft {
            source: "mgtv".into(),
            external_id: clip_id,
            media,
            ..Default::default()
        };
        // 芒果的知乎评分是百分制（实测同一频道会出现 82.2，不是 8.2）。
        if let Some(score) = row
            .get("zhihuScore")
            .and_then(|v| v.as_str())
            .and_then(|s| s.trim().parse::<f64>().ok())
            .filter(|score| *score > 0.0)
        {
            draft.ratings.push(CatalogRating {
                source: "mgtv".into(),
                category: "audience".into(),
                value: Some(score),
                scale: 100.0,
                url: draft.media.source_url.clone(),
                ..Default::default()
            });
        }
        out.push(draft);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "code": 200,
      "data": {"hasMore": true, "hitDocs": [
        {"clipId": "817372", "title": "今天是太阳雨", "img": "http://2img.hitv.com/a.jpg",
         "story": "倔强少女郑雨与孤独少年陈砚周的故事。",
         "kind": ["青春", "爱情", "言情"], "year": "2026", "updateInfo": "全24集",
         "subtitle": "杨超越,董思成,孙宁", "zhihuScore": "84.2", "se_updateTime": "2026-11-25 11:54:48"},
        {"clipId": "914227", "title": "后会友期", "kind": ["真人秀"], "year": "2026",
         "updateInfo": "2026-11-20期", "subtitle": "友情治愈纪实类真人秀", "zhihuScore": "0.0"},
        {"clipId": "", "title": "无 id"}
      ]}
    }"#;

    #[test]
    fn episodes_year_and_rating_are_parsed() {
        let items = parse("tv", FIXTURE).unwrap();
        assert_eq!(items.len(), 2, "缺 clipId 的条目要丢掉");
        let show = &items[0];
        assert_eq!(show.media.status, "completed");
        assert_eq!(show.media.episode_count, Some(24));
        assert_eq!(show.media.aired_episodes, Some(24));
        assert_eq!(show.media.year, Some(2026));
        assert_eq!(show.media.genres, vec!["青春", "爱情", "言情"]);
        assert_eq!(show.media.cast, vec!["杨超越", "董思成", "孙宁"]);
        assert_eq!(show.media.summary.as_deref(), Some("倔强少女郑雨与孤独少年陈砚周的故事。"));
        assert_eq!(show.ratings.len(), 1);
        assert_eq!(show.ratings[0].value, Some(84.2));
        assert_eq!(show.ratings[0].scale, 100.0);

        let variety = &items[1];
        assert_eq!(variety.media.status, "airing");
        assert_eq!(variety.media.episode_count, None);
        assert!(variety.media.cast.is_empty(), "综艺的定位文案不是演员表");
        assert!(variety.ratings.is_empty(), "0 分不造评分");
    }
}
