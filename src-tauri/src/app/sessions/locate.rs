//! 定位会话的本地正文文件。「打开正文所在文件夹」用。
//!
//! 只对**文件落盘**的来源开放：WorkBuddy（jsonl）、CodeBuddy（messages 目录）、
//! Qoder 插件（conversation-history jsonl）。正文在 SQLite 库里的来源
//! （Qoder / ZCode）与只有云端占位的 Trae 没有可打开的文件，返回 `None`，
//! 界面把菜单项置灰——硬造一个「客户端根目录」只会让人摸不着头脑。

use crate::app::clients::Roots;
use crate::app::sessions::preview::Target;
use crate::app::sessions::sources::{qoder_plugin, wb};
use std::path::PathBuf;

/// 返回**包含正文的最深目录**（会话目录本身），打开后用户直接看到正文文件。
pub fn body_dir(roots: &Roots, target: &Target) -> Option<PathBuf> {
    match target.kind.as_str() {
        "workbuddy" => {
            let ep = wb::editions(roots)
                .into_iter()
                .find(|ep| ep.key == target.source_key)?;
            let files = wb::project_files(&ep.projects);
            let owned = files.get(&target.session_id)?.clone();
            let path = owned
                .into_iter()
                .find(|p| p.extension().map(|e| e == "jsonl").unwrap_or(false))?;
            Some(path.parent().map(PathBuf::from).unwrap_or(path))
        }
        "codebuddy" => {
            let history = roots
                .codebuddy()
                .join(&target.account)
                .join(&target.source_key)
                .join(&target.account)
                .join("history");
            // 工作区名不固定：与预览/删除同一套「逐个工作区试」的策略
            let entries = std::fs::read_dir(&history).ok()?;
            for entry in entries.flatten() {
                let dir = entry.path().join(&target.session_id).join("messages");
                if dir.is_dir() {
                    return Some(entry.path().join(&target.session_id));
                }
            }
            None
        }
        "qoder_plugin" => {
            let root = roots.qoder_plugin(target.source_key == "qoder_plugin_cn");
            qoder_plugin::session_files(&root)
                .into_iter()
                .find_map(|p| {
                    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned())?;
                    (stem == target.session_id).then(|| p.parent().map(PathBuf::from).unwrap_or(p))
                })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 正文在库里的来源与云端占位：没有可打开的文件，必须返回 None
    /// （返回 Some 会误导用户打开一个没有正文的目录）。
    #[test]
    fn db_sources_have_no_body_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let roots = Roots::at(dir.path(), dir.path(), dir.path());
        for kind in ["qoder", "zcode", "trae", "trae_plugin"] {
            let target = Target {
                kind: kind.into(),
                source_key: "qoder_cn".into(),
                account: String::new(),
                session_id: "s1".into(),
            };
            assert!(body_dir(&roots, &target).is_none(), "{kind} 不该有正文目录");
        }
    }
}
