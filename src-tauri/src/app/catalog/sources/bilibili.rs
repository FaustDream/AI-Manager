//! B 站每日更新。时间线接口自带精确日期，是四个源里最可靠的一个；
//! 国创（types=4）并入动漫，因为它与番剧在用户侧是同一类追更目标。

use super::{
    check, is_finished, non_empty, normalize_date, year_of, Draft, DESKTOP_UA,
};
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::{Http, HttpRequest};
use chrono::{DateTime, Datelike, FixedOffset};
use futures_util::future::join_all;
use std::collections::BTreeMap;
use std::sync::Arc;

const TYPES: &[(u8, &str)] = &[
    (1, "anime"),
    (2, "movie"),
    (3, "documentary"),
    (4, "anime"),
    (5, "tv"),
    (6, "variety"),
];

pub async fn fetch(http: Arc<dyn Http>) -> Result<Vec<Draft>> {
    let requests = TYPES.iter().map(|(kind_id, kind)| {
        let http = http.clone();
        async move {
            let url = format!(
                "https://api.bilibili.com/pgc/web/timeline?types={kind_id}&before=7&after=7"
            );
            let req = HttpRequest::get(url)
                .header("User-Agent", DESKTOP_UA)
                .header("Referer", "https://www.bilibili.com/");
            let resp = check(http.send(req).await?, "哔哩哔哩")?;
            parse(kind, &resp.body)
        }
    });
    let mut out = Vec::new();
    for result in join_all(requests).await {
        out.extend(result?);
    }
    Ok(out)
}

fn parse(kind: &'static str, body: &str) -> Result<Vec<Draft>> {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|_| {
        AppError::new(ErrorKind::Provider, "哔哩哔哩返回内容不是 JSON")
    })?;
    if let Some(code) = value.get("code").and_then(|v| v.as_i64()) {
        if code != 0 {
            let message = value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("未知错误");
            return Err(AppError::new(
                ErrorKind::Provider,
                format!("哔哩哔哩时间线返回错误：{message}"),
            ));
        }
    }
    let days = value
        .get("result")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            AppError::new(ErrorKind::Provider, "哔哩哔哩时间线缺少 result 数组")
        })?;

    let fallback_year = chrono::Local::now().year();
    // 同一集可能重复出现在时间线中，去重不能压掉同季的其他更新。
    let mut collected: Vec<(String, i64, Draft)> = Vec::new();
    for day in days {
        let day_ts = day.get("date_ts").and_then(|v| v.as_i64()).unwrap_or(0);
        let day_date = day.get("date").and_then(|v| v.as_str()).unwrap_or("");
        let Some(episodes) = day.get("episodes").and_then(|v| v.as_array()) else {
            continue;
        };
        for ep in episodes {
            let Some(title) = non_empty(ep.get("title").and_then(|v| v.as_str())) else {
                continue;
            };
            let episode_id = ep.get("episode_id").and_then(|v| v.as_i64()).unwrap_or(0);
            if episode_id == 0 {
                continue;
            }
            let pub_ts = ep.get("pub_ts").and_then(|v| v.as_i64()).unwrap_or(day_ts);
            let air_date = date_from_ts(if day_ts > 0 { day_ts } else { pub_ts })
                .or_else(|| normalize_date(day_date, fallback_year));
            let native_id = ep
                .get("season_id")
                .and_then(|v| v.as_i64())
                .filter(|id| *id > 0)
                .map(|id| id.to_string())
                .unwrap_or_else(|| episode_id.to_string());
            let note = non_empty(ep.get("pub_index").and_then(|v| v.as_str()));
            let finished = note.as_deref().is_some_and(is_finished);
            collected.push((
                air_date.clone().unwrap_or_default(),
                pub_ts,
                Draft {
                    platform: "bilibili",
                    native_id: native_id.clone(),
                    kind,
                    title,
                    update_note: note,
                    year: year_of(air_date.as_deref()),
                    air_date,
                    finished,
                    url: Some("https://api.bilibili.com/pgc/web/timeline".into()),
                    episode_id: Some(episode_id.to_string()),
                    local_time: non_empty(ep.get("pub_time").and_then(|v| v.as_str())),
                    poster: non_empty(ep.get("cover").and_then(|v| v.as_str())),
                },
            ));
        }
    }
    collected.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let mut map: BTreeMap<String, Draft> = BTreeMap::new();
    for (_, _, draft) in collected {
        map.entry(draft.episode_id.clone().unwrap_or_else(|| draft.key())).or_insert(draft);
    }
    Ok(map.into_values().collect())
}

/// B 站给的是东八区自然日的时间戳，按 +08 取日期才是用户看到的那一天。
fn date_from_ts(ts: i64) -> Option<String> {
    if ts <= 0 {
        return None;
    }
    let offset = FixedOffset::east_opt(8 * 3600)?;
    let dt = DateTime::from_timestamp(ts, 0)?.with_timezone(&offset);
    Some(dt.format("%Y-%m-%d").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "code": 0,
      "message": "success",
      "result": [
        {
          "date": "11-17",
          "date_ts": 1794844800,
          "day_of_week": 2,
          "episodes": [
            {
              "episode_id": 7702237,
              "pub_index": "第15话~第16话",
              "pub_time": "10:00",
              "pub_ts": 1794880800,
              "season_id": 355139,
              "title": "小羊肖恩 第七季"
            }
          ]
        },
        {
          "date": "11-20",
          "date_ts": 1795104000,
          "day_of_week": 5,
          "episodes": [
            {
              "episode_id": 7703000,
              "pub_index": "第20话",
              "pub_ts": 1795120000,
              "season_id": 355139,
              "title": "小羊肖恩 第七季"
            }
          ]
        }
      ]
    }"#;

    #[test]
    fn episode_fields_are_normalized_and_each_episode_is_kept() {
        let items = parse("anime", FIXTURE).unwrap();
        assert_eq!(items.len(), 2, "不同集的历史更新必须分别保留");
        let latest = items.iter().find(|i| i.update_note.as_deref() == Some("第20话")).unwrap();
        assert_eq!(latest.native_id, "355139");
        assert_eq!(latest.title, "小羊肖恩 第七季");
        assert_eq!(latest.air_date.as_deref(), Some("2026-11-20"));
        assert_eq!(latest.year, Some(2026));
        assert_eq!(
            latest.url.as_deref(),
            Some("https://api.bilibili.com/pgc/web/timeline")
        );
        assert!(!latest.finished);
        assert_eq!(latest.kind, "anime");
        let earlier = items.iter().find(|i| i.update_note.as_deref() == Some("第15话~第16话")).unwrap();
        assert_eq!(earlier.air_date.as_deref(), Some("2026-11-17"));
        assert_eq!(earlier.episode_id.as_deref(), Some("7702237"));
    }

    #[test]
    fn two_part_date_falls_back_to_local_year() {
        let body = r#"{"code":0,"result":[{"date":"12-01","episodes":[
            {"episode_id":1,"pub_index":"第1话","season_id":9,"title":"测试剧"}]}]}"#;
        let items = parse("anime", body).unwrap();
        let expected = format!("{}-12-01", chrono::Local::now().year());
        assert_eq!(items[0].air_date.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn non_zero_code_is_a_provider_error() {
        let err = parse("anime", r#"{"code":-412,"message":"请求被拦截"}"#).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Provider);
        assert!(err.message.contains("请求被拦截"));
    }
}
