//! 腾讯视频移动端首页：SSR 把频道模块塞进 `window.__PINIA__`，没有更轻的公开接口。
//!
//! 同一部剧会在多个模块里重复出现，且首屏模块的 `title` 常是运营文案
//! （例如「撞破亲哥出轨！」），后置模块才是剧名——去重时保留更短的那个。

use super::{
    check, is_finished, non_empty, normalize_date, pace, year_of, Draft, DESKTOP_UA, MOBILE_UA,
};
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::HttpRequest;
use std::collections::BTreeMap;

const PAGE: &str = "https://m.v.qq.com/";
const LIST_URL: &str = "https://v.qq.com/x/bu/pagesheet/list";
const LIST_CHANNELS: &[(&str, &str)] = &[
    ("movie", "movie"),
    ("tv", "tv"),
    ("cartoon", "anime"),
    ("variety", "variety"),
];

pub async fn fetch(ctx: super::AdapterContext) -> Result<Vec<Draft>> {
    let mut items = Vec::new();
    let req = HttpRequest::get(PAGE).header("User-Agent", MOBILE_UA);
    let resp = check(ctx.http.send(req).await?, "腾讯视频")?;
    items.append(&mut parse(&resp.body)?);

    for (channel, kind) in LIST_CHANNELS {
        for page in 0..ctx.max_pages.max(1) {
            let url = format!(
                "{LIST_URL}?_all=1&append=1&channel={channel}&listpage=2&offset={}&pagesize=30",
                page * 30
            );
            let req = HttpRequest::get(url).header("User-Agent", DESKTOP_UA);
            let resp = check(ctx.http.send(req).await?, "腾讯视频频道列表")?;
            let mut rows = parse_list(kind, &resp.body)?;
            if rows.is_empty() {
                break;
            }
            items.append(&mut rows);
            pace().await;
        }
    }
    Ok(merge_drafts(items))
}

fn merge_drafts(drafts: Vec<Draft>) -> Vec<Draft> {
    let mut map: BTreeMap<String, Draft> = BTreeMap::new();
    for draft in drafts {
        match map.get_mut(&draft.native_id) {
            Some(old) => {
                if old.summary.is_none() { old.summary.clone_from(&draft.summary); }
                if old.poster.is_none() { old.poster.clone_from(&draft.poster); }
                if old.year.is_none() { old.year = draft.year; }
                if old.air_date.is_none() { old.air_date.clone_from(&draft.air_date); }
                if old.rating.is_none() { old.rating = draft.rating; }
                for value in draft.cast {
                    if !old.cast.contains(&value) { old.cast.push(value); }
                }
                for value in draft.genres {
                    if !old.genres.contains(&value) { old.genres.push(value); }
                }
            }
            None => { map.insert(draft.native_id.clone(), draft); }
        }
    }
    map.into_values().collect()
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
                ..Default::default()
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

/// 频道列表页返回的 HTML 卡片：这里只取能稳定解析的标题、封面、演员与评分。
fn parse_list(kind: &'static str, html: &str) -> Result<Vec<Draft>> {
    let mut out = Vec::new();
    let marks: Vec<usize> = html.match_indices("list_item").map(|(i, _)| i).collect();
    for (index, mark) in marks.iter().enumerate() {
        let start = html[..*mark].rfind("<div").unwrap_or(*mark);
        let end = marks
            .get(index + 1)
            .and_then(|next| html[..*next].rfind("<div"))
            .unwrap_or(html.len());
        let card = &html[start..end];
        let Some(title) = non_empty(attr(card, "title")) else {
            continue;
        };
        let href = non_empty(attr(card, "href")).unwrap_or_default();
        let Some(native_id) = non_empty(attr(card, "data-float")).or_else(|| cover_id(&href)) else {
            continue;
        };
        let poster = non_empty(attr(card, "src")).map(|src| {
            if src.starts_with("//") { format!("https:{src}") } else { src }
        });
        let rating = tag_text(card, "figure_score\">")
            .and_then(|score| score.parse::<f64>().ok())
            .filter(|score| (0.0..=10.0).contains(score));
        out.push(Draft {
            platform: "tencent",
            native_id,
            kind,
            title,
            update_note: None,
            year: None,
            air_date: None,
            finished: false,
            url: Some(absolute(&href)),
            episode_id: None,
            local_time: None,
            poster,
            summary: None,
            cast: cast_from_card(card),
            genres: Vec::new(),
            regions: Vec::new(),
            rating,
        });
    }
    Ok(out)
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

fn cover_id(href: &str) -> Option<String> {
    let rest = href.split_once("/cover/")?.1;
    let id = rest.split(['.', '?', '#']).next()?.trim();
    non_empty(Some(id))
}

fn tag_text(card: &str, marker: &str) -> Option<String> {
    let start = card.find(marker)? + marker.len();
    let rest = &card[start..];
    non_empty(Some(&rest[..rest.find('<').unwrap_or(rest.len())]))
}

fn cast_from_card(card: &str) -> Vec<String> {
    let marker = "title=\"主演：";
    let Some(start) = card.find(marker) else {
        return Vec::new();
    };
    let rest = &card[start + marker.len()..];
    let end = rest.find('"').unwrap_or(rest.len());
    rest[..end]
        .split_whitespace()
        .filter_map(|name| non_empty(Some(name)))
        .collect()
}

fn absolute(href: &str) -> String {
    if href.starts_with("https://") || href.starts_with("http://") {
        href.to_string()
    } else if href.starts_with("//") {
        format!("https:{href}")
    } else {
        format!("https://v.qq.com{href}")
    }
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

    const LIST_FIXTURE: &str = r#"<html><body>
      <div class="list_item">
        <a class="figure" data-float="j3czmhisqin799r" href="https://v.qq.com/x/cover/j3czmhisqin799r.html" title="霸王别姬">
          <img class="figure_pic" src="//puui.qpic.cn/vcover.jpg/220">
          <div class="figure_score">9.6</div>
        </a>
        <div class="figure_detail">
          <a class="figure_title" title="霸王别姬">霸王别姬</a>
          <div class="figure_desc" title="主演：张国荣 张丰毅 巩俐">主演：张国荣 张丰毅 巩俐</div>
        </div>
      </div>
    </body></html>"#;

    #[test]
    fn channel_list_cards_are_parsed() {
        let items = parse_list("movie", LIST_FIXTURE).unwrap();
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.native_id, "j3czmhisqin799r");
        assert_eq!(item.title, "霸王别姬");
        assert_eq!(item.kind, "movie");
        assert_eq!(item.poster.as_deref(), Some("https://puui.qpic.cn/vcover.jpg/220"));
        assert_eq!(item.cast, vec!["张国荣", "张丰毅", "巩俐"]);
        assert_eq!(item.rating, Some(9.6));
    }
}
