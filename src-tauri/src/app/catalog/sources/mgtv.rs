//! 芒果TV 频道列表。
//!
//! 公开列表接口一条记录已带中文简介、类型、年份、更新进度与知乎评分，
//! 无需逐条详情。`se_updateTime` 是该记录的最近更新时刻，记成一条
//! release（每个条目一行，刷新覆盖），让「最新更新」视图能看到芒果的动态。
//! 列表按频道推荐排序，不是全量目录。

use super::{episode_numbers, pace, publish, valid_date};
use crate::app::catalog::sources::AdapterContext;
use crate::app::catalog::types::{CatalogMedia, CatalogRating, CatalogRelease, IngestionBatch, MediaDraft};
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::HttpRequest;
use serde_json::Value;
use tokio::sync::mpsc::Sender;

const LIST_URL: &str = "https://pianku.api.mgtv.com/rider/list/pcweb/v3";
const UA: &str = super::MOBILE_UA;

/// 频道号：1 综艺、2 电视剧、3 动漫。电影频道实测返回 400，不猜参数。
const CHANNELS: &[(u8, &str)] = &[(1, "variety"), (2, "tv"), (3, "anime")];

pub async fn fetch(ctx: AdapterContext, tx: Sender<IngestionBatch>) -> Result<()> {
    // 每频道页数：预算平摊到 3 个频道；热增量的小预算也能覆盖各频道第一页
    let pages_per_channel = (ctx.max_pages / CHANNELS.len() as u32).clamp(1, 40);
    let mut items = Vec::new();
    let mut first_error = None;
    for (channel_id, kind) in CHANNELS {
        for pn in 1..=pages_per_channel {
            let url = format!("{LIST_URL}?channelId={channel_id}&pn={pn}&pc=30");
            let req = HttpRequest::get(url).header("User-Agent", UA);
            let resp = match ctx.http.send(req).await {
                Ok(resp) => resp,
                Err(err) => { first_error.get_or_insert(err); break; }
            };
            if !resp.is_ok() {
                first_error.get_or_insert_with(|| {
                    AppError::new(ErrorKind::Provider, format!("芒果TV返回 HTTP {}", resp.status))
                });
                break;
            }
            let drafts = match parse(kind, &resp.body) {
                Ok(drafts) => drafts,
                Err(err) => { first_error.get_or_insert(err); break; }
            };
            let exhausted = drafts.is_empty();
            items.extend(drafts);
            if exhausted { break; }
            if pn < pages_per_channel { pace().await; }
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
        warning: Some("芒果TV拉综艺/电视剧/动漫频道列表（预算内翻页），se_updateTime 记为该条目的最近更新；列表不含确认单集日期，历史逐集不进发行日历".into()),
        ..Default::default()
    })
    .await
}

/// `se_updateTime` 形如 `2026-11-25 11:54:48`：拆成发行日期 + 当地时刻，
/// 并合成 RFC3339 戳（芒果是中国平台，固定 +08:00）。
fn parse_update_time(raw: &str) -> Option<(String, Option<String>, String)> {
    let raw = raw.trim();
    let (date, time) = raw.split_once(' ')?;
    if !valid_date(date) {
        return None;
    }
    let time = time.trim();
    let stamp = format!("{date}T{time}+08:00");
    Some((date.to_string(), Some(time.to_string()), stamp))
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
        // se_updateTime → 每条目一行「最近更新」release：同一 external_id 的行在
        // save_release 里按戳更新，热刷新轮次自动把它推到最新更新视图。
        if let Some(update_time) = row.get("se_updateTime").and_then(|v| v.as_str()) {
            if let Some((date, local_time, stamp)) = parse_update_time(update_time) {
                draft.releases.push(CatalogRelease {
                    source: "mgtv".into(),
                    external_id: format!("upd:{}", draft.external_id),
                    date,
                    local_time,
                    title: update_info.clone(),
                    release_kind: "update".into(),
                    url: draft.media.source_url.clone(),
                    updated_at: stamp,
                    ..Default::default()
                });
            }
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

        // se_updateTime → 每条目一条「最近更新」release，戳用 +08:00
        let show = &items[0];
        assert_eq!(show.releases.len(), 1, "有 se_updateTime 就要有更新记录");
        let release = &show.releases[0];
        assert_eq!(release.date, "2026-11-25");
        assert_eq!(release.local_time.as_deref(), Some("11:54:48"));
        assert_eq!(release.updated_at, "2026-11-25T11:54:48+08:00");
        assert_eq!(release.release_kind, "update");
        assert_eq!(release.title.as_deref(), Some("全24集"));
    }
}
