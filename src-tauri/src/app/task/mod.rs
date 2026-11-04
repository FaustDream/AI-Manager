//! 任务调度层：任务表（TaskStore）+ 取消令牌（CancelToken）+ 统一托管入口。
//!
//! 长任务一律走 `TaskStore::spawn_managed`，不要手写
//! `tauri::async_runtime::spawn` + 手动 `set(Running)`——那会让业务 panic
//! 之后任务永远卡在 Running，前端再也收不到结果。
//! 互斥与并发上限由各能力自己协调（阻塞池上限见 `runtime.rs`），
//! 本层只负责「看得见、能取消、崩溃有交代」。

use crate::app::error::ErrorKind;
use serde::Serialize;
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
pub enum CapabilityId {
    AccountCenter,
    Sessions,
    Hub,
    Maintenance,
    Settings,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
/// 七态。比 v1 多了 cancelling——用户点了取消必须看见反馈。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Running,
    Cancelling,
    Done,
    /// 部分条目失败（批量操作里常见）
    Partial,
    Failed,
    Cancelled,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub id: u64,
    pub capability: CapabilityId,
    pub name: String,
    pub state: TaskState,
    pub progress: Option<f32>,
    pub message: String,
    /// spawn → 开始执行的排队耗时；Queued 时为 None，转 Running 时填一次。
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub queue_ms: Option<u64>,
    /// 单调递增，前端据此丢弃过期帧
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub rev: u64,
    /// 排队开始时刻，只在进程内算 queue_ms 用，不给前端
    #[serde(skip)]
    #[cfg_attr(feature = "ts", ts(skip))]
    pub queued_at: Option<std::time::Instant>,
}

/// 取消令牌。排队中立即生效；运行中的同步代码需在安全点调 `checkpoint()`。
pub struct CancelToken {
    tx: watch::Sender<bool>,
    /// 必须持有一个接收者：tokio 的 `watch` 在无接收者时 `send()` 会失败
    /// 且**不更新值**，取消就会静默失效（这个坑真实踩过）。
    _keep: watch::Receiver<bool>,
}

impl CancelToken {
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(false);
        Self { tx, _keep: rx }
    }

    pub fn cancel(&self) {
        // send_replace 永不失败，语义也更准确：取消是终点态，不需要保留旧值
        self.tx.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.tx.subscribe().borrow()
    }

    /// async 侧等待取消。
    pub async fn cancelled(&self) {
        let mut rx = self.tx.subscribe();
        if *rx.borrow() {
            return;
        }
        let _ = rx.changed().await;
    }

    /// 同步阻塞代码（rusqlite 循环）在事务边界调用，返回 Err(Cancelled)。
    pub fn checkpoint(&self) -> crate::app::error::Result<()> {
        if self.is_cancelled() {
            Err(crate::app::error::AppError::new(
                ErrorKind::Cancelled,
                "任务已取消",
            ))
        } else {
            Ok(())
        }
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

/// 任务表。前端按 id 与 cap 订阅，不持有自己的状态副本。
pub struct TaskStore {
    inner: RwLock<HashMap<u64, TaskView>>,
    /// 任务 id → 取消令牌。`task_cancel` 命令按 id 找它，不暴露令牌本身。
    cancels: RwLock<HashMap<u64, Arc<CancelToken>>>,
    next: AtomicU64,
    /// 状态变化广播。Tauri 外壳把它转成 `task:state` 事件。
    tx: tokio::sync::broadcast::Sender<TaskView>,
}

impl TaskStore {
    pub fn new() -> Arc<Self> {
        let (tx, _) = tokio::sync::broadcast::channel(256);
        Arc::new(Self {
            inner: RwLock::new(HashMap::new()),
            cancels: RwLock::new(HashMap::new()),
            next: AtomicU64::new(1),
            tx,
        })
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<TaskView> {
        self.tx.subscribe()
    }

    /// 登记取消令牌。返回 false 表示任务已结束（可能刚跑完）。
    pub fn register_cancel(&self, id: u64, token: Arc<CancelToken>) -> bool {
        let mut guard = self.cancels.write().unwrap();
        if !self.inner.read().unwrap().contains_key(&id) {
            return false;
        }
        guard.insert(id, token);
        true
    }

    /// 取消任务。返回 false 表示任务不存在或已结束。
    /// **先置 Cancelling 再发信号**：用户点了取消必须立刻看见反馈。
    pub fn cancel_task(&self, id: u64) -> bool {
        let token = self.cancels.read().unwrap().get(&id).cloned();
        let Some(token) = token else { return false };
        let alive = self
            .inner
            .read()
            .unwrap()
            .get(&id)
            .map(|v| matches!(v.state, TaskState::Queued | TaskState::Running))
            .unwrap_or(false);
        if !alive {
            return false;
        }
        self.set(id, |v| v.state = TaskState::Cancelling);
        token.cancel();
        true
    }

    pub fn spawn(&self, capability: CapabilityId, name: impl Into<String>) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let view = TaskView {
            id,
            capability,
            name: name.into(),
            state: TaskState::Queued,
            progress: None,
            message: String::new(),
            queue_ms: None,
            rev: 1,
            queued_at: Some(std::time::Instant::now()),
        };
        self.inner.write().unwrap().insert(id, view.clone());
        let _ = self.tx.send(view);
        id
    }

    pub fn set(&self, id: u64, f: impl FnOnce(&mut TaskView)) {
        let mut guard = self.inner.write().unwrap();
        if let Some(view) = guard.get_mut(&id) {
            f(view);
            // queue_ms 打点：第一次转 Running 时结算排队耗时（TASK-CONCURRENCY §9）
            if view.state == TaskState::Running && view.queue_ms.is_none() {
                if let Some(at) = view.queued_at {
                    view.queue_ms = Some(at.elapsed().as_millis() as u64);
                }
            }
            view.rev += 1;
            let snapshot = view.clone();
            drop(guard);
            let _ = self.tx.send(snapshot);
        }
    }

    pub fn get(&self, id: u64) -> Option<TaskView> {
        self.inner.read().unwrap().get(&id).cloned()
    }

    pub fn list(&self) -> Vec<TaskView> {
        let mut rows: Vec<_> = self.inner.read().unwrap().values().cloned().collect();
        rows.sort_by_key(|v| std::cmp::Reverse(v.id));
        rows
    }

    /// 清理已结束的任务，返回清理条数。
    pub fn clear_finished(&self) -> usize {
        let mut guard = self.inner.write().unwrap();
        let before = guard.len();
        guard.retain(|_, v| {
            matches!(
                v.state,
                TaskState::Queued | TaskState::Running | TaskState::Cancelling
            )
        });
        let removed: Vec<u64> = {
            let alive: std::collections::HashSet<u64> = guard.keys().copied().collect();
            self.cancels
                .read()
                .unwrap()
                .keys()
                .copied()
                .filter(|id| !alive.contains(id))
                .collect()
        };
        let mut cancels = self.cancels.write().unwrap();
        for id in removed {
            cancels.remove(&id);
        }
        before - guard.len()
    }

    /// 长任务的统一入口：登记取消令牌 → Running → 执行 → 结束归类。
    ///
    /// work 返回 `Err(cancelled)` 记 Cancelled、`Err(conflict)` 记 Partial、
    /// 其余 Err 记 Failed、Ok 记 Done（最终文案用 `handle.message` 自己报）；
    /// work panic 由 JoinError 兜底记 Failed——**任务不会卡在 Running**。
    /// 取消只在 work 主动检查 `handle.cancel` 的安全点生效。
    pub fn spawn_managed<F, Fut>(
        self: &Arc<Self>,
        capability: CapabilityId,
        name: impl Into<String>,
        work: F,
    ) -> u64
    where
        F: FnOnce(TaskHandle) -> Fut + Send + 'static,
        Fut: Future<Output = crate::app::error::Result<()>> + Send + 'static,
    {
        let id = self.spawn(capability, name);
        let cancel = Arc::new(CancelToken::new());
        self.register_cancel(id, cancel.clone());
        let store = Arc::clone(self);
        tokio::task::spawn(async move {
            let handle = TaskHandle {
                id,
                cancel: cancel.clone(),
                store: store.clone(),
            };
            store.set(id, |v| v.state = TaskState::Running);
            // work 单独 spawn：它 panic 时 JoinError 在这里被接住归类
            let outcome = tokio::task::spawn(work(handle)).await;
            // 失败时把原因带上：Err(e) 的文案、panic 的 JoinError 描述
            let message = match &outcome {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.message.clone()),
                Err(e) => Some(format!("任务崩溃：{e}")),
            };
            let state = settle(outcome);
            store.set(id, |v| {
                v.state = state;
                v.progress = None;
                if let Some(message) = message {
                    v.message = message;
                }
            });
        });
        id
    }
}

/// 交给业务代码的句柄：上报进度 + 检查取消。业务拿不到 TaskStore，改不了别人的任务。
#[derive(Clone)]
pub struct TaskHandle {
    pub id: u64,
    pub cancel: Arc<CancelToken>,
    store: Arc<TaskStore>,
}

impl TaskHandle {
    pub fn progress(&self, value: f32, message: impl Into<String>) {
        let msg = message.into();
        self.store.set(self.id, |v| {
            v.progress = Some(value.clamp(0.0, 1.0));
            v.message = msg;
        });
    }

    pub fn message(&self, message: impl Into<String>) {
        let msg = message.into();
        self.store.set(self.id, |v| v.message = msg);
    }

    /// 同步阻塞代码（rusqlite 事务循环）在安全点调用。
    pub fn checkpoint(&self) -> crate::app::error::Result<()> {
        self.cancel.checkpoint()
    }
}

/// 把 tokio 任务的结果归到七态之一。**取消不能被记成失败。**
pub fn settle(outcome: Result<crate::app::error::Result<()>, tokio::task::JoinError>) -> TaskState {
    use crate::app::error::ErrorKind;
    match outcome {
        Ok(Ok(())) => TaskState::Done,
        Ok(Err(e)) if e.is_cancelled() => TaskState::Cancelled,
        Ok(Err(e)) if e.kind == ErrorKind::Conflict => TaskState::Partial,
        Ok(Err(_)) => TaskState::Failed,
        Err(e) if e.is_panic() => TaskState::Failed,
        Err(_) => TaskState::Cancelled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_token_reports_state() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
        assert!(token.checkpoint().is_ok());
        token.cancel();
        assert!(token.is_cancelled());
        assert!(token.checkpoint().is_err(), "取消后 checkpoint 必须报错");
    }

    #[test]
    fn store_lifecycle_and_clear() {
        let store = TaskStore::new();
        let id = store.spawn(CapabilityId::Sessions, "扫描会话");
        assert_eq!(store.get(id).unwrap().state, TaskState::Queued);
        store.set(id, |v| v.state = TaskState::Running);
        assert_eq!(store.get(id).unwrap().state, TaskState::Running);
        assert_eq!(store.get(id).unwrap().rev, 2, "每次 set 都要推进 rev");

        store.set(id, |v| v.state = TaskState::Done);
        assert_eq!(store.clear_finished(), 1, "已结束的任务应被清理");
        assert!(store.get(id).is_none());
    }

    #[test]
    fn cancelled_is_not_failed() {
        // 契约：主动取消不得被记成失败，否则会污染错误统计
        let err = crate::app::error::AppError::new(ErrorKind::Cancelled, "用户取消");
        assert_eq!(settle(Ok(Err(err))), TaskState::Cancelled);
        assert_eq!(settle(Ok(Ok(()))), TaskState::Done);
    }

    /// panic 的任务必须被记成 Failed，不能永远卡在 Running。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn managed_task_survives_panic() {
        let store = TaskStore::new();
        let id = store.spawn_managed(CapabilityId::Sessions, "会崩的任务", |_h| async {
            panic!("业务崩了");
            #[allow(unreachable_code)]
            Ok(())
        });
        for _ in 0..200 {
            if store.get(id).map(|v| v.state) == Some(TaskState::Failed) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let view = store.get(id).unwrap();
        assert_eq!(view.state, TaskState::Failed);
        assert!(view.message.contains("任务崩溃"), "失败原因要写进 message");
    }

    /// work 主动响应取消令牌时，最终态必须是 Cancelled 而不是 Failed。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn managed_task_cancel_maps_to_cancelled() {
        let store = TaskStore::new();
        let id = store.spawn_managed(CapabilityId::Sessions, "可取消任务", |h| async move {
            h.cancel.cancelled().await;
            Err(crate::app::error::AppError::new(
                ErrorKind::Cancelled,
                "用户取消",
            ))
        });
        // 等它转 Running 再取消：太早点击会被「取消只在安全点生效」语义吞掉
        for _ in 0..200 {
            if store.get(id).map(|v| v.state) == Some(TaskState::Running) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(store.cancel_task(id));
        for _ in 0..200 {
            if store.get(id).map(|v| v.state) == Some(TaskState::Cancelled) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(store.get(id).unwrap().state, TaskState::Cancelled);
    }

    /// 取消必须按任务隔离：停掉 A 不能把 B 一起停掉。
    #[test]
    fn cancel_is_per_task() {
        let a = Arc::new(CancelToken::new());
        let b = Arc::new(CancelToken::new());
        a.cancel();
        assert!(a.is_cancelled());
        assert!(!b.is_cancelled(), "取消 A 不得影响 B");
        assert!(b.checkpoint().is_ok());
    }
}
