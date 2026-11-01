//! 各客户端会话来源的定位与扫描。
//!
//! **指纹便宜、扫描贵**：`sources()` 只 stat 元数据给出每个来源的指纹，
//! 上层比对指纹决定要不要真去读内容（`scan_one`）。换号会改变归属判定，
//! 所以 WorkBuddy 的指纹里带上当前 uid。

pub mod codebuddy;
pub mod others;
pub mod wb;

use crate::app::clients::Roots;
use crate::app::error::Result;
use crate::app::sessions::store::RawRow;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;


/// 消息条数与体积的缓存：**按会话**而不是按来源。
///
/// 客户端一直在写自己的库，来源级指纹几乎每次都会变；但绝大多数会话的正文文件
/// 没动过。按「文件元数据」缓存计数，重扫一个来源就从「读几十 MB 正文」变成
/// 「几十次 stat」——这是 v1 里最有效的一条优化，不能省。
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MsgCache {
    #[serde(default)]
    pub entries: HashMap<String, Cached>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cached {
    pub fp: String,
    pub msgs: i64,
    pub size: i64,
}

impl MsgCache {
    pub fn hit(&self, key: &str, fp: &str) -> Option<&Cached> {
        self.entries.get(key).filter(|item| item.fp == fp)
    }

    pub fn put(&mut self, key: &str, fp: &str, msgs: i64, size: i64) {
        self.entries.insert(
            key.to_string(),
            Cached {
                fp: fp.to_string(),
                msgs,
                size,
            },
        );
    }

}


/// 一个来源的身份与指纹。`present=false` 表示这个客户端没装/没登录过。
#[derive(Debug, Clone)]
pub struct SourceInfo {
    pub key: String,
    pub label: String,
    pub fp: String,
    pub present: bool,
}

/// 便宜的来源清单：只碰元数据，不读正文。
pub fn sources(roots: &Roots) -> Vec<SourceInfo> {
    let mut out = Vec::new();
    for ep in wb::editions(roots) {
        let fp = wb::fingerprint(&ep);
        out.push(SourceInfo {
            key: ep.key.to_string(),
            label: ep.label.to_string(),
            fp,
            present: true,
        });
    }
    for src in codebuddy::roots(roots) {
        out.push(SourceInfo {
            key: src.platform.to_string(),
            label: src.label.to_string(),
            fp: src.fp.clone(),
            present: src.dirs.iter().any(|(_, dir)| dir.is_dir()),
        });
    }
    for info in others::sources(roots) {
        out.push(info);
    }
    out
}

/// 真去扫一个来源。`key` 必须来自 `sources()`。
pub fn scan_one(
    roots: &Roots,
    key: &str,
    cache: &mut MsgCache,
    errors: &mut Vec<String>,
) -> Result<Vec<RawRow>> {
    if let Some(ep) = wb::editions(roots).into_iter().find(|ep| ep.key == key) {
        return Ok(wb::scan(&ep, cache, errors));
    }
    if let Some(src) = codebuddy::roots(roots)
        .into_iter()
        .find(|s| s.platform == key)
    {
        return Ok(codebuddy::scan(&src, errors));
    }
    Ok(others::scan_one(roots, key, errors))
}
