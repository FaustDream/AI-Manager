//! 腾讯视频移动端首页：SSR 把频道模块塞进 `window.__PINIA__`，没有更轻的公开接口。
//!
//! 同一部剧会在多个模块里重复出现，且首屏模块的 `title` 常是运营文案
//! （例如「撞破亲哥出轨！」），后置模块才是剧名——去重时保留更短的那个。

use super::{
    check, is_finished, non_empty, normalize_date, year_of, Draft, MOBILE_UA,
};
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::{Http, HttpRequest};
use std::collections::BTreeMap;
use std::sync::Arc;

const PAGE: &str = "https://m.v.qq.com/";

pub async fn fetch(http: Arc<dyn Http>) -> Result<Vec<Draft>> {
    let req = HttpRequest::get(PAGE).header("User-Agent", MOBILE_UA);
    let resp = check(http.send(req).await?, "腾讯视频")?;
    parse(&resp.body)
}

fn parse(html: &str) -> Result<Vec<Draft>> {
    let raw = pinia_body(html).ok_or_else(|| {
        AppError::new(
            ErrorKind::Provider,
            "腾讯视频页面里找不到内嵌数据（页面结构可能变了）",
        )
    })?;
    // 页面里的裸 `undefined` 是 JS 字面量，serde_json 不认；正文不会出现这个词。
    let cleaned = raw.replace("undefined", "null");
    let value: serde_json::Value = serde_json::from_str(&cleaned).map_err(|_| {
        AppError::new(ErrorKind::Provider, "腾讯视频内嵌数据无法解析")
    })?;
    let index = value
        .pointer("/global/channelModData/index")
        .or_else(|| value.pointer("/channel/channelModData/index"))
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            AppError::new(ErrorKind::Provider, "腾讯视频内嵌数据缺少频道模块")
        })?;

    let mut map: BTreeMap<String, Draft> = BTreeMap::new();
    for module in index {
        let Some(list) = module.get("list").and_then(|v| v.as_array()) else {
            continue;
        };
        for card in list {
            let Some(title) = non_empty(card.get("title").and_then(|v| v.as_str())) else {
                continue;
            };
            let Some(cid) = non_empty(card.get("cid").and_then(|v| v.as_str())) else {
                continue;
            };
            let timelong = non_empty(card.get("timelong").and_then(|v| v.as_str()));
            let publish = card.get("publishdate").and_then(|v| v.as_str()).unwrap_or("");
            let air_date = normalize_date(publish, 0).filter(|_| publish.matches('-').count() == 2);
            let kind = kind_of(card.get("type"));
            let draft = Draft {
                platform: "tencent",
                native_id: cid.clone(),
                kind,
                title: title.clone(),
                update_note: timelong.clone(),
                year: year_of(air_date.as_deref()),
                air_date,
                finished: timelong.as_deref().is_some_and(is_finished),
                url: Some(format!("https://v.qq.com/x/cover/{cid}.html")),
                episode_id: None,
                local_time: None,
                poster: non_empty(card.get("pic").and_then(|v| v.as_str())),
            };
            match map.get(&cid) {
                Some(old) if old.title.chars().count() <= draft.title.chars().count() => {}
                _ => {
                    map.insert(cid, draft);
                }
            }
        }
    }
    Ok(map.into_values().collect())
}

fn pinia_body(html: &str) -> Option<&str> {
    const MARKER: &str = "window.__PINIA__=";
    let start = html.find(MARKER)? + MARKER.len();
    let rest = &html[start..];
    let end = rest.find("</script>")?;
    Some(rest[..end].trim().trim_end_matches(';'))
}

/// `type` 是腾讯的频道号；106 是少儿动画，归入动漫。
fn kind_of(value: Option<&serde_json::Value>) -> &'static str {
    let text = match value {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        _ => return "other",
    };
    match text.as_str() {
        "1" => "movie",
        "2" => "tv",
        "3" | "106" => "anime",
        "10" => "variety",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"<html><script>
window.__PINIA__={"channel":{"channelModData":{"index":[
  {"list":[
    {"cid":"mzc002001w361jz","title":"CP三搭！孟子义李昀锐赴命定情缘","timelong":"全40集","publishdate":"2026-10-23","type":"2"},
    {"cid":"eazfh1m41s8e3bc","title":"僵尸叔叔","timelong":"","publishdate":"","type":"1"},
    {"cid":"","title":"无 cid 的广告","timelong":"全1集","type":"1"},
    {"cid":"mzc00200aaogpgh","title":"仙逆","timelong":"更新至168集","publishdate":"2023-09-25","type":"3"}
  ]},
  {"list":[
    {"cid":"mzc002001w361jz","title":"尚公主","timelong":"全40集","publishdate":"2026-10-23","type":"2"}
  ]}
]}}};
</script></html>"#;

    #[test]
    fn pinia_undefined_and_duplicate_cards_are_handled() {
        let body = FIXTURE.replace("\"channel\"", "\"global\":undefined,\"channel\"");
        let items = parse(&body).unwrap();
        assert_eq!(items.len(), 3, "无 cid 的卡片要丢掉");
        let show = items
            .iter()
            .find(|i| i.native_id == "mzc002001w361jz")
            .unwrap();
        assert_eq!(show.title, "尚公主", "重复卡片保留更短的剧名");
        assert_eq!(show.kind, "tv");
        assert_eq!(show.air_date.as_deref(), Some("2026-10-23"));
        assert_eq!(show.year, Some(2026));
        assert!(show.finished);
        assert_eq!(
            show.url.as_deref(),
            Some("https://v.qq.com/x/cover/mzc002001w361jz.html")
        );

        let anime = items
            .iter()
            .find(|i| i.native_id == "mzc00200aaogpgh")
            .unwrap();
        assert_eq!(anime.kind, "anime");
        assert!(!anime.finished);
    }

    #[test]
    fn missing_pinia_is_a_provider_error() {
        let err = parse("<html></html>").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Provider);
    }
}
