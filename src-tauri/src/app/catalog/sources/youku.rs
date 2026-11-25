//! 优酷电视频道页：没有公开的更新接口，只能解析卡片上的 `aria-label`。
//!
//! 页面不给日期与年份，因此这里产出的条目只进「年更库-年份未知」和「已完结」，
//! 不硬编造日期。属性顺序不固定，所以按标签整体取属性而不是匹配固定模板。

use super::{check, Draft, DESKTOP_UA};
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::{Http, HttpRequest};
use std::collections::BTreeMap;
use std::sync::Arc;

const PAGE: &str = "https://tv.youku.com/";

const PREFIXES: &[&str] = &[
    "独播剧", "独播", "会员", "VIP", "vip", "限免中", "限免", "预告", "广告", "热播", "自制",
    "首发", "免费", "付费", "新剧", "片花", "花絮",
];

pub async fn fetch(http: Arc<dyn Http>) -> Result<Vec<Draft>> {
    let req = HttpRequest::get(PAGE).header("User-Agent", DESKTOP_UA);
    let resp = check(http.send(req).await?, "优酷")?;
    parse(&resp.body)
}

fn parse(html: &str) -> Result<Vec<Draft>> {
    let mut map: BTreeMap<String, Draft> = BTreeMap::new();
    let mut offset = 0;
    while let Some(found) = html[offset..].find("<a") {
        let start = offset + found;
        let Some(close) = html[start..].find('>') else {
            break;
        };
        let tag = &html[start..start + close + 1];
        offset = start + close + 1;
        let boundary = tag.as_bytes().get(2).copied().unwrap_or(b'>');
        if !(boundary == b'>' || boundary.is_ascii_whitespace()) {
            continue;
        }
        let (Some(label), Some(href)) = (attr(tag, "aria-label"), attr(tag, "href")) else {
            continue;
        };
        if href.contains("/live/") {
            continue;
        }
        let Some((title, note)) = clean_label(label) else {
            continue;
        };
        let href = href.replace("&amp;", "&");
        let native_id = show_id(&href).unwrap_or_else(|| href.clone());
        let draft = Draft {
            platform: "youku",
            native_id: native_id.clone(),
            kind: "tv",
            title,
            update_note: note.clone(),
            year: None,
            air_date: None,
            finished: note.as_deref().is_some_and(super::is_finished),
            url: Some(absolute(&href)),
            episode_id: None,
            local_time: None,
            poster: None,
        };
        match map.get(&native_id) {
            Some(old) if old.update_note.is_some() || draft.update_note.is_none() => {}
            _ => {
                map.insert(native_id, draft);
            }
        }
    }
    if map.is_empty() {
        return Err(AppError::new(
            ErrorKind::Provider,
            "优酷页面没有可解析的剧集卡片",
        ));
    }
    Ok(map.into_values().collect())
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

fn absolute(href: &str) -> String {
    if href.starts_with("http") {
        href.to_string()
    } else if href.starts_with("//") {
        format!("https:{href}")
    } else {
        format!("https://tv.youku.com{href}")
    }
}

/// 从 `...?s=xxx` / `...&s=xxx` 取剧集 id；取不到就退回整条链接。
fn show_id(href: &str) -> Option<String> {
    let tail = href
        .split_once("?s=")
        .or_else(|| href.split_once("&s="))?
        .1;
    let id = tail.split(['&', '#']).next()?.trim();
    (!id.is_empty()).then(|| id.to_string())
}

fn clean_label(label: &str) -> Option<(String, Option<String>)> {
    let mut text = label.trim().to_string();
    let mut note = None;
    if let Some((start, end, status)) = extract_status(&text) {
        note = Some(status);
        text.replace_range(start..end, " ");
    }
    loop {
        let trimmed = text.trim_start();
        let mut rest = None;
        for prefix in PREFIXES {
            if let Some(after) = trimmed.strip_prefix(prefix) {
                rest = Some(after.to_string());
                break;
            }
        }
        match rest {
            Some(after) => text = after,
            None => break,
        }
    }
    let title = text
        .split_whitespace()
        .filter(|token| !is_noise(token))
        .collect::<Vec<_>>()
        .join(" ");
    (!title.is_empty()).then_some((title, note))
}

fn is_noise(token: &str) -> bool {
    if token == "上线" || token == "更新" || token == "更新至" {
        return true;
    }
    let core = token.strip_suffix('集').unwrap_or(token);
    !core.is_empty() && core.chars().all(|c| c.is_ascii_digit() || matches!(c, '-' | '/' | '.'))
}

/// 状态有三种真实写法：`更新至17集`、`30集全`、`全40集`。
fn extract_status(text: &str) -> Option<(usize, usize, String)> {
    let mut hits: Vec<(usize, usize, String)> = Vec::new();
    for (index, _) in text.match_indices("集全") {
        let Some((digits_start, _)) = text[..index]
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_ascii_digit())
            .last()
        else {
            continue;
        };
        hits.push((
            digits_start,
            index + "集全".len(),
            text[digits_start..index + "集全".len()].to_string(),
        ));
    }
    for (index, _) in text.match_indices('全') {
        let rest = &text[index + '全'.len_utf8()..];
        let digits_len = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits_len == 0 {
            continue;
        }
        let digits_end = index + '全'.len_utf8() + digits_len;
        if text[digits_end..].starts_with('集') {
            hits.push((index, digits_end + '集'.len_utf8(), text[index..digits_end + '集'.len_utf8()].to_string()));
        }
    }
    for (index, _) in text.match_indices("更新至") {
        let rest = &text[index + "更新至".len()..];
        let rest = rest.strip_prefix('第').unwrap_or(rest);
        let digits_len = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits_len == 0 {
            continue;
        }
        let digits_end = text.len() - rest.len() + digits_len;
        if text[digits_end..].starts_with('集') {
            hits.push((
                index,
                digits_end + '集'.len_utf8(),
                text[index..digits_end + '集'.len_utf8()].to_string(),
            ));
        }
    }
    hits.into_iter().min_by_key(|(start, _, _)| *start)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"<html><body>
      <a href="//v.youku.com/v_show/id_XNjQ1.html?spm=a2h&s=abc123" aria-label="独播 30集全 嫁金钗" class="x"></a>
      <a aria-label="独播剧 更新至第17集 漫长的季节" href="https://v.youku.com/v_show/id_XNjQ2.html?s=def456"></a>
      <a aria-label="会员 全40集 甄嬛传" href="/v_show/id_XNjQ3.html?s=ghi789"></a>
      <a aria-label="限免中 限免04-05集 无声的回响" href="https://v.youku.com/v_show/id_XNjQ4.html?s=jkl012"></a>
      <a aria-label="直播中的节目" href="https://v.youku.com/live/abc"></a>
    </body></html>"#;

    #[test]
    fn labels_are_cleaned_and_statuses_extracted() {
        let items = parse(FIXTURE).unwrap();
        assert_eq!(items.len(), 4, "直播卡片要丢掉");
        let find = |id: &str| items.iter().find(|i| i.native_id == id).unwrap();

        let done = find("abc123");
        assert_eq!(done.title, "嫁金钗");
        assert_eq!(done.update_note.as_deref(), Some("30集全"));
        assert!(done.finished);
        assert_eq!(
            done.url.as_deref(),
            Some("https://v.youku.com/v_show/id_XNjQ1.html?spm=a2h&s=abc123")
        );

        let ongoing = find("def456");
        assert_eq!(ongoing.title, "漫长的季节");
        assert_eq!(ongoing.update_note.as_deref(), Some("更新至第17集"));
        assert!(!ongoing.finished);
        assert_eq!(ongoing.year, None);
        assert_eq!(ongoing.air_date, None);

        let full = find("ghi789");
        assert_eq!(full.title, "甄嬛传");
        assert!(full.finished);
        assert!(full.url.as_deref().unwrap().starts_with("https://tv.youku.com/"));

        let free = find("jkl012");
        assert_eq!(free.title, "无声的回响");
        assert_eq!(free.update_note, None);
        assert!(!free.finished);
    }

    #[test]
    fn html_without_cards_is_reported() {
        let err = parse("<html><body><p>无卡片</p></body></html>").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Provider);
    }
}
