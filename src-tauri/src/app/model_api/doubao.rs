//! 豆包模型接口的协议层：模型目录、聊天请求体、请求 URL 的组装。
//!
//! 协议口径来自参考实现（doubao-work-gateway / dsh-model 的 `lib/providers/doubao.js`），
//! 与「签到」那套（`providers::doubao`，aid=497858）是**两套 aid**：模型接口走
//! aid=1044603 的「豆包工作」上下文。这里只做纯函数组装，网络在 `model_api` 上层发。
//!
//! 关键接口：
//!   POST /alice/user/launch                       取 assistant_bot_id
//!   POST /alice/slot/action_bar_v3/brief_list     取模型目录（mode_id × model_item_key）
//!   POST /chat/completion                         发对话，回 SSE 流

use crate::app::providers::doubao::device_ids;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const ORIGIN: &str = "https://www.doubao.com";

/// 模型目录里的一条模型（`mode_id × model_item_key` 的一种组合）。
#[derive(Debug, Clone)]
pub struct ModelInfo {
    /// `model_item_key`：真正塞进请求体的模型键
    pub id: String,
    pub name: String,
    /// 模式 id（2=深度思考，3=普通，…）；空 = 无明确模式
    pub mode_id: String,
    /// 智能体模式（0=普通对话，1/2=agent 模式）
    pub agent_mode: i64,
    /// 深度思考档位（透传给 `need_deep_think` / `use_deep_think`）
    pub deep_think: i64,
    /// 模型附参（原样透传，可能带 provider_id 等）
    pub extra: Value,
}

/// 对话消息（OpenAI 侧规整后）。
#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// 从 launch 回包里取 assistant_bot_id。
pub fn parse_launch(reply: &Value) -> Option<String> {
    let data = reply.get("data").unwrap_or(reply);
    let bot = data
        .get("assistant_bot_id")
        .or_else(|| reply.get("assistant_bot_id"));
    bot.and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 从凭据里抽出「cookie + 稳定种子」。
/// cookie 供请求头，seed 派生 web_id（与签到/浏览器登录同一套，账号稳定）。
pub fn session_of(token: &str, seed: &str) -> (String, String) {
    let cookie = normalize_cookie(token);
    let seed = if seed.is_empty() {
        "doubao".to_string()
    } else {
        seed.to_string()
    };
    (cookie, seed)
}

/// cookie 头里翻某个键的值（fp 从 `s_v_web_id` / `fp` 兜底取）。
pub fn cookie_value(cookie: &str, key: &str) -> Option<String> {
    cookie.split(';').find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        (k.trim() == key)
            .then(|| v.trim().to_string())
            .filter(|s| !s.is_empty())
    })
}

/// 模型接口的请求 URL（aid=1044603 的「豆包工作」上下文）。
pub fn endpoint(route: &str, seed: &str, fp: &str) -> String {
    let (web_id, _) = device_ids(seed);
    let mut params: Vec<(&str, String)> = vec![
        ("aid", "1044603".into()),
        ("real_aid", "1044603".into()),
        ("device_platform", "web".into()),
        ("version_code", "20800".into()),
        ("samantha_web", "1".into()),
        ("web_platform", "desktop".into()),
        ("doubao_device_platform", "desktop".into()),
        ("client_platform", "pc_client".into()),
        ("pkg_type", "release_version".into()),
        ("runtime", "web".into()),
        ("runtime_version", "3.42.2".into()),
        ("pc_version", "2.33.5".into()),
        ("doubao_pc_version", "2.33.5".into()),
        ("use-olympus-account", "1".into()),
        ("language", "zh".into()),
        ("region", "CN".into()),
        ("sys_region", "CN".into()),
        ("web_id", web_id),
        ("device_id", String::new()),
    ];
    if !fp.is_empty() {
        params.push(("fp", fp.to_string()));
    }
    let qs = params
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    format!("{ORIGIN}{route}?{qs}")
}

/// 模型目录请求体。
pub fn catalog_body(bot: &str) -> Value {
    json!({ "language_code": "zh", "bot_id": bot })
}

/// 把模型目录回包解析成 `模型键 → ModelInfo`。
///
/// 模型键有两种形态：
///   - 一个 model_item_key 只有一种模式 → 直接是 `model_item_key`；
///   - 同名 model_item_key 有多个模式 → `mode_id::model_item_key`（或 `agent-N::…`）。
///     这样 `/v1/models` 暴露的 id 与 `/v1/chat/completions` 的 `model` 一一对应。
pub fn parse_catalog(reply: &Value) -> BTreeMap<String, ModelInfo> {
    let mut index: BTreeMap<String, BTreeMap<String, ModelInfo>> = BTreeMap::new();

    let entry_list = reply
        .pointer("/data/entry_list")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    for item in &entry_list {
        let Some(config) = item.get("active_switch_conf") else {
            continue;
        };
        if !config
            .get("use_menu_conf_v2")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            // 旧目录形态：直接列 item_list
            if let Some(items) = config
                .pointer("/menu_conf/item_list")
                .and_then(Value::as_array)
            {
                for entry in items {
                    record(&mut index, entry, &Value::Null);
                }
            }
            continue;
        }
        let v2 = config.get("menu_conf_v2").cloned().unwrap_or(Value::Null);
        let modes = v2
            .pointer("/mode_list/item_list")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for mode in &modes {
            if let Some(models) = mode
                .pointer("/model_list/item_list")
                .and_then(Value::as_array)
            {
                for model in models {
                    record(&mut index, model, mode);
                }
            }
        }
        // 顶层 model_list：按模式是否支持该模型挑一个「最佳模式」记录
        if let Some(models) = v2
            .pointer("/model_list/item_list")
            .and_then(Value::as_array)
        {
            for model in models {
                let id = model_key(model);
                if id.is_empty() || blocked_id(&id) {
                    continue;
                }
                let supported: Vec<&Value> = modes
                    .iter()
                    .filter(|m| {
                        let mode_id = str_opt(m.get("mode_id"));
                        let conv_ok = model
                            .get("conversation_mode_list")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().map(str_of).collect::<Vec<_>>())
                            .map(|list| list.is_empty() || list.contains(&mode_id))
                            .unwrap_or(true);
                        let support_ok = m
                            .get("support_models")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().map(model_key).collect::<Vec<_>>())
                            .map(|list| list.is_empty() || list.contains(&id))
                            .unwrap_or(true);
                        conv_ok && support_ok
                    })
                    .collect();
                let chosen = if supported.is_empty() {
                    if modes.is_empty() {
                        record(&mut index, model, &Value::Null);
                    }
                    continue;
                } else {
                    // rank：默认模式 > 新会话默认模式 > agent 模式 > 第一个
                    supported
                        .into_iter()
                        .max_by_key(|m| rank_of(m, model, &v2))
                        .unwrap()
                };
                record(&mut index, model, chosen);
            }
        }
    }

    // 展平：单模式直接暴露 id，多模式用 mode::id 区分
    let mut out = BTreeMap::new();
    for (id, group) in index {
        if group.len() == 1 {
            out.insert(id, group.into_values().next().unwrap());
            continue;
        }
        for (_, m) in group {
            let tag = if m.mode_id.is_empty() {
                format!("agent-{}", m.agent_mode)
            } else {
                m.mode_id.clone()
            };
            let key = format!("{}::{}", urlencode(&tag), urlencode(&id));
            out.insert(key, m);
        }
    }
    out
}

fn record(index: &mut BTreeMap<String, BTreeMap<String, ModelInfo>>, entry: &Value, mode: &Value) {
    let id = model_key(entry);
    if id.is_empty() || blocked_id(&id) {
        return;
    }
    let agent_mode = mode
        .get("agent_mode")
        .or_else(|| entry.get("agent_mode"))
        .and_then(as_i64)
        .unwrap_or(0);
    let mode_id = mode
        .get("mode_id")
        .or_else(|| entry.get("default_mode"))
        .map(str_of)
        .unwrap_or_default();
    let depth = entry
        .get("menu_type")
        .or_else(|| entry.get("item_id"))
        .and_then(as_i64)
        .unwrap_or(0);
    let extra = entry
        .get("model_extra_params")
        .and_then(|e| {
            if let Some(s) = e.as_str() {
                serde_json::from_str::<Value>(s).ok()
            } else {
                Some(e.clone())
            }
        })
        .filter(|e| e.is_object())
        .unwrap_or_else(|| json!({}));
    let name = entry
        .get("name")
        .map(str_of)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| id.clone());
    let deep_think = if mode_id == "2" && depth == 4 {
        3
    } else {
        depth
    };

    index
        .entry(id.clone())
        .or_default()
        .entry(format!("{mode_id}|{agent_mode}"))
        .or_insert(ModelInfo {
            id,
            name,
            mode_id,
            agent_mode,
            deep_think,
            extra,
        });
}

fn rank_of(m: &Value, model: &Value, v2: &Value) -> i64 {
    let mode_id = str_opt(m.get("mode_id"));
    if m.get("mode_id").is_none() {
        0
    } else if mode_id == str_opt(model.get("default_mode")) {
        3
    } else if mode_id
        == v2
            .pointer("/default_select_mode_map/new_chat/mode_id")
            .map(str_of)
            .unwrap_or_default()
    {
        2
    } else if m.get("agent_mode").and_then(as_i64) == Some(2) {
        1
    } else {
        0
    }
}

/// 组装一次对话请求体。协议字段逐一对齐参考实现，缺一可能被风控/网关拒。
pub fn chat_payload(bot: &str, model: &ModelInfo, messages: &[ChatMessage]) -> Value {
    let id = uuid();
    let at = crate::app::timeutil::now_ms();
    let mode = if model.mode_id.is_empty() {
        "3".to_string()
    } else {
        model.mode_id.clone()
    };
    // 单轮 user 直接当文本；多轮把整段对话 JSON 化（参考实现的同一口径）
    let message = if messages.len() == 1 && messages[0].role == "user" {
        messages[0].content.clone()
    } else {
        serde_json::to_string(messages).unwrap_or_default()
    };
    let block = json!({
        "block_type": 10000,
        "content": {
            "text_block": { "text": message, "icon_url": "", "icon_url_dark": "", "summary": "" },
            "pc_event_block": ""
        },
        "block_id": uuid(),
        "parent_id": "",
        "meta_info": [],
        "append_fields": []
    });
    let mut option = json!({
        "create_time_ms": at,
        "need_create_conversation": true,
        "conversation_init_option": { "need_ack_conversation": true },
        "need_deep_think": model.deep_think,
        "is_audio": false,
        "answer_with_suggest": false,
        "tts_switch": false,
        "is_regen": false,
        "is_replace": false,
        "unique_key": uuid(),
        "start_seq": 0,
        "sse_recv_event_options": { "support_chunk_delta": true },
        "recovery_option": {
            "is_recovery": false,
            "req_create_time_sec": at / 1000,
            "append_sse_event_scene": 0
        },
        "message_storage_type": 0
    });
    let mut ext = json!({
        "use_deep_think": model.deep_think.to_string(),
        "sub_conv_firstmet_type": "1",
        "commerce_credit_config_enable": "0"
    });
    if model.agent_mode != 0 || !model.mode_id.is_empty() {
        option["agent_mode"] = json!(model.agent_mode);
        option["model_config"] = json!({
            "model_item_key": model.id,
            "model_extra_params": model.extra
        });
        let mut agg = json!({
            "mode_id": mode,
            "model_item_key": model.id,
            "agent_mode": model.agent_mode.to_string()
        });
        if let Some(pid) = model.extra.get("provider_id").and_then(Value::as_str) {
            agg["provider_id"] = json!(pid);
        }
        option["aggregate_params"] = agg;
        ext["agent_mode"] = json!(model.agent_mode.to_string());
        if model.agent_mode == 1 || model.agent_mode == 2 {
            let general = json!({
                "action": 0,
                "thread_local_message_id": [id],
                "client_option": { "enable_sandbox": false, "os": "windows", "shared_folder_path": [] },
                "runtime_type": 2,
                "agent_task_param": { "runtime_type": 2 },
                "agent_task_param_change": {
                    "runtime_changed": true,
                    "device_changed": false,
                    "sandbox_auth_type_changed": false
                },
                "need_modify_conversation": false,
                "task_input_json": "{\"project_context\":{}}"
            });
            option["general_task_param"] = general.clone();
            ext["general_task_param"] = json!(serde_json::to_string(&general).unwrap_or_default());
            ext["is_finish"] = json!("1");
        }
    }
    let local = random_digits(16);
    json!({
        "client_meta": {
            "local_conversation_id": format!("local_{local}"),
            "conversation_id": "",
            "bot_id": bot,
            "last_section_id": "",
            "last_message_index": null
        },
        "messages": [ { "local_message_id": id, "content_block": [block], "message_status": 0 } ],
        "option": option,
        "user_context": [],
        "ext": ext
    })
}

// ------------------------------------------------------------------ 纯函数

fn model_key(entry: &Value) -> String {
    str_opt(entry.get("model_item_key").or_else(|| entry.get("item_id")))
}

fn blocked_id(id: &str) -> bool {
    matches!(id, "__proto__" | "prototype" | "constructor")
}

fn str_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn str_opt(v: Option<&Value>) -> String {
    v.map(str_of).unwrap_or_default()
}

fn as_i64(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))
}

fn normalize_cookie(raw: &str) -> String {
    let mut text = raw.trim();
    if text.len() >= 7 && text[..7].eq_ignore_ascii_case("cookie:") {
        text = text[7..].trim();
    }
    text.split(';')
        .map(|part| part.trim().replace(['\r', '\n', '\t'], " "))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

/// query 里的模型键只可能是 ASCII，做一次最小转义即可。
fn urlencode(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' | ':' | '/' | '+' | '%' => {
                c.to_string()
            }
            other => format!("%{:02X}", other as u32 & 0xff),
        })
        .collect()
}

pub(crate) fn uuid() -> String {
    let mut buf = [0u8; 16];
    let _ = getrandom::getrandom(&mut buf);
    let h: String = buf.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

fn random_digits(len: usize) -> String {
    let mut buf = vec![0u8; len];
    let _ = getrandom::getrandom(&mut buf);
    buf.iter().map(|b| char::from(b'0' + (b % 10))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_parses_bot_id() {
        let reply = json!({ "data": { "assistant_bot_id": "bot_123" } });
        assert_eq!(parse_launch(&reply).as_deref(), Some("bot_123"));
    }

    #[test]
    fn endpoint_uses_work_aid_and_stable_web_id() {
        let url = endpoint("/alice/user/launch", "seed1", "fp1");
        assert!(url.contains("aid=1044603"), "{url}");
        assert!(url.contains("device_platform=web"), "{url}");
        assert!(url.contains("web_id="), "{url}");
        assert!(url.contains("fp=fp1"), "{url}");
        // 同一种子 → 同一个 web_id
        assert_eq!(
            endpoint("/a", "seed1", "").split("web_id=").nth(1).unwrap(),
            endpoint("/b", "seed1", "").split("web_id=").nth(1).unwrap()
        );
    }

    fn catalog_json() -> Value {
        serde_json::from_str(
            r#"{
                "data": { "entry_list": [
                    { "active_switch_conf": { "use_menu_conf_v2": false,
                        "menu_conf": { "item_list": [
                            { "model_item_key": "m1", "name": "模型一", "default_mode": "3", "menu_type": 0 }
                        ] } } },
                    { "active_switch_conf": { "use_menu_conf_v2": true, "menu_conf_v2": {
                        "mode_list": { "item_list": [
                            { "mode_id": "2", "agent_mode": 0, "model_list": { "item_list": [
                                { "model_item_key": "deep", "name": "深度", "menu_type": 4 }
                            ] } },
                            { "mode_id": "3", "agent_mode": 0, "model_list": { "item_list": [
                                { "model_item_key": "deep", "name": "深度", "menu_type": 0 }
                            ] } }
                        ] }
                    } } }
                ] } }
            "#,
        )
        .unwrap()
    }

    #[test]
    fn catalog_single_mode_uses_plain_id() {
        let map = parse_catalog(&catalog_json());
        assert!(
            map.contains_key("m1"),
            "单模式直接暴露 id，实际键：{:?}",
            map.keys()
        );
        assert_eq!(map["m1"].name, "模型一");
        // 同名多模式 → mode::id 区分
        assert!(
            map.contains_key("2::deep"),
            "多模式要带 mode 前缀：{:?}",
            map.keys()
        );
        assert!(map.contains_key("3::deep"));
        assert_eq!(
            map["2::deep"].deep_think, 3,
            "mode=2 且 depth=4 的深度思考档位是 3"
        );
    }

    #[test]
    fn chat_payload_has_required_fields() {
        let model = ModelInfo {
            id: "m1".into(),
            name: "模型一".into(),
            mode_id: "3".into(),
            agent_mode: 0,
            deep_think: 0,
            extra: json!({}),
        };
        let msgs = vec![ChatMessage {
            role: "user".into(),
            content: "你好".into(),
        }];
        let payload = chat_payload("bot1", &model, &msgs);
        assert_eq!(payload["client_meta"]["bot_id"], "bot1");
        assert_eq!(
            payload["messages"][0]["content_block"][0]["block_type"],
            10000
        );
        assert!(payload["option"]["unique_key"].is_string());
        // 单轮 user 直接当文本
        assert_eq!(
            payload["messages"][0]["content_block"][0]["content"]["text_block"]["text"],
            "你好"
        );
    }

    #[test]
    fn session_of_normalizes_cookie_and_falls_back_seed() {
        assert_eq!(
            session_of("Cookie: a=1; b=2", ""),
            ("a=1; b=2".to_string(), "doubao".to_string())
        );
        assert_eq!(session_of("a=1", "s1").1, "s1");
    }
}
