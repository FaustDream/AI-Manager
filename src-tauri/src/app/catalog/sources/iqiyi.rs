//! 爱奇艺频道公开 JSON。实测 `movie` 端点已 404，电影频道走 `film`；
//! 四个频道任一失败不算整源失败，只要还有一个频道有数据就继续用。

use super::{
    check, is_finished, non_empty, normalize_date, year_of, Draft, DESKTOP_UA,
};
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::{Http, HttpRequest};
use futures_util::future::join_all;
use std::collections::BTreeMap;
use std::sync::Arc;

const CHANNELS: &[(&str, u8, &str)] = &[
    ("film", 1, "movie"),
    ("tv", 2, "tv"),
    ("cartoon", 4, "anime"),
    ("variety", 6, "variety"),
];

pub async fn fetch(http: Arc<dyn Http>) -> Result<Vec<Draft>> {
    let requests = CHANNELS.iter().map(|(channel, channel_id, kind)| {
        let http = http.clone();
        async move {
            let url = format!(
                "https://mesh.if.iqiyi.com/portal/lw/v7/channel/{channel}?channel_id={channel_id}\
                 &page_id=&ret_num=30&page_num=1&mode=24&sort=4&without_qipu=1"
            );
            let req = HttpRequest::get(url).header("User-Agent", DESKTOP_UA);
            let resp = http.send(req).await?;
            Ok::<_, AppError>((kind, resp))
        }
    });
    let mut out = Vec::new();
    let mut first_error = None;
    for result in join_all(requests).await {
        let (kind, resp) = match result {
            Ok(value) => value,
            Err(e) => {
                first_error.get_or_insert(e);
                continue;
            }
        };
        match check(resp, "爱奇艺").and_then(|r| parse(kind, &r.body)) {
            Ok(mut items) => out.append(&mut items),
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    if out.is_empty() {
        if let Some(e) = first_error {
            return Err(e);
        }
    }
    Ok(out)
}

fn parse(kind: &'static str, body: &str) -> Result<Vec<Draft>> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|_| AppError::new(ErrorKind::Provider, "爱奇艺返回内容不是 JSON"))?;
    let items = value.get("items").and_then(|v| v.as_array()).ok_or_else(|| {
        AppError::new(ErrorKind::Provider, "爱奇艺返回内容缺少 items")
    })?;

    let mut map: BTreeMap<String, Draft> = BTreeMap::new();
    for item in items {
        let Some(videos) = item.get("video").and_then(|v| v.as_array()) else {
            continue;
        };
        for video in videos {
            let Some(data) = video.get("data").and_then(|v| v.as_array()) else {
                continue;
            };
            for card in data {
                let Some(title) = non_empty(card.get("title").and_then(|v| v.as_str())) else {
                    continue;
                };
                let Some(album_id) = text_id(card.get("album_id")) else {
                    continue;
                };
                let note = non_empty(card.get("dq_updatestatus").and_then(|v| v.as_str()));
                let air_date = date_of(card);
                let page_url = non_empty(card.get("page_url").and_then(|v| v.as_str()))
                    .unwrap_or_else(|| format!("https://www.iqiyi.com/v_{album_id}.html"));
                let draft = Draft {
                    platform: "iqiyi",
                    native_id: album_id.clone(),
                    kind,
                    title,
                    update_note: note.clone(),
                    year: year_of(air_date.as_deref()),
                    air_date,
                    finished: note.as_deref().is_some_and(is_finished),
                    url: Some(page_url),
                    episode_id: None,
                    local_time: None,
                    poster: non_empty(card.get("image_url").and_then(|v| v.as_str())),
                };
                match map.get(&album_id) {
                    Some(old) if old.air_date.is_some() || draft.air_date.is_none() => {}
                    _ => {
                        map.insert(album_id, draft);
                    }
                }
            }
        }
    }
    Ok(map.into_values().collect())
}

fn text_id(value: Option<&serde_json::Value>) -> Option<String> {
    match value? {
        serde_json::Value::String(s) => non_empty(Some(s)),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// `showDate` 是主口径；缺失时从 `date` 对象拼，电影频道只有后者可用。
fn date_of(card: &serde_json::Value) -> Option<String> {
    if let Some(text) = card.get("showDate").and_then(|v| v.as_str()) {
        if let Some(date) = normalize_date(text, 0).filter(|_| text.matches('-').count() == 2) {
            return Some(date);
        }
    }
    let date = card.get("date")?;
    let year = date.get("year")?.as_i64()? as i32;
    let month = date.get("month")?.as_i64()? as u32;
    let day = date.get("day")?.as_i64()? as u32;
    Some(
        chrono::NaiveDate::from_ymd_opt(year, month, day)?
            .format("%Y-%m-%d")
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "items": [
        {"video": [
          {"data": [
            {"title":"两京十五日","album_id":3190714930072901,"page_url":"https://www.iqiyi.com/v_1ewmjgivkm4.html","dq_updatestatus":"更新至12集","showDate":"2026-11-5"},
            {"title":"罪恶拼图","album_id":8848390653908101,"page_url":"https://www.iqiyi.com/v_28z1fta8ytw.html","dq_updatestatus":"24集全","showDate":"2026-11-18"},
            {"title":"没有专辑号","dq_updatestatus":"全1集","showDate":"2026-11-18"}
          ]}
        ]}
      ]
    }"#;

    #[test]
    fn fields_are_normalized_and_finished_is_detected() {
        let items = parse("tv", FIXTURE).unwrap();
        assert_eq!(items.len(), 2, "缺 album_id 的条目应丢掉");
        let ongoing = items
            .iter()
            .find(|i| i.native_id == "3190714930072901")
            .unwrap();
        assert_eq!(ongoing.title, "两京十五日");
        assert_eq!(ongoing.air_date.as_deref(), Some("2026-11-05"));
        assert_eq!(ongoing.year, Some(2026));
        assert_eq!(ongoing.update_note.as_deref(), Some("更新至12集"));
        assert!(!ongoing.finished);

        let done = items
            .iter()
            .find(|i| i.native_id == "8848390653908101")
            .unwrap();
        assert!(done.finished, "「24集全」是完结");
    }

    #[test]
    fn date_object_is_used_when_show_date_is_missing() {
        let body = r#"{"items":[{"video":[{"data":[
          {"title":"电影","album_id":9,"page_url":"https://www.iqiyi.com/v_9.html",
           "dq_updatestatus":"全1集","date":{"year":2026,"month":11,"day":5}}
        ]}]}]}"#;
        let items = parse("movie", body).unwrap();
        assert_eq!(items[0].air_date.as_deref(), Some("2026-11-05"));
        assert_eq!(items[0].kind, "movie");
    }
}
