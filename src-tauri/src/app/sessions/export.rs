//! 会话导出：把正文写成 Markdown / JSON 落到 `data/exports/`。
//!
//! 导出走**全量模式**（`preview::load_for(full=true)`）：不再受「头尾 100 条」的
//! 预览口径限制，但仍有消息硬顶（5000 条）与单条 3000 字截断——一条失控的会话
//! 不该把内存拖垮。输出文件名带来源与日期，固定写 exports 目录（不弹文件对话框，
//! 与「打开数据目录」同一套零依赖套路），写完由界面提示路径。

use crate::app::clients::Roots;
use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::sessions::preview::{self, Target};
use crate::app::sessions::store::SqliteSessions;
use chrono::Local;
use serde::Serialize;
use std::io::Write;

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct ExportReport {
    /// 写出的第一个文件路径（批量=合并成一个文件）
    pub path: String,
    /// 导出的会话条数
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub sessions: usize,
    /// 导出的消息总条数
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub messages: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExportFormat {
    Markdown,
    Json,
}

impl ExportFormat {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "markdown" | "md" => Ok(Self::Markdown),
            "json" => Ok(Self::Json),
            other => Err(AppError::new(
                ErrorKind::Invalid,
                format!("不认识的导出格式：{other}（可选 markdown / json）"),
            )),
        }
    }

    fn ext(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Json => "json",
        }
    }
}

/// 导出入口。批量导出合并为一个文件（按会话分节），方便一次带走一批。
pub fn export(
    roots: &Roots,
    db: &Db,
    keys: &[String],
    format: ExportFormat,
) -> Result<ExportReport> {
    if keys.is_empty() {
        return Err(AppError::new(ErrorKind::Invalid, "没有选中会话"));
    }

    // 标题用索引表里的**生效标题**（本地命名优先），文件名与正文标题都靠它
    let store = SqliteSessions::new(db);
    let mut targets = Vec::new();
    for key in keys {
        targets.push((key.clone(), preview::parse_key(key)?));
    }
    let titles = store.titles_of(&targets).unwrap_or_default();

    let mut sessions = Vec::new();
    for (key, target) in &targets {
        let loaded = preview::load_for(roots, target, true);
        let title = titles
            .get(key)
            .cloned()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| format!("会话 {}", target.session_id));
        sessions.push((title, target.clone(), loaded));
    }

    let dir = paths_exports_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| AppError::new(ErrorKind::Io, format!("创建导出目录失败：{e}")))?;
    let stamp = Local::now().format("%Y%m%d-%H%M%S");
    let name = if sessions.len() == 1 {
        file_stem(&sessions[0].0)
    } else {
        format!("sessions-{}", sessions.len())
    };
    let path = dir.join(format!("{name}-{stamp}.{}", format.ext()));

    let messages = match format {
        ExportFormat::Markdown => write_markdown(&path, &sessions)?,
        ExportFormat::Json => write_json(&path, &sessions)?,
    };

    crate::app::log::log(
        crate::app::log::Level::Info,
        "sessions",
        format!(
            "导出 {} 条会话（{messages} 条消息）到 {}",
            sessions.len(),
            path.display()
        ),
    );
    Ok(ExportReport {
        path: path.to_string_lossy().into_owned(),
        sessions: sessions.len(),
        messages,
    })
}

/// 导出目录：`data/exports/`。
pub fn paths_exports_dir() -> std::path::PathBuf {
    crate::app::paths::data_dir().join("exports")
}

fn write_markdown(
    path: &std::path::Path,
    sessions: &[(String, Target, preview::Preview)],
) -> Result<usize> {
    let mut out = std::io::BufWriter::new(
        std::fs::File::create(path)
            .map_err(|e| AppError::new(ErrorKind::Io, format!("创建导出文件失败：{e}")))?,
    );
    let mut messages = 0usize;
    for (index, (title, target, loaded)) in sessions.iter().enumerate() {
        if index > 0 {
            writeln!(out, "\n\n---\n").map_err(io_err)?;
        }
        writeln!(out, "# {title}").map_err(io_err)?;
        writeln!(
            out,
            "\n> 来源：`{}` · 会话 id：`{}` · 共 {} 条消息{}",
            target.source_key,
            target.session_id,
            loaded.total,
            if loaded.truncated {
                "（超出上限已截断）"
            } else {
                ""
            }
        )
        .map_err(io_err)?;
        for message in &loaded.messages {
            if message.role == "gap" {
                writeln!(out, "\n*（{}）*", message.text).map_err(io_err)?;
                continue;
            }
            let who = if message.role == "user" {
                "用户"
            } else {
                "AI"
            };
            writeln!(out, "\n**{who}**：\n\n{}", message.text).map_err(io_err)?;
            messages += 1;
        }
    }
    out.flush().map_err(io_err)?;
    Ok(messages)
}

fn write_json(
    path: &std::path::Path,
    sessions: &[(String, Target, preview::Preview)],
) -> Result<usize> {
    // 机读格式：结构与 Preview 一致，批量=数组；gap 占位条不输出
    let mut value = Vec::with_capacity(sessions.len());
    let mut messages = 0usize;
    for (title, target, loaded) in sessions {
        let list: Vec<&preview::Message> =
            loaded.messages.iter().filter(|m| m.role != "gap").collect();
        messages += list.len();
        value.push(serde_json::json!({
            "title": title,
            "source_key": target.source_key,
            "session_id": target.session_id,
            "total": loaded.total,
            "truncated": loaded.truncated,
            "messages": list,
        }));
    }
    let text = serde_json::to_string_pretty(&value)
        .map_err(|e| AppError::new(ErrorKind::Io, format!("序列化失败：{e}")))?;
    crate::app::fsx::write_atomic(path, text.as_bytes())?;
    Ok(messages)
}

/// 文件名只留「人能看懂」的部分：来源名 + 标题前 40 字，剔掉路径与非法字符。
fn file_stem(title: &str) -> String {
    let clean: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => ' ',
            c => c,
        })
        .collect();
    let clean = clean.trim();
    if clean.is_empty() {
        "sessions".to_string()
    } else {
        clean.chars().take(40).collect()
    }
}

fn io_err(e: std::io::Error) -> AppError {
    AppError::new(ErrorKind::Io, format!("写导出文件失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_format() {
        assert!(matches!(
            ExportFormat::parse("markdown"),
            Ok(ExportFormat::Markdown)
        ));
        assert!(matches!(
            ExportFormat::parse("md"),
            Ok(ExportFormat::Markdown)
        ));
        assert!(matches!(
            ExportFormat::parse("json"),
            Ok(ExportFormat::Json)
        ));
        assert!(ExportFormat::parse("pdf").is_err());
    }

    #[test]
    fn sanitizes_file_stem() {
        assert_eq!(file_stem("a/b\\c:d*e"), "a b c d e");
        assert_eq!(file_stem(""), "sessions");
        assert_eq!(file_stem("短标题"), "短标题");
    }
}
