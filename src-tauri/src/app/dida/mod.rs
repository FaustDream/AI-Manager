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

/// 清单式子任务（滴答的 `items[]`）。与「真父子任务」是两回事：
/// items 是任务内部的勾选条目，只有标题/完成态/排序/时间；
/// 真父子任务由 `parent_id` 关联，子任务本身是完整任务。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, serde::Deserialize)]
pub struct DidaItem {
    /// 新建条目时为空，由平台分配
    #[serde(default)]
    pub id: String,
    pub title: String,
    /// 0 未完成、1 已完成（注意与任务级 status 的 0/2 不同口径）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    #[serde(default)]
    pub status: u8,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    #[serde(default)]
    pub sort_order: i64,
    #[serde(default)]
    pub start_date: Option<String>,
    #[serde(default)]
    pub is_all_day: bool,
    #[serde(default)]
    pub time_zone: Option<String>,
    #[serde(default)]
    pub completed_time: Option<String>,
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
    /// 父任务 id。空串 = 顶层任务。真父子关系要求父子同清单。
    pub parent_id: String,
    /// 清单式子任务
    pub items: Vec<DidaItem>,
    pub start_date: Option<String>,
    /// IANA 时区名（如 Asia/Shanghai）。为空时按 +0800 解释。
    pub time_zone: Option<String>,
    /// 提醒触发串（滴答 TRIGGER 语法，如 TRIGGER:-PT0M）
    pub reminders: Vec<String>,
    /// 重复规则（RRULE，如 RRULE:FREQ=DAILY;INTERVAL=1）
    pub repeat_flag: Option<String>,
    pub tags: Vec<String>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub sort_order: i64,
    /// 多行描述（与 content 的单行备注区分，滴答两个字段都有）
    pub desc: Option<String>,
    /// TEXT / CHECKLIST / NOTE
    pub kind: String,
    pub etag: Option<String>,
    pub created_time: Option<String>,
    pub modified_time: Option<String>,
    /// 负责人用户名。只读——指派走 assign/unassign 端点，且需要共享清单。
    pub assignee: Option<String>,
}

/// 新建 / 更新任务的入参。**字段即滴答开放平台任务对象的可写子集**。
///
/// 用一个结构体而不是十几个位置参数：字段还在长，位置参数每次都要改所有调用点，
/// 而且 `Option<&str>` 一路排下去极易传错顺序。
///
/// 清空语义：`content` / `due_date` / `start_date` / `repeat_flag` / `desc`
/// 传 `None` 表示**显式清除**（发 null）；`tags` / `reminders` / `items`
/// 传空数组表示清空该集合。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct DidaTaskInput {
    /// 有值 = 更新，空 = 新建
    #[serde(default)]
    pub id: String,
    pub title: String,
    pub project_id: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub desc: Option<String>,
    #[serde(default)]
    pub due_date: Option<String>,
    #[serde(default)]
    pub start_date: Option<String>,
    #[serde(default)]
    pub is_all_day: bool,
    #[serde(default)]
    pub time_zone: Option<String>,
    #[serde(default)]
    pub priority: Option<u8>,
    #[serde(default)]
    pub reminders: Vec<String>,
    #[serde(default)]
    pub repeat_flag: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub items: Vec<DidaItem>,
    /// 空串 = 解除父子关系（平台文档语义）；None = 本次不动父子关系
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub sort_order: Option<i64>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DidaMember {
    pub username: String,
    pub name: Option<String>,
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

    /// 新建任务（旧签名保留给既有调用点与测试，内部走统一的 save_task）。
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
        self.save_task(DidaTaskInput {
            title: title.to_string(),
            project_id: project_id.to_string(),
            content: content.map(str::to_string),
            due_date: due_date.map(str::to_string),
            priority,
            is_all_day,
            ..Default::default()
        })
        .await
    }

    /// 新建或更新任务：`input.id` 有值即更新。
    ///
    /// 更新走 PUT，遇 404/405 用同一体改 POST 重试一次——开放平台文档对更新动词
    /// 表述不一（两种写法都存在），body 幂等，重放无副作用。
    pub async fn save_task(&self, input: DidaTaskInput) -> Result<DidaTask> {
        validate_input(&input)?;
        let token = self.token().await?;
        let is_update = !input.id.trim().is_empty();
        let body = task_body(&input, is_update);

        if !is_update {
            let req = HttpRequest::post(format!("{BASE}/task"))
                .header("Authorization", format!("Bearer {token}"))
                .json_body(&body);
            let resp = check(self.http.send(req).await?, "创建任务")?;
            return parse_task(&resp.json()?).ok_or_else(|| {
                AppError::new(ErrorKind::Provider, "滴答清单创建任务的回执缺少任务 id")
            });
        }

        let url = format!("{BASE}/task/{}", input.id.trim());
        let req = HttpRequest::put(url.clone())
            .header("Authorization", format!("Bearer {token}"))
            .json_body(&body);
        let resp = self.http.send(req).await?;
        let resp = if resp.status == 404 || resp.status == 405 {
            let req = HttpRequest::post(url)
                .header("Authorization", format!("Bearer {token}"))
                .json_body(&body);
            self.http.send(req).await?
        } else {
            resp
        };
        let resp = check(resp, "更新任务")?;
        parse_task(&resp.json()?)
            .ok_or_else(|| AppError::new(ErrorKind::Provider, "滴答清单更新任务的回执缺少任务 id"))
    }

    /// 更新任务（旧签名保留）。
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
        if task_id.trim().is_empty() {
            return Err(AppError::new(ErrorKind::Invalid, "任务不存在或已删除"));
        }
        self.save_task(DidaTaskInput {
            id: task_id.to_string(),
            title: title.to_string(),
            project_id: project_id.to_string(),
            content: content.map(str::to_string),
            due_date: due_date.map(str::to_string),
            priority,
            is_all_day,
            ..Default::default()
        })
        .await
    }

    async fn token(&self) -> Result<String> {
        self.stored_token()
            .await?
            .ok_or_else(|| AppError::new(ErrorKind::Invalid, "请先填写滴答清单 API 口令"))
    }

    /// 单个清单的任务：开放平台没有全局任务列表，只能按清单取
    /// （跨清单聚合走 `undone_tasks`）。回执里若带已完成条目就一并保留，
    /// 完整的已完成归档走 `completed_tasks`。
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

    /// 单任务详情：列表回执有时缺 items / desc，编辑前按 id 取一次全量。
    pub async fn task_detail(&self, project_id: &str, task_id: &str) -> Result<DidaTask> {
        let (project_id, task_id) = validate_pair(project_id, task_id)?;
        let token = self.token().await?;
        let req = HttpRequest::get(format!("{BASE}/project/{project_id}/task/{task_id}"))
            .header("Authorization", format!("Bearer {token}"));
        let resp = check(self.http.send(req).await?, "读取任务详情")?;
        parse_task(&resp.json()?)
            .ok_or_else(|| AppError::new(ErrorKind::Provider, "滴答清单任务详情缺少任务 id"))
    }

    /// 已完成任务。按清单 + 完成时间区间取，单次上限 200（平台上限）。
    ///
    /// 代码里原先写着「国内版恒 404」——那是旧版开放平台的结论，现已提供该端点。
    /// 真机若仍拒绝，错误按 Provider 归类并保留原文，不静默当成空列表。
    pub async fn completed_tasks(
        &self,
        project_ids: &[String],
        from: &str,
        to: &str,
    ) -> Result<Vec<DidaTask>> {
        let ids = clean_list(project_ids);
        if ids.is_empty() {
            return Err(AppError::new(ErrorKind::Invalid, "请选择清单"));
        }
        if from.trim().is_empty() || to.trim().is_empty() {
            return Err(AppError::new(ErrorKind::Invalid, "请给出完成时间区间"));
        }
        let token = self.token().await?;
        let body = serde_json::json!({
            "projectIds": ids,
            "startDate": from.trim(),
            "endDate": to.trim(),
        });
        let req = HttpRequest::post(format!("{BASE}/task/completed"))
            .header("Authorization", format!("Bearer {token}"))
            .json_body(&body);
        let resp = check(self.http.send(req).await?, "读取已完成任务")?;
        Ok(task_array(&resp.json()?))
    }

    /// 全部清单的未完成任务（平台限最近 14 天窗口）。多项目聚合视图用它，
    /// 不必对每个清单各打一发。
    pub async fn undone_tasks(&self) -> Result<Vec<DidaTask>> {
        let token = self.token().await?;
        let req = HttpRequest::post(format!("{BASE}/task/undone"))
            .header("Authorization", format!("Bearer {token}"))
            .json_body(&serde_json::json!({}));
        let resp = check(self.http.send(req).await?, "读取未完成任务")?;
        Ok(task_array(&resp.json()?))
    }

    /// 批量完成。单次上限 50（平台上限），超出直接拒绝而不是静默截断。
    pub async fn complete_many(&self, pairs: &[(String, String)]) -> Result<usize> {
        let items = task_pairs(pairs)?;
        if items.len() > 50 {
            return Err(AppError::new(
                ErrorKind::Invalid,
                "单次最多完成 50 条，请分批操作",
            ));
        }
        let token = self.token().await?;
        let req = HttpRequest::post(format!("{BASE}/task/completeTasks"))
            .header("Authorization", format!("Bearer {token}"))
            .json_body(&serde_json::Value::Array(items.clone()));
        check(self.http.send(req).await?, "批量完成任务")?;
        Ok(items.len())
    }

    /// 批量移动到其他清单。父子任务必须同清单，移动父任务时调用方要带上子任务。
    pub async fn move_many(
        &self,
        pairs: &[(String, String)],
        to_project_id: &str,
    ) -> Result<usize> {
        let to_project_id = to_project_id.trim();
        if to_project_id.is_empty() {
            return Err(AppError::new(ErrorKind::Invalid, "请选择目标清单"));
        }
        let items: Vec<serde_json::Value> = pairs
            .iter()
            .filter_map(|(from, tid)| {
                let (from, tid) = (from.trim(), tid.trim());
                if from.is_empty() || tid.is_empty() || from == to_project_id {
                    return None;
                }
                Some(serde_json::json!({
                    "fromProjectId": from, "toProjectId": to_project_id, "taskId": tid,
                }))
            })
            .collect();
        if items.is_empty() {
            return Err(AppError::new(ErrorKind::Invalid, "没有需要移动的任务"));
        }
        let token = self.token().await?;
        let req = HttpRequest::post(format!("{BASE}/task/move"))
            .header("Authorization", format!("Bearer {token}"))
            .json_body(&serde_json::Value::Array(items.clone()));
        check(self.http.send(req).await?, "移动任务")?;
        Ok(items.len())
    }

    /// 指派负责人。需要共享清单（个人清单没有成员），平台侧还可能受会员限制。
    pub async fn assign_task(
        &self,
        project_id: &str,
        task_id: &str,
        username: &str,
    ) -> Result<()> {
        let (project_id, task_id) = validate_pair(project_id, task_id)?;
        let username = username.trim();
        if username.is_empty() {
            return Err(AppError::new(ErrorKind::Invalid, "请填写负责人用户名"));
        }
        let token = self.token().await?;
        let req = HttpRequest::post(format!("{BASE}/task/assign"))
            .header("Authorization", format!("Bearer {token}"))
            .json_body(&serde_json::json!({
                "projectId": project_id, "taskId": task_id, "assigneeUsername": username,
            }));
        check(self.http.send(req).await?, "指派负责人")?;
        Ok(())
    }

    pub async fn unassign_task(&self, project_id: &str, task_id: &str) -> Result<()> {
        let (project_id, task_id) = validate_pair(project_id, task_id)?;
        let token = self.token().await?;
        let req = HttpRequest::post(format!("{BASE}/task/unassign"))
            .header("Authorization", format!("Bearer {token}"))
            .json_body(&serde_json::json!({ "projectId": project_id, "taskId": task_id }));
        check(self.http.send(req).await?, "取消指派")?;
        Ok(())
    }

    /// 清单成员。指派下拉的候选人来源；个人清单返回空数组。
    pub async fn project_members(&self, project_id: &str) -> Result<Vec<DidaMember>> {
        let project_id = project_id.trim();
        if project_id.is_empty() {
            return Err(AppError::new(ErrorKind::Invalid, "请选择清单"));
        }
        let token = self.token().await?;
        let req = HttpRequest::get(format!("{BASE}/project/{project_id}/members"))
            .header("Authorization", format!("Bearer {token}"));
        let resp = check(self.http.send(req).await?, "读取清单成员")?;
        let value = resp.json()?;
        Ok(value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        Some(DidaMember {
                            username: text_field(item, "username")
                                .or_else(|| text_field(item, "email"))?,
                            name: text_field(item, "displayName")
                                .or_else(|| text_field(item, "name")),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
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
        // 平台对顶层任务给 ""，对子任务给父 id；缺字段按顶层处理
        parent_id: text_field(value, "parentId").unwrap_or_default(),
        items: value
            .get("items")
            .and_then(|v| v.as_array())
            .map(|items| items.iter().filter_map(parse_item).collect())
            .unwrap_or_default(),
        start_date: text_field(value, "startDate"),
        time_zone: text_field(value, "timeZone"),
        reminders: string_list(value, "reminders"),
        repeat_flag: text_field(value, "repeatFlag"),
        tags: string_list(value, "tags"),
        sort_order: value
            .get("sortOrder")
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        desc: text_field(value, "desc"),
        kind: text_field(value, "kind").unwrap_or_else(|| "TEXT".into()),
        etag: text_field(value, "etag"),
        created_time: text_field(value, "createdTime"),
        modified_time: text_field(value, "modifiedTime"),
        assignee: text_field(value, "assigneeUsername"),
    })
}

fn parse_item(value: &serde_json::Value) -> Option<DidaItem> {
    Some(DidaItem {
        id: text_field(value, "id").unwrap_or_default(),
        title: text_field(value, "title")?,
        status: value
            .get("status")
            .and_then(|v| v.as_u64())
            .map(|v| v as u8)
            .unwrap_or(0),
        sort_order: value
            .get("sortOrder")
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        start_date: text_field(value, "startDate"),
        is_all_day: value
            .get("isAllDay")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        time_zone: text_field(value, "timeZone"),
        completed_time: text_field(value, "completedTime"),
    })
}

fn string_list(value: &serde_json::Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// 入参 → 请求体。**新建与更新共用**，避免两处字段漂移。
///
/// `is_update` 决定清空语义：更新时可清空字段发显式 null；新建时省略即可，
/// 发一堆 null 只会让平台回执里多出无意义的字段。
fn task_body(input: &DidaTaskInput, is_update: bool) -> serde_json::Value {
    let mut body = serde_json::json!({
        "title": input.title.trim(),
        "projectId": input.project_id.trim(),
        "isAllDay": input.is_all_day,
    });
    if is_update {
        body["id"] = serde_json::json!(input.id.trim());
        body["content"] = optional_text(input.content.as_deref());
        body["desc"] = optional_text(input.desc.as_deref());
        body["dueDate"] = optional_text(input.due_date.as_deref());
        body["startDate"] = optional_text(input.start_date.as_deref());
        body["repeatFlag"] = optional_text(input.repeat_flag.as_deref());
    } else {
        for (key, value) in [
            ("content", input.content.as_deref()),
            ("desc", input.desc.as_deref()),
            ("dueDate", input.due_date.as_deref()),
            ("startDate", input.start_date.as_deref()),
            ("repeatFlag", input.repeat_flag.as_deref()),
        ] {
            if let Some(text) = value.map(str::trim).filter(|s| !s.is_empty()) {
                body[key] = serde_json::Value::String(text.to_string());
            }
        }
    }
    if let Some(priority) = input.priority {
        body["priority"] = serde_json::json!(priority);
    }
    if let Some(zone) = input.time_zone.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        body["timeZone"] = serde_json::Value::String(zone.to_string());
    }
    if let Some(sort_order) = input.sort_order {
        body["sortOrder"] = serde_json::json!(sort_order);
    }
    // 集合字段：更新时空数组表示清空，新建时空数组省略
    if is_update || !input.tags.is_empty() {
        body["tags"] = serde_json::json!(clean_list(&input.tags));
    }
    if is_update || !input.reminders.is_empty() {
        body["reminders"] = serde_json::json!(clean_list(&input.reminders));
    }
    if is_update || !input.items.is_empty() {
        body["items"] = serde_json::Value::Array(
            input
                .items
                .iter()
                .filter(|item| !item.title.trim().is_empty())
                .enumerate()
                .map(|(index, item)| {
                    let mut obj = serde_json::json!({
                        "title": item.title.trim(),
                        "status": item.status,
                        "sortOrder": if item.sort_order == 0 { index as i64 } else { item.sort_order },
                    });
                    if !item.id.trim().is_empty() {
                        obj["id"] = serde_json::json!(item.id.trim());
                    }
                    if let Some(start) = item.start_date.as_deref().filter(|s| !s.is_empty()) {
                        obj["startDate"] = serde_json::json!(start);
                        obj["isAllDay"] = serde_json::json!(item.is_all_day);
                    }
                    if let Some(zone) = item.time_zone.as_deref().filter(|s| !s.is_empty()) {
                        obj["timeZone"] = serde_json::json!(zone);
                    }
                    obj
                })
                .collect(),
        );
    }
    // parentId 只在调用方明确给出时才发：None = 本次不动父子关系，
    // Some("") = 解除关系（平台文档语义）。默认不动，避免编辑标题时把子任务提成顶层。
    if let Some(parent) = input.parent_id.as_deref() {
        body["parentId"] = serde_json::json!(parent.trim());
    }
    body
}

fn clean_list(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn validate_input(input: &DidaTaskInput) -> Result<()> {
    if input.title.trim().is_empty() {
        return Err(AppError::new(ErrorKind::Invalid, "任务标题不能为空"));
    }
    if input.project_id.trim().is_empty() {
        return Err(AppError::new(ErrorKind::Invalid, "请选择清单"));
    }
    if let Some(priority) = input.priority {
        if !matches!(priority, 0 | 1 | 3 | 5) {
            return Err(AppError::new(ErrorKind::Invalid, "优先级只能是 0/1/3/5"));
        }
    }
    Ok(())
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

/// 批量端点共用的 [{projectId, taskId}] 体。
fn task_pairs(pairs: &[(String, String)]) -> Result<Vec<serde_json::Value>> {
    let items: Vec<serde_json::Value> = pairs
        .iter()
        .filter_map(|(pid, tid)| {
            let (pid, tid) = (pid.trim(), tid.trim());
            if pid.is_empty() || tid.is_empty() {
                return None;
            }
            Some(serde_json::json!({ "projectId": pid, "taskId": tid }))
        })
        .collect();
    if items.is_empty() {
        return Err(AppError::new(ErrorKind::Invalid, "没有选中任务"));
    }
    Ok(items)
}

/// 任务数组回执：既接受裸数组，也接受 {tasks:[...]} 包裹（平台两种都出现过）。
fn task_array(value: &serde_json::Value) -> Vec<DidaTask> {
    let items = value
        .as_array()
        .or_else(|| value.get("tasks").and_then(|v| v.as_array()));
    items
        .map(|items| items.iter().filter_map(parse_task).collect())
        .unwrap_or_default()
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

    // ------------------------------------------------ 子任务 / 时间 / 提醒 / 重复 / 标签

    async fn seeded() -> (tempfile::TempDir, Arc<MockHttp>, Service) {
        let (dir, _db, http, service) = service();
        http.on(
            PROJECT_URL,
            200,
            r#"[{"id":"p1","name":"工作","closed":false}]"#,
        );
        service.set_token("tk-123").await.unwrap();
        (dir, http, service)
    }

    #[tokio::test]
    async fn save_task_sends_subtasks_time_reminders_repeat_and_tags() {
        let (_dir, http, service) = seeded().await;
        http.on(
            TASK_URL,
            200,
            r#"{"id":"t9","projectId":"p1","title":"发布","kind":"CHECKLIST","items":[{"id":"i1","title":"写公告","status":0}],"tags":["发布"],"reminders":["TRIGGER:-PT15M"],"repeatFlag":"RRULE:FREQ=WEEKLY;INTERVAL=1","startDate":"2026-11-25T09:00:00+0800","timeZone":"Asia/Shanghai"}"#,
        );

        let task = service
            .save_task(DidaTaskInput {
                title: "发布".into(),
                project_id: "p1".into(),
                start_date: Some("2026-11-25T09:00:00+0800".into()),
                due_date: Some("2026-11-25T18:00:00+0800".into()),
                time_zone: Some("Asia/Shanghai".into()),
                priority: Some(5),
                reminders: vec!["TRIGGER:-PT15M".into()],
                repeat_flag: Some("RRULE:FREQ=WEEKLY;INTERVAL=1".into()),
                tags: vec!["发布".into(), "  ".into()],
                items: vec![
                    DidaItem {
                        title: "写公告".into(),
                        ..Default::default()
                    },
                    DidaItem {
                        title: "  ".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            })
            .await
            .unwrap();

        let body: serde_json::Value =
            serde_json::from_str(&http.last_body(TASK_URL).unwrap()).unwrap();
        assert_eq!(body["startDate"], "2026-11-25T09:00:00+0800");
        assert_eq!(body["timeZone"], "Asia/Shanghai");
        assert_eq!(body["reminders"][0], "TRIGGER:-PT15M");
        assert_eq!(body["repeatFlag"], "RRULE:FREQ=WEEKLY;INTERVAL=1");
        assert_eq!(body["tags"].as_array().unwrap().len(), 1, "空白标签要剔掉");
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 1, "空标题子任务不该发出去");
        assert_eq!(items[0]["title"], "写公告");
        assert_eq!(items[0]["sortOrder"], 0);
        // 新建时不该凭空发 parentId，否则平台可能把任务挂到意外的父级
        assert!(body.get("parentId").is_none());

        assert_eq!(task.items.len(), 1);
        assert_eq!(task.tags, vec!["发布".to_string()]);
        assert_eq!(task.repeat_flag.as_deref(), Some("RRULE:FREQ=WEEKLY;INTERVAL=1"));
        assert_eq!(task.time_zone.as_deref(), Some("Asia/Shanghai"));
        assert_eq!(task.kind, "CHECKLIST");
    }

    #[tokio::test]
    async fn update_clears_collections_and_can_detach_parent() {
        let (_dir, http, service) = seeded().await;
        http.on(
            UPDATE_URL,
            200,
            r#"{"id":"t1","projectId":"p1","title":"改标题","parentId":""}"#,
        );

        service
            .save_task(DidaTaskInput {
                id: "t1".into(),
                title: "改标题".into(),
                project_id: "p1".into(),
                parent_id: Some("".into()),
                ..Default::default()
            })
            .await
            .unwrap();

        let body: serde_json::Value =
            serde_json::from_str(&http.last_body(UPDATE_URL).unwrap()).unwrap();
        assert_eq!(body["id"], "t1");
        assert_eq!(body["parentId"], "", "空串 = 解除父子关系");
        assert!(body["tags"].as_array().unwrap().is_empty(), "空数组 = 清空标签");
        assert!(body["items"].as_array().unwrap().is_empty());
        assert!(body["reminders"].as_array().unwrap().is_empty());
        assert!(body["repeatFlag"].is_null());
        assert!(body["startDate"].is_null());
    }

    #[tokio::test]
    async fn update_without_parent_field_leaves_hierarchy_untouched() {
        let (_dir, http, service) = seeded().await;
        http.on(UPDATE_URL, 200, r#"{"id":"t1","projectId":"p1","title":"改标题"}"#);

        service
            .save_task(DidaTaskInput {
                id: "t1".into(),
                title: "改标题".into(),
                project_id: "p1".into(),
                ..Default::default()
            })
            .await
            .unwrap();

        let body: serde_json::Value =
            serde_json::from_str(&http.last_body(UPDATE_URL).unwrap()).unwrap();
        assert!(
            body.get("parentId").is_none(),
            "只改标题不该把子任务提成顶层任务"
        );
    }

    #[tokio::test]
    async fn parse_task_reads_parent_and_subtasks_from_list() {
        let (_dir, http, service) = seeded().await;
        http.on(
            DATA_URL,
            200,
            r#"{"tasks":[{"id":"p","projectId":"p1","title":"父任务","parentId":"","items":[{"id":"i1","title":"清单项","status":1,"sortOrder":3}]},{"id":"c","projectId":"p1","title":"子任务","parentId":"p"}]}"#,
        );
        let tasks = service.list_tasks("p1").await.unwrap();
        assert_eq!(tasks[0].parent_id, "");
        assert_eq!(tasks[0].items[0].title, "清单项");
        assert_eq!(tasks[0].items[0].status, 1);
        assert_eq!(tasks[0].items[0].sort_order, 3);
        assert_eq!(tasks[1].parent_id, "p", "真父子关系要读出来");
    }

    // ------------------------------------------------ 已完成 / 聚合 / 批量 / 指派

    const COMPLETED_URL: &str = "https://api.dida365.com/open/v1/task/completed";
    const UNDONE_URL: &str = "https://api.dida365.com/open/v1/task/undone";
    const COMPLETE_MANY_URL: &str = "https://api.dida365.com/open/v1/task/completeTasks";
    const MOVE_URL: &str = "https://api.dida365.com/open/v1/task/move";
    const ASSIGN_URL: &str = "https://api.dida365.com/open/v1/task/assign";
    const DETAIL_URL: &str = "https://api.dida365.com/open/v1/project/p1/task/t1";
    const MEMBERS_URL: &str = "https://api.dida365.com/open/v1/project/p1/members";

    #[tokio::test]
    async fn completed_tasks_queries_by_project_and_range() {
        let (_dir, http, service) = seeded().await;
        http.on(
            COMPLETED_URL,
            200,
            r#"[{"id":"d1","projectId":"p1","title":"已做完","status":2,"completedTime":"2026-11-20T10:00:00+0800"}]"#,
        );
        let tasks = service
            .completed_tasks(
                &["p1".to_string()],
                "2026-11-01T00:00:00+0800",
                "2026-11-25T23:59:59+0800",
            )
            .await
            .unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, 2);
        let body: serde_json::Value =
            serde_json::from_str(&http.last_body(COMPLETED_URL).unwrap()).unwrap();
        assert_eq!(body["projectIds"][0], "p1");
        assert_eq!(body["startDate"], "2026-11-01T00:00:00+0800");
    }

    #[tokio::test]
    async fn completed_tasks_reports_platform_rejection_instead_of_empty_list() {
        let (_dir, http, service) = seeded().await;
        // 旧版开放平台对这个端点回 404：必须报出来，不能静默当成「没有已完成任务」
        http.on(COMPLETED_URL, 404, "");
        let err = service
            .completed_tasks(&["p1".to_string()], "a", "b")
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Provider);
        assert!(err.message.contains("404"), "{}", err.message);
    }

    #[tokio::test]
    async fn undone_tasks_accepts_both_array_and_wrapped_shapes() {
        let (_dir, http, service) = seeded().await;
        http.on(
            UNDONE_URL,
            200,
            r#"{"tasks":[{"id":"a","projectId":"p1","title":"甲"},{"id":"b","projectId":"p2","title":"乙"}]}"#,
        );
        let tasks = service.undone_tasks().await.unwrap();
        assert_eq!(tasks.len(), 2, "跨清单聚合");
        assert_eq!(tasks[1].project_id, "p2");
    }

    #[tokio::test]
    async fn batch_complete_and_move_send_pair_arrays() {
        let (_dir, http, service) = seeded().await;
        http.on(COMPLETE_MANY_URL, 200, "");
        http.on(MOVE_URL, 200, "");
        let pairs = vec![
            ("p1".to_string(), "t1".to_string()),
            ("p1".to_string(), "t2".to_string()),
        ];
        assert_eq!(service.complete_many(&pairs).await.unwrap(), 2);
        assert_eq!(service.move_many(&pairs, "p2").await.unwrap(), 2);

        let body: serde_json::Value =
            serde_json::from_str(&http.last_body(COMPLETE_MANY_URL).unwrap()).unwrap();
        assert_eq!(body[0]["taskId"], "t1");
        let body: serde_json::Value =
            serde_json::from_str(&http.last_body(MOVE_URL).unwrap()).unwrap();
        assert_eq!(body[0]["fromProjectId"], "p1");
        assert_eq!(body[0]["toProjectId"], "p2");
    }

    #[tokio::test]
    async fn batch_complete_refuses_oversized_selection() {
        let (_dir, _http, service) = seeded().await;
        let pairs: Vec<(String, String)> = (0..51)
            .map(|i| ("p1".to_string(), format!("t{i}")))
            .collect();
        let err = service.complete_many(&pairs).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        assert!(err.message.contains("50"), "{}", err.message);
    }

    #[tokio::test]
    async fn move_skips_tasks_already_in_target_project() {
        let (_dir, _http, service) = seeded().await;
        let pairs = vec![("p2".to_string(), "t1".to_string())];
        let err = service.move_many(&pairs, "p2").await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
    }

    #[tokio::test]
    async fn detail_members_and_assign_hit_their_endpoints() {
        let (_dir, http, service) = seeded().await;
        http.on(
            DETAIL_URL,
            200,
            r#"{"id":"t1","projectId":"p1","title":"全量","desc":"多行描述","items":[{"id":"i1","title":"子项"}]}"#,
        );
        http.on(
            MEMBERS_URL,
            200,
            r#"[{"username":"alice","displayName":"爱丽丝"}]"#,
        );
        http.on(ASSIGN_URL, 200, "");

        let task = service.task_detail("p1", "t1").await.unwrap();
        assert_eq!(task.desc.as_deref(), Some("多行描述"));
        assert_eq!(task.items.len(), 1);

        let members = service.project_members("p1").await.unwrap();
        assert_eq!(members[0].username, "alice");
        assert_eq!(members[0].name.as_deref(), Some("爱丽丝"));

        service.assign_task("p1", "t1", "alice").await.unwrap();
        let body: serde_json::Value =
            serde_json::from_str(&http.last_body(ASSIGN_URL).unwrap()).unwrap();
        assert_eq!(body["assigneeUsername"], "alice");
    }
}
