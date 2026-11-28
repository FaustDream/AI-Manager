//! 两个日志窗格的数据源：
//! 1. 运行控制台 = 后端 stdout/stderr 实时捕获 + 启动器自身动作（环形缓冲，UI 持有）；
//! 2. 业务调用链日志 = `data/logs/app.jsonl` 增量 tail（结构化行渲染成人读文本）。

use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

/// 环形缓冲：超限丢最旧的。返回值表示是否发生裁剪（调用方需全量重绘窗格）。
pub struct Ring {
    lines: VecDeque<String>,
    cap: usize,
}

impl Ring {
    pub fn new(cap: usize) -> Self {
        Self {
            lines: VecDeque::with_capacity(cap),
            cap,
        }
    }

    pub fn push(&mut self, line: String) -> bool {
        let mut trimmed = false;
        if self.lines.len() >= self.cap {
            self.lines.pop_front();
            trimmed = true;
        }
        self.lines.push_back(line);
        trimmed
    }

    /// 全量重绘文本（仅在被裁剪时用；增量追加走 win::append，避免整窗重置）。
    pub fn render(&self) -> String {
        let mut text = String::new();
        for (i, line) in self.lines.iter().enumerate() {
            if i > 0 {
                text.push('\n');
            }
            text.push_str(line);
        }
        text
    }
}

/// 首次接入回看的字节数：足够铺满半个窗格，又不至于把 MB 级历史全部读进来。
const SEED_BYTES: u64 = 32 * 1024;
/// 单轮增量读取上限：日志风暴也不拖慢 UI，剩余的下一轮接着读。
const POLL_BUDGET: u64 = 256 * 1024;

/// app.jsonl 增量 tailer。只读新增字节；轮转/清空（文件变短）自动复位。
/// 偏移永远停在完整行边界：半行留在文件里，下一轮连同补全部分一起重读。
pub struct JsonlTail {
    path: PathBuf,
    offset: u64,
}

impl JsonlTail {
    pub fn open(path: PathBuf) -> Self {
        let mut tail = Self { path, offset: 0 };
        tail.rewind_to_tail();
        tail
    }

    fn rewind_to_tail(&mut self) {
        self.offset = 0;
        let Ok(meta) = std::fs::metadata(&self.path) else {
            return;
        };
        let len = meta.len();
        if len <= SEED_BYTES {
            return;
        }
        let Ok(mut file) = File::open(&self.path) else {
            return;
        };
        let start = len - SEED_BYTES;
        if file.seek(SeekFrom::Start(start)).is_err() {
            return;
        }
        let mut buf = Vec::new();
        let _ = file.take(SEED_BYTES).read_to_end(&mut buf);
        // 对齐到下一个行首，避免首行是半行
        let skip = buf
            .iter()
            .position(|&b| b == b'\n')
            .map(|p| p as u64 + 1)
            .unwrap_or(0);
        self.offset = start + skip;
    }

    /// 取自上次以来的新行（已渲染）。半行不渲染，补全后下一轮出完整行。
    pub fn poll(&mut self) -> Vec<String> {
        let Ok(meta) = std::fs::metadata(&self.path) else {
            return Vec::new();
        };
        let len = meta.len();
        if len < self.offset {
            // 日志被清空或轮转：先复位，内容下一轮再取
            self.rewind_to_tail();
            return Vec::new();
        }
        if len == self.offset {
            return Vec::new();
        }
        let Ok(mut file) = File::open(&self.path) else {
            return Vec::new();
        };
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let mut buf = Vec::new();
        let want = (len - self.offset).min(POLL_BUDGET);
        let _ = file.take(want).read_to_end(&mut buf);
        // 只消费完整行；尾部残行留在文件里，下一轮从原偏移重读
        let last_nl = buf
            .iter()
            .rposition(|&b| b == b'\n')
            .map(|p| p + 1)
            .unwrap_or(0);
        self.offset += last_nl as u64;
        String::from_utf8_lossy(&buf[..last_nl])
            .lines()
            .filter_map(render_line)
            .collect()
    }

    /// 首次接入：从文件尾部取最多 max 行。
    pub fn seed(&mut self, max: usize) -> Vec<String> {
        let all = self.poll();
        if all.len() > max {
            all[all.len() - max..].to_vec()
        } else {
            all
        }
    }
}

/// 一行结构化日志渲染成人读文本；解析失败丢弃（app.log 的人读行不会出现在这里）。
/// 字段语义见 src-tauri/src/app/log.rs 的 `Line`。
fn render_line(raw: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let ts = value.get("ts")?.as_str()?;
    let level = value
        .get("level")
        .and_then(|x| x.as_str())
        .unwrap_or("info");
    let module = value.get("module").and_then(|x| x.as_str()).unwrap_or("");
    let message = value.get("message").and_then(|x| x.as_str()).unwrap_or("");
    // ts 是 RFC3339：前 19 字节固定为 `YYYY-MM-DDTHH:MM:SS`，取时刻即可
    let time = ts.get(11..19).unwrap_or(ts);
    Some(format!("{time} [{level}] {module} · {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_trims_oldest_and_reports_trim() {
        let mut ring = Ring::new(3);
        assert!(!ring.push("a".into()));
        assert!(!ring.push("b".into()));
        assert!(!ring.push("c".into()));
        assert!(ring.push("d".into()), "第 4 行应触发裁剪");
        assert_eq!(ring.render(), "b\nc\nd");
    }

    #[test]
    fn render_line_formats_structured_row() {
        let raw = r#"{"ts":"2026-11-28T10:22:31.000+08:00","level":"warn","module":"action","message":"切换登录失败","outcome":"fail"}"#;
        let line = render_line(raw).unwrap();
        assert!(line.starts_with("10:22:31 [warn] action · "), "{line}");
        assert!(line.contains("切换登录失败"), "{line}");
    }

    #[test]
    fn render_line_tolerates_missing_fields_and_garbage() {
        assert!(render_line("不是 JSON").is_none());
        assert!(render_line("{\"ts\":\"2026-11-28T10:22:31+08:00\"}").is_some());
    }

    #[test]
    fn tail_polls_only_new_lines_and_survives_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.jsonl");
        std::fs::write(
            &path,
            "{\"ts\":\"2026-11-28T09:00:00+08:00\",\"level\":\"info\",\"module\":\"app\",\"message\":\"一\"}\n",
        )
        .unwrap();
        let mut tail = JsonlTail::open(path.clone());
        let seed = tail.seed(50);
        assert_eq!(seed.len(), 1);
        assert!(seed[0].contains("一"), "{seed:?}");

        // 追加两行 → 只出两行
        let appended = [
            r#"{"ts":"2026-11-28T09:01:00+08:00","level":"info","module":"app","message":"二"}"#,
            r#"{"ts":"2026-11-28T09:02:00+08:00","level":"info","module":"app","message":"三"}"#,
        ]
        .join("\n");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(format!("{appended}\n").as_bytes())
            .unwrap();
        let more = tail.poll();
        assert_eq!(more.len(), 2);
        assert!(tail.poll().is_empty(), "无新增时应为空");

        // 半行不出现：写半行后应为空，补全后下一次出完整行
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        use std::io::Write;
        write!(f, "{{\"ts\":\"2026-11-28T09:03:00+08:00\"").unwrap();
        assert!(tail.poll().is_empty(), "半行必须等完整才渲染");
        writeln!(
            f,
            ",\"level\":\"info\",\"module\":\"app\",\"message\":\"四\"}}"
        )
        .unwrap();
        let last = tail.poll();
        assert_eq!(last.len(), 1);
        assert!(last[0].contains("四"), "{last:?}");

        // 轮转（文件变短）：复位后能继续读到新文件内容
        std::fs::write(
            &path,
            "{\"ts\":\"2026-11-28T10:00:00+08:00\",\"level\":\"info\",\"module\":\"app\",\"message\":\"新文件\"}\n",
        )
        .unwrap();
        assert!(tail.poll().is_empty(), "轮转当轮先复位");
        let rotated = tail.poll();
        assert_eq!(rotated.len(), 1);
        assert!(rotated[0].contains("新文件"), "{rotated:?}");
    }

    #[test]
    fn tail_handles_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut tail = JsonlTail::open(dir.path().join("不存在.jsonl"));
        assert!(tail.poll().is_empty());
        assert!(tail.seed(10).is_empty());
    }
}
