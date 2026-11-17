//! 豆包 `/chat/completion` 的 SSE 事件解码：把事件帧解析成文本增量。
//!
//! 豆包把同一条回复拆成多次事件推送：
//!   - `STREAM_MSG_NOTIFY`：增量追加；
//!   - `FULL_MSG_NOTIFY`：整块替换（可能是思考块的最终形态）；
//!   - `STREAM_CHUNK`：按 patch 追加/替换。
//! 这里按 `block_id` 记历史，把「这轮多出来的文本」算出来，避免把已输出的内容重复吐一遍。

use crate::app::error::{AppError, ErrorKind, Result};
use serde_json::Value;
use std::collections::BTreeMap;

/// 一次解码产出：正文增量或思考增量。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delta {
    Content(String),
    Reasoning(String),
}

impl Delta {
    pub fn text(&self) -> &str {
        match self {
            Delta::Content(t) | Delta::Reasoning(t) => t,
        }
    }
}

/// 一个 block 的累计状态（区分思考块与正文块）。
#[derive(Clone)]
struct Block {
    thinking: bool,
    text: String,
}

/// 增量式 SSE 解码器：喂入任意大小的文本块，吐出完整的文本增量。
/// 出错（事件不是合法 JSON、上游报错）时返回 AppError，由上层中断转发。
pub struct SseDecoder {
    buf: String,
    event: String,
    data: Vec<String>,
    history: BTreeMap<String, Block>,
    done: bool,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self {
            buf: String::new(),
            event: String::new(),
            data: Vec::new(),
            history: BTreeMap::new(),
            done: false,
        }
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    /// 喂入一块已解码的 UTF-8 文本，返回这轮产出的增量。
    pub fn feed(&mut self, chunk: &str) -> Result<Vec<Delta>> {
        self.buf.push_str(chunk);
        let mut out = Vec::new();
        while let Some(nl) = self.buf.find('\n') {
            let line = self.buf[..nl].trim_end_matches('\r').to_string();
            self.buf.drain(..=nl);
            if line.is_empty() {
                // 空行 = 一个事件结束
                if !self.data.is_empty() {
                    let event = std::mem::take(&mut self.event);
                    let data = std::mem::take(&mut self.data).join("\n");
                    self.handle_event(&event, &data, &mut out)?;
                }
                continue;
            }
            if line.starts_with(':') {
                continue; // 注释行（心跳）
            }
            let (field, value) = match line.find(':') {
                Some(i) => (
                    line[..i].to_string(),
                    line[i + 1..].trim_start_matches(' ').to_string(),
                ),
                None => (line.clone(), String::new()),
            };
            match field.as_str() {
                "event" => self.event = value,
                "data" => self.data.push(value),
                _ => {}
            }
        }
        Ok(out)
    }

    fn handle_event(&mut self, event: &str, data: &str, out: &mut Vec<Delta>) -> Result<()> {
        let v: Value = serde_json::from_str(data)
            .map_err(|_| AppError::new(ErrorKind::Provider, "豆包流式事件格式异常"))?;
        let code = v.get("code").and_then(as_i64).unwrap_or(0);
        // 事件名带 error / err、回包带 error、code 非 0、或带 decision（要验证码）都算失败
        let is_error = event.to_ascii_lowercase().contains("err")
            || v.get("error").is_some()
            || code != 0
            || v.get("decision").is_some()
            || v.get("extra").and_then(|e| e.get("decision")).is_some();
        if is_error {
            let msg = v
                .get("msg")
                .or_else(|| v.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("豆包返回错误");
            return Err(AppError::new(
                ErrorKind::Provider,
                format!("豆包对话出错：{msg}（code {code}）"),
            ));
        }
        match event {
            "SSE_REPLY_END" => self.done = true,
            "CHUNK_DELTA" => {
                if let Some(text) = v.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        out.push(Delta::Content(text.to_string()));
                    }
                }
            }
            "STREAM_MSG_NOTIFY" => {
                self.blocks(v.pointer("/content/content_block"), true, out)?;
            }
            "FULL_MSG_NOTIFY" => {
                if v.pointer("/message/user_type").and_then(as_i64) == Some(2) {
                    self.blocks(v.pointer("/message/content_block"), false, out)?;
                }
            }
            "STREAM_CHUNK" => {
                if let Some(ops) = v.get("patch_op").and_then(Value::as_array) {
                    for op in ops {
                        if op.get("patch_object").and_then(as_i64) == Some(1) {
                            let append = op.get("patch_type").and_then(as_i64) == Some(1);
                            self.blocks(op.pointer("/patch_value/content_block"), append, out)?;
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// 从 `content_block` 数组里抽出文本增量。`append=true` 表示整块是追加，
    /// `false` 表示整块替换——增量 = 去掉历史前缀后的部分。
    fn blocks(&mut self, list: Option<&Value>, append: bool, out: &mut Vec<Delta>) -> Result<()> {
        let Some(items) = list.and_then(Value::as_array) else {
            return Ok(());
        };
        for item in items {
            if item.get("block_type").and_then(as_i64) != Some(10000) {
                continue;
            }
            let Some(text) = item
                .pointer("/content/text_block/text")
                .and_then(Value::as_str)
            else {
                continue;
            };
            let key = item
                .get("block_id")
                .and_then(Value::as_str)
                .unwrap_or("default")
                .to_string();
            let info = item.pointer("/content/text_block");
            let icon = info
                .and_then(|i| i.get("icon_url"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let icon_dark = info
                .and_then(|i| i.get("icon_url_dark"))
                .and_then(Value::as_str)
                .unwrap_or("");
            // 「思考」的判定：图标带 deep_think，或（无图标时）沿用该 block 的历史判定
            let flags = [icon, icon_dark];
            let deep = flags.iter().any(|f| f.contains("deep_think"));
            let prior = self.history.get(&key);
            let thinking = deep
                || (flags.iter().all(|f| f.is_empty())
                    && prior.map(|p| p.thinking).unwrap_or(false));
            let before = if prior.map(|p| p.thinking).unwrap_or(false) == thinking {
                prior.map(|p| p.text.clone()).unwrap_or_default()
            } else {
                String::new()
            };
            let stored = if append {
                format!("{before}{text}")
            } else {
                text.to_string()
            };
            self.history.insert(
                key.clone(),
                Block {
                    thinking,
                    text: stored,
                },
            );
            // 替换块里去掉已输出过的前缀；拿不到前缀就整段算增量（宽口径，别误伤）
            let delta = if append {
                text.to_string()
            } else {
                text.strip_prefix(&before).unwrap_or(text).to_string()
            };
            if !delta.is_empty() {
                out.push(if thinking {
                    Delta::Reasoning(delta)
                } else {
                    Delta::Content(delta)
                });
            }
        }
        Ok(())
    }
}

fn as_i64(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(name: &str, data: serde_json::Value) -> String {
        format!(
            "event: {name}\ndata: {}\n\n",
            serde_json::to_string(&data).unwrap()
        )
    }

    #[test]
    fn chunk_delta_yields_content() {
        let mut d = SseDecoder::new();
        let out = d
            .feed(&event("CHUNK_DELTA", serde_json::json!({"text": "你好"})))
            .unwrap();
        assert_eq!(out, vec![Delta::Content("你好".into())]);
        assert!(!d.is_done());
    }

    #[test]
    fn reply_end_marks_done() {
        let mut d = SseDecoder::new();
        d.feed(&event("SSE_REPLY_END", serde_json::json!({})))
            .unwrap();
        assert!(d.is_done());
    }

    #[test]
    fn stream_notify_appends_and_full_notify_replaces() {
        let mut d = SseDecoder::new();
        // STREAM_MSG_NOTIFY 的 text 是「本轮新增的那段」：先「你」再「好」
        let notify = |text: &str| {
            event(
                "STREAM_MSG_NOTIFY",
                serde_json::json!({"content": {"content_block": [
                    {"block_type": 10000, "block_id": "b1",
                     "content": {"text_block": {"text": text, "icon_url": "", "icon_url_dark": "", "summary": ""}}}
                ]}}),
            )
        };
        d.feed(&notify("你")).unwrap();
        let out = d.feed(&notify("好")).unwrap();
        assert_eq!(
            out,
            vec![Delta::Content("好".into())],
            "追加块吐的是本轮增量"
        );

        // FULL_MSG_NOTIFY 的 text 是整块替换：去掉已输出的前缀后只剩新增部分
        let full = event(
            "FULL_MSG_NOTIFY",
            serde_json::json!({"message": {"user_type": 2, "content_block": [
                {"block_type": 10000, "block_id": "b1",
                 "content": {"text_block": {"text": "你好啊", "icon_url": "", "icon_url_dark": "", "summary": ""}}}
            ]}}),
        );
        let out = d.feed(&full).unwrap();
        assert_eq!(
            out,
            vec![Delta::Content("啊".into())],
            "替换块只吐多出来的部分"
        );
    }

    #[test]
    fn error_event_is_surfaced() {
        let mut d = SseDecoder::new();
        let err = d
            .feed(&event(
                "gateway-error",
                serde_json::json!({"code": 710012001, "msg": "登录已过期"}),
            ))
            .unwrap_err();
        assert!(err.message.contains("710012001"), "{}", err.message);
    }

    #[test]
    fn partial_chunks_are_buffered_across_feeds() {
        let mut d = SseDecoder::new();
        // 一个事件分两次喂进来，也要能拼成完整事件
        let full = event("CHUNK_DELTA", serde_json::json!({"text": "hello"}));
        let (a, b) = full.split_at(full.len() / 2);
        assert!(d.feed(a).unwrap().is_empty());
        let out = d.feed(b).unwrap();
        assert_eq!(out, vec![Delta::Content("hello".into())]);
    }
}
