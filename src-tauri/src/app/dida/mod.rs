//! 滴答清单（Dida365 国内版）开放接口：口令存取、清单读取与任务写入。
//!
//! 口令是用户在网页版生成的长期凭据：只经 DPAPI 加密后落 kv，不写配置文件、
//! 不进日志与返回体。界面只拿得到「已配置 / 未配置」。
//!
//! 网络 client 由壳层注入（与签到共用同一个），这里不自行构造 reqwest。

use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::{Http, HttpRequest, HttpResponse};
use crate::app::runtime::blocking;
use crate::app::secret;
use serde::Serialize;
use std::sync::Arc;

const BASE: &str = "https://api.dida365.com/open/v1";
const KV_TOKEN: &str = "dida.token";

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DidaProject {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    pub closed: bool,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DidaTask {
    pub id: String,
    pub title: String,
    pub project_id: String,
    pub content: Option<String>,
    pub due_date: Option<String>,
    /// 全天任务：滴答只记日期，界面不显示时刻。
    pub is_all_day: bool,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub priority: u8,
    /// 滴答原样状态：0 未完成、2 已完成。界面据此决定展示与可用操作。
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub status: u8,
    pub completed_time: Option<String>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DidaState {
    pub configured: bool,
    pub projects: Vec<DidaProject>,
    /// 已配置但读不到清单时的可读原因；口令本身永远不出现在这里。
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct Service {
    db: Arc<Db>,
    http: Arc<dyn Http>,
}

impl Service {
    pub fn new(db: Arc<Db>, http: Arc<dyn Http>) -> Self {
        Self { db, http }
    }

    /// 界面首屏状态。拉清单失败不算命令失败：已配置的事实要保留，
    /// 原因放进 `error` 让页面能继续提供「清除口令」。
    pub async fn state(&self) -> DidaState {
        let token = match self.stored_token().await {
            Ok(Some(token)) => token,
            Ok(None) => return DidaState::default(),
            Err(e) => {
                return DidaState {
                    configured: true,
                    projects: Vec::new(),
                    error: Some(e.message),
                }
            }
        };
        match self.fetch_projects(&token).await {
            Ok(projects) => DidaState {
                configured: true,
                projects,
                error: None,
            },
            Err(e) => DidaState {
                configured: true,
                projects: Vec::new(),
                error: Some(e.message),
            },
        }
    }

    /// 先验口令再落库：无效口令不该占据「已配置」位，否则用户要先清除才能重填。
    pub async fn set_token(&self, token: &str) -> Result<DidaState> {
        let token = token.trim();
        if token.is_empty() {
            return Err(AppError::new(ErrorKind::Invalid, "API 口令不能为空"));
        }
        let projects = self.fetch_projects(token).await?;
        self.store_token(token).await?;
        Ok(DidaState {
            configured: true,
            projects,
            error: None,
        })
    }

    pub async fn clear_token(&self) -> Result<DidaState> {
        let db = self.db.clone();
        blocking(move || db.kv_delete(KV_TOKEN)).await??;
        Ok(DidaState::default())
    }

    // 同 update_task：参数即平台字段，不另造承载结构。
    #[allow(clippy::too_many_arguments)]
    pub async fn create_task(
        &self,
        title: &str,
        project_id: &str,
        content: Option<&str>,
        due_date: Option<&str>,
        priority: Option<u8>,
        is_all_day: bool,
    ) -> Result<DidaTask> {
        let title = title.trim();
        if title.is_empty() {
            return Err(AppError::new(ErrorKind::Invalid, "任务标题不能为空"));
        }
        let project_id = project_id.trim();
        if project_id.is_empty() {
            return Err(AppError::new(ErrorKind::Invalid, "请选择清单"));
        }
        if let Some(priority) = priority {
            if !matches!(priority, 0 | 1 | 3 | 5) {
                return Err(AppError::new(ErrorKind::Invalid, "优先级只能是 0/1/3/5"));
            }
        }
        let token = self
            .stored_token()
            .await?
            .ok_or_else(|| AppError::new(ErrorKind::Invalid, "请先填写滴答清单 API 口令"))?;

        let mut body = serde_json::json!({ "title": title, "projectId": project_id });
        if let Some(content) = content.map(str::trim).filter(|s| !s.is_empty()) {
            body["content"] = serde_json::Value::String(content.to_string());
        }
        if let Some(due_date) = due_date.map(str::trim).filter(|s| !s.is_empty()) {
            body["dueDate"] = serde_json::Value::String(due_date.to_string());
        }
        if let Some(priority) = priority {
            body["priority"] = serde_json::json!(priority);
        }
        body["isAllDay"] = serde_json::json!(is_all_day);

        let req = HttpRequest::post(format!("{BASE}/task"))
            .header("Authorization", format!("Bearer {token}"))
            .json_body(&body);
        let resp = check(self.http.send(req).await?, "创建任务")?;
        let value = resp.json()?;
        parse_task(&value)
            .ok_or_else(|| AppError::new(ErrorKind::Provider, "滴答清单创建任务的回执缺少任务 id"))
    }

    /// 单个清单的任务：开放平台没有全局任务列表，只能按清单取。
    /// 已完成任务在另一个端点上，按需合并，避免每次刷新都多打一发请求。
    /// 单个清单的未完成任务：开放平台没有全局任务列表，只能按清单取。
    /// 国内版实测不提供已完成任务端点（恒 404），已完成任务只在滴答客户端里看。
    pub async fn list_tasks(&self, project_id: &str) -> Result<Vec<DidaTask>> {
        let project_id = project_id.trim();
        if project_id.is_empty() {
            return Err(AppError::new(ErrorKind::Invalid, "请选择清单"));
        }
        let token = self
            .stored_token()
            .await?
            .ok_or_else(|| AppError::new(ErrorKind::Invalid, "请先填写滴答清单 API 口令"))?;

        let req = HttpRequest::get(format!("{BASE}/project/{project_id}/data"))
            .header("Authorization", format!("Bearer {token}"));
        let resp = check(self.http.send(req).await?, "读取任务")?;
        let value = resp.json()?;
        let items = value
            .get("tasks")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                AppError::new(
                    ErrorKind::Provider,
                    "滴答清单返回的任务列表格式不符（预期 tasks 数组）",
                )
            })?;
        Ok(items.iter().filter_map(parse_task).collect())
    }

    /// 更新走全量字段：标题与清单必带，内容/截止传 null 表示清除。
    // 字段即滴答开放平台任务对象的子集，拆结构体只是把参数换个地方排；限宽靠 allow。
    #[allow(clippy::too_many_arguments)]
    pub async fn update_task(
        &self,
        task_id: &str,
        project_id: &str,
        title: &str,
        content: Option<&str>,
        due_date: Option<&str>,
        priority: Option<u8>,
        is_all_day: bool,
    ) -> Result<DidaTask> {
        let task_id = task_id.trim();
        if task_id.is_empty() {
            return Err(AppError::new(ErrorKind::Invalid, "任务不存在或已删除"));
        }
        let title = title.trim();
        if title.is_empty() {
            return Err(AppError::new(ErrorKind::Invalid, "任务标题不能为空"));
        }
        let project_id = project_id.trim();
        if project_id.is_empty() {
            return Err(AppError::new(ErrorKind::Invalid, "请选择清单"));
        }
        if let Some(priority) = priority {
            if !matches!(priority, 0 | 1 | 3 | 5) {
                return Err(AppError::new(ErrorKind::Invalid, "优先级只能是 0/1/3/5"));
            }
        }
        let token = self
            .stored_token()
            .await?
            .ok_or_else(|| AppError::new(ErrorKind::Invalid, "请先填写滴答清单 API 口令"))?;

        let mut body = serde_json::json!({ "title": title, "projectId": project_id });
        body["content"] = optional_text(content);
        body["dueDate"] = optional_text(due_date);
        if let Some(priority) = priority {
            body["priority"] = serde_json::json!(priority);
        }
        body["isAllDay"] = serde_json::json!(is_all_day);

        let url = format!("{BASE}/task/{task_id}");
        let req = HttpRequest::put(url.clone())
            .header("Authorization", format!("Bearer {token}"))
            .json_body(&body);
        let resp = self.http.send(req).await?;
        // 开放平台文档对更新动词表述不一（PUT / POST 两种写法都存在），
        // 遇 404/405 用同一体再试一次；body 幂等，重放无副作用。
        let resp = if resp.status == 404 || resp.status == 405 {
            let req = HttpRequest::post(url)
                .header("Authorization", format!("Bearer {token}"))
                .json_body(&body);
            self.http.send(req).await?
        } else {
            resp
        };
        let resp = check(resp, "更新任务")?;
        let value = resp.json()?;
        parse_task(&value)
            .ok_or_else(|| AppError::new(ErrorKind::Provider, "滴答清单更新任务的回执缺少任务 id"))
    }

    pub async fn complete_task(&self, project_id: &str, task_id: &str) -> Result<()> {
        let (project_id, task_id) = validate_pair(project_id, task_id)?;
        let token = self
            .stored_token()
            .await?
            .ok_or_else(|| AppError::new(ErrorKind::Invalid, "请先填写滴答清单 API 口令"))?;
        let req = HttpRequest::post(format!(
            "{BASE}/project/{project_id}/task/{task_id}/complete"
        ))
        .header("Authorization", format!("Bearer {token}"));
        check(self.http.send(req).await?, "完成任务")?;
        Ok(())
    }

    pub async fn delete_task(&self, project_id: &str, task_id: &str) -> Result<()> {
        let (project_id, task_id) = validate_pair(project_id, task_id)?;
        let token = self
            .stored_token()
            .await?
            .ok_or_else(|| AppError::new(ErrorKind::Invalid, "请先填写滴答清单 API 口令"))?;
        let req = HttpRequest::delete(format!("{BASE}/project/{project_id}/task/{task_id}"))
            .header("Authorization", format!("Bearer {token}"));
        check(self.http.send(req).await?, "删除任务")?;
        Ok(())
    }

    async fn fetch_projects(&self, token: &str) -> Result<Vec<DidaProject>> {
        let req = HttpRequest::get(format!("{BASE}/project"))
            .header("Authorization", format!("Bearer {token}"));
        let resp = check(self.http.send(req).await?, "读取清单")?;
        let value = resp.json()?;
        let items = value.as_array().ok_or_else(|| {
            AppError::new(
                ErrorKind::Provider,
                "滴答清单返回的清单列表格式不符（预期数组）",
            )
        })?;
        Ok(items
            .iter()
            .filter_map(|item| {
                Some(DidaProject {
                    id: text_field(item, "id")?,
                    name: text_field(item, "name")?,
                    color: text_field(item, "color"),
                    closed: item
                        .get("closed")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                })
            })
            .collect())
    }

    async fn stored_token(&self) -> Result<Option<String>> {
        let db = self.db.clone();
        let Some(raw) = blocking(move || db.kv_get(KV_TOKEN)).await?? else {
            return Ok(None);
        };
        let plain = blocking(move || decode_token(&raw)).await??;
        Ok(Some(plain))
    }

    async fn store_token(&self, token: &str) -> Result<()> {
        let plain = token.to_string();
        let encoded = blocking(move || -> Result<String> {
            use base64::Engine;
            let blob = secret::protect(plain.as_bytes())?;
            Ok(base64::engine::general_purpose::STANDARD.encode(blob))
        })
        .await??;
        let db = self.db.clone();
        blocking(move || db.kv_set(KV_TOKEN, &encoded)).await??;
        Ok(())
    }
}

fn decode_token(raw: &str) -> Result<String> {
    use base64::Engine;
    let blob = base64::engine::general_purpose::STANDARD
        .decode(raw.trim().as_bytes())
        .map_err(|_| AppError::new(ErrorKind::Crypto, "滴答清单口令存储已损坏，请重新填写"))?;
    let bytes = secret::unprotect(&blob)
        .map_err(|_| AppError::new(ErrorKind::Crypto, "滴答清单口令存储已损坏，请重新填写"))?;
    String::from_utf8(bytes)
        .map_err(|_| AppError::new(ErrorKind::Crypto, "滴答清单口令存储已损坏，请重新填写"))
}

fn text_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

/// 任务回执与列表项共用一套字段映射；缺 id 的条目直接丢弃，避免界面上出现点不动的幽灵行。
fn parse_task(value: &serde_json::Value) -> Option<DidaTask> {
    Some(DidaTask {
        id: text_field(value, "id")?,
        title: text_field(value, "title").unwrap_or_default(),
        project_id: text_field(value, "projectId").unwrap_or_default(),
        content: text_field(value, "content"),
        due_date: text_field(value, "dueDate"),
        is_all_day: value
            .get("isAllDay")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        priority: value
            .get("priority")
            .and_then(|v| v.as_u64())
            .map(|v| v as u8)
            .unwrap_or(0),
        status: value
            .get("status")
            .and_then(|v| v.as_u64())
            .map(|v| v as u8)
            .unwrap_or(0),
        completed_time: text_field(value, "completedTime"),
    })
}

/// 可清空的文本字段：留空即显式 null，滴答据此清除原值。
fn optional_text(value: Option<&str>) -> serde_json::Value {
    match value.map(str::trim).filter(|s| !s.is_empty()) {
        Some(text) => serde_json::Value::String(text.to_string()),
        None => serde_json::Value::Null,
    }
}

fn validate_pair(project_id: &str, task_id: &str) -> Result<(String, String)> {
    let project_id = project_id.trim().to_string();
    let task_id = task_id.trim().to_string();
    if project_id.is_empty() {
        return Err(AppError::new(ErrorKind::Invalid, "请选择清单"));
    }
    if task_id.is_empty() {
        return Err(AppError::new(ErrorKind::Invalid, "任务不存在或已删除"));
    }
    Ok((project_id, task_id))
}

/// 401/403 是口令被平台拒绝，必须归「登录失效」：前端会给出「重新生成口令」
/// 而不是笼统的「平台错误」。
fn check(resp: HttpResponse, action: &str) -> Result<HttpResponse> {
    if resp.is_ok() {
        return Ok(resp);
    }
    Err(match resp.status {
        401 | 403 => AppError::new(
            ErrorKind::AuthExpired,
            format!("滴答清单 API 口令无效或已失效（{action}），请在网页版重新生成"),
        ),
        429 => AppError::new(ErrorKind::RateLimited, "滴答清单接口限流了，稍等几分钟再试"),
        status => AppError::new(
            ErrorKind::Provider,
            format!("{action}失败：滴答清单返回 HTTP {status}"),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::net::MockHttp;

    const PROJECT_URL: &str = "https://api.dida365.com/open/v1/project";
    const TASK_URL: &str = "https://api.dida365.com/open/v1/task";

    fn service() -> (tempfile::TempDir, Arc<Db>, Arc<MockHttp>, Service) {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Arc::new(Db::open_at(&dir.path().join("app.db")).unwrap());
        let http = Arc::new(MockHttp::new());
        let service = Service::new(db.clone(), http.clone());
        (dir, db, http, service)
    }

    #[tokio::test]
    async fn create_task_sends_url_auth_and_body_fields() {
        let (_dir, _db, http, service) = service();
        http.on(
            PROJECT_URL,
            200,
            r#"[{"id":"inbox","name":"收件箱","closed":false}]"#,
        );
        http.on(
            TASK_URL,
            200,
            r#"{"id":"t1","projectId":"p1","title":"写周报","dueDate":"2026-11-24T20:00:00+0800","priority":3}"#,
        );
        service.set_token("tk-123").await.unwrap();

        service
            .create_task(
                "写周报",
                "p1",
                Some("本周进展"),
                Some("2026-11-24T20:00:00+0800"),
                Some(3),
                false,
            )
            .await
            .unwrap();

        assert_eq!(http.call_count(TASK_URL), 1);
        assert_eq!(
            http.last_header(TASK_URL, "authorization").as_deref(),
            Some("Bearer tk-123")
        );
        let body: serde_json::Value =
            serde_json::from_str(&http.last_body(TASK_URL).unwrap()).unwrap();
        assert_eq!(body["title"], "写周报");
        assert_eq!(body["projectId"], "p1");
        assert_eq!(body["content"], "本周进展");
        assert_eq!(body["dueDate"], "2026-11-24T20:00:00+0800");
        assert_eq!(body["priority"], 3);
    }

    #[tokio::test]
    async fn invalid_token_is_auth_expired_and_state_keeps_readable_reason() {
        let (_dir, _db, http, service) = service();
        http.on(PROJECT_URL, 401, "");

        let err = service.set_token("bad-token").await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::AuthExpired);
        assert!(err.message.contains("口令"), "{}", err.message);

        // 已落库的旧口令被平台拒绝：状态仍要显示「已配置」，原因可读
        service.store_token("stale-token").await.unwrap();
        let state = service.state().await;
        assert!(state.configured);
        assert!(state.projects.is_empty());
        assert!(state.error.unwrap().contains("口令"));
    }

    const DATA_URL: &str = "https://api.dida365.com/open/v1/project/p1/data";
    const UPDATE_URL: &str = "https://api.dida365.com/open/v1/task/t1";
    const COMPLETE_URL: &str = "https://api.dida365.com/open/v1/project/p1/task/t1/complete";
    const DELETE_URL: &str = "https://api.dida365.com/open/v1/project/p1/task/t1";

    #[tokio::test]
    async fn list_tasks_reads_project_data() {
        let (_dir, _db, http, service) = service();
        http.on(
            PROJECT_URL,
            200,
            r#"[{"id":"p1","name":"工作","closed":false}]"#,
        );
        http.on(
            DATA_URL,
            200,
            r#"{"tasks":[{"id":"t1","projectId":"p1","title":"写周报","priority":3,"status":0},{"id":"t2","projectId":"p1","title":"旧任务","status":2,"completedTime":"2026-11-20T10:00:00+0800"}]}"#,
        );
        service.set_token("tk-123").await.unwrap();

        let tasks = service.list_tasks("p1").await.unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].priority, 3);
        assert_eq!(tasks[1].status, 2);
        assert_eq!(
            tasks[1].completed_time.as_deref(),
            Some("2026-11-20T10:00:00+0800")
        );
        assert_eq!(http.call_count(DATA_URL), 1);
    }

    #[tokio::test]
    async fn update_task_sends_full_body_and_clears_empty_fields() {
        let (_dir, _db, http, service) = service();
        http.on(
            PROJECT_URL,
            200,
            r#"[{"id":"p1","name":"工作","closed":false}]"#,
        );
        http.on(
            UPDATE_URL,
            200,
            r#"{"id":"t1","projectId":"p1","title":"新标题","priority":5}"#,
        );
        service.set_token("tk-123").await.unwrap();

        service
            .update_task("t1", "p1", "新标题", None, None, Some(5), false)
            .await
            .unwrap();

        let body: serde_json::Value =
            serde_json::from_str(&http.last_body(UPDATE_URL).unwrap()).unwrap();
        assert_eq!(body["title"], "新标题");
        assert_eq!(body["projectId"], "p1");
        assert!(body["content"].is_null());
        assert!(body["dueDate"].is_null());
        assert_eq!(body["priority"], 5);
    }

    #[tokio::test]
    async fn update_task_retries_with_post_when_put_is_rejected() {
        let (_dir, _db, http, service) = service();
        http.on(
            PROJECT_URL,
            200,
            r#"[{"id":"p1","name":"工作","closed":false}]"#,
        );
        http.on_sequence(
            UPDATE_URL,
            vec![
                (405, ""),
                (200, r#"{"id":"t1","projectId":"p1","title":"新标题"}"#),
            ],
        );
        service.set_token("tk-123").await.unwrap();

        let task = service
            .update_task("t1", "p1", "新标题", None, None, None, false)
            .await
            .unwrap();
        assert_eq!(task.id, "t1");
        assert_eq!(http.call_count(UPDATE_URL), 2);
    }

    #[tokio::test]
    async fn complete_and_delete_hit_their_own_endpoints() {
        let (_dir, _db, http, service) = service();
        http.on(
            PROJECT_URL,
            200,
            r#"[{"id":"p1","name":"工作","closed":false}]"#,
        );
        http.on(COMPLETE_URL, 200, "");
        http.on(DELETE_URL, 200, "");
        service.set_token("tk-123").await.unwrap();

        service.complete_task("p1", "t1").await.unwrap();
        service.delete_task("p1", "t1").await.unwrap();

        assert_eq!(http.call_count(COMPLETE_URL), 1);
        assert_eq!(http.call_count(DELETE_URL), 1);
        assert_eq!(
            http.last_header(COMPLETE_URL, "authorization").as_deref(),
            Some("Bearer tk-123")
        );
    }

    #[tokio::test]
    async fn token_is_encrypted_before_storage() {
        use base64::Engine;
        let (_dir, db, _http, service) = service();
        service.store_token("plain-secret-token").await.unwrap();

        let raw = db.kv_get(KV_TOKEN).unwrap().unwrap();
        assert!(!raw.contains("plain-secret-token"), "kv 里不能出现口令明文");
        let blob = base64::engine::general_purpose::STANDARD
            .decode(raw.as_bytes())
            .unwrap();
        assert_eq!(secret::unprotect(&blob).unwrap(), b"plain-secret-token");
        assert_eq!(
            service.stored_token().await.unwrap().as_deref(),
            Some("plain-secret-token")
        );
    }
}
