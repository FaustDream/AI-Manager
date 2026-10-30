//! 任务调度层。三层正交隔离（详见 `docs/TASK-CONCURRENCY.md`）：
//!   ① 能力配额：A 占满自身份额，抢不走 B 的线程
//!   ② 资源租约：A 只锁它真正写的资源
//!   ③ 取消令牌：长任务可停，写路径只压到事务边界
//!
//! **硬规则：等待不占线程。** 等配额、等锁全发生在 async 侧；
//! `spawn_blocking` 只执行「已拿到全部资源」的就绪工作。

pub mod resources;

use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use tokio::sync::{watch, Semaphore};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityId {
    AccountCenter,
    Sessions,
    Hub,
    Maintenance,
    Settings,
}

pub struct Capability {
    pub id: CapabilityId,
    pub title: &'static str,
    pub quota: usize,
}

/// 加能力 = 这张表加一行。调度核心不认识任何业务含义。
pub static CAPABILITIES: &[Capability] = &[
    Capability { id: CapabilityId::AccountCenter, title: "账号中心", quota: 3 },
    Capability { id: CapabilityId::Sessions, title: "会话管理", quota: 2 },
    Capability { id: CapabilityId::Hub, title: "技能与 MCP", quota: 1 },
    Capability { id: CapabilityId::Maintenance, title: "备份与清理", quota: 1 },
    Capability { id: CapabilityId::Settings, title: "设置与迁移", quota: 1 },
];

pub fn quota_of(id: CapabilityId) -> usize {
    CAPABILITIES.iter().find(|c| c.id == id).map(|c| c.quota).unwrap_or(1)
}

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

#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
    pub id: u64,
    pub capability: CapabilityId,
    pub name: String,
    pub state: TaskState,
    pub progress: Option<f32>,
    pub message: String,
    /// 单调递增，前端据此丢弃过期帧
    pub rev: u64,
}

/// 能力配额。每个能力一把信号量，互不相干。
pub struct Quotas {
    sem: HashMap<CapabilityId, Arc<Semaphore>>,
}

impl Quotas {
    pub fn new() -> Self {
        let sem = CAPABILITIES
            .iter()
            .map(|c| (c.id, Arc::new(Semaphore::new(c.quota))))
            .collect();
        Self { sem }
    }

    /// 等待发生在 async 侧，不占线程。
    pub async fn acquire(&self, id: CapabilityId) -> tokio::sync::SemaphorePermit<'_> {
        let sem = self.sem.get(&id).expect("未知能力");
        sem.acquire().await.expect("信号量被关闭")
    }

    pub fn available(&self, id: CapabilityId) -> usize {
        self.sem.get(&id).map(|s| s.available_permits()).unwrap_or(0)
    }
}

impl Default for Quotas {
    fn default() -> Self {
        Self::new()
    }
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
            Err(crate::app::error::AppError::new("cancelled", "任务已取消"))
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
            rev: 1,
        };
        self.inner.write().unwrap().insert(id, view.clone());
        let _ = self.tx.send(view);
        id
    }

    pub fn set(&self, id: u64, f: impl FnOnce(&mut TaskView)) {
        let mut guard = self.inner.write().unwrap();
        if let Some(view) = guard.get_mut(&id) {
            f(view);
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
        rows.sort_by(|a, b| b.id.cmp(&a.id));
        rows
    }

    /// 清理已结束的任务，返回清理条数。
    pub fn clear_finished(&self) -> usize {
        let mut guard = self.inner.write().unwrap();
        let before = guard.len();
        guard.retain(|_, v| {
            matches!(v.state, TaskState::Queued | TaskState::Running | TaskState::Cancelling)
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

/// 任务执行环境：配额 / 租约 / 取消三层的唯一入口。
///
/// 顺序是刻意的：**先排队拿配额，再等资源，最后才 spawn_blocking**。
/// 等待全部在 async 侧，阻塞池只跑「资源已就绪」的工作——这是死锁免疫的地基。
pub struct TaskEnv {
    pub store: Arc<TaskStore>,
    pub quotas: Quotas,
    pub resources: Arc<resources::ResourceRegistry>,
}

impl TaskEnv {
    pub fn new(store: Arc<TaskStore>, resources: Arc<resources::ResourceRegistry>) -> Self {
        Self { store, quotas: Quotas::new(), resources }
    }

    pub async fn run<F>(
        &self,
        capability: CapabilityId,
        name: impl Into<String>,
        reqs: &[(resources::ResourceKey, resources::Access)],
        cancel: Arc<CancelToken>,
        work: F,
    ) -> TaskState
    where
        F: FnOnce(TaskHandle) -> crate::app::error::Result<()> + Send + 'static,
    {
        let id = self.store.spawn(capability, name);
        self.store.register_cancel(id, cancel.clone());
        let handle = TaskHandle { id, cancel: cancel.clone(), store: self.store.clone() };

        // ① 配额：能力之间互不侵占
        let _permit = self.quotas.acquire(capability).await;

        // 排队期间被取消 → 直接结束，连资源都不申请
        if cancel.is_cancelled() {
            self.store.set(id, |v| v.state = TaskState::Cancelled);
            return TaskState::Cancelled;
        }
        self.store.set(id, |v| v.state = TaskState::Running);

        // ② 资源：超时是强制项，否则 bug 会表现为「界面永远转圈」
        let lease = match self.resources.lease_with_timeout(reqs, std::time::Duration::from_secs(60)).await {
            Ok(lease) => lease,
            Err(e) => {
                let state = if e.is_cancelled() { TaskState::Cancelled } else { TaskState::Failed };
                self.store.set(id, |v| {
                    v.state = state;
                    v.message = e.message.clone();
                });
                return state;
            }
        };

        // ③ 执行：阻塞工作进 blocking 池，租约随任务结束释放
        let outcome = tokio::task::spawn_blocking(move || {
            let result = work(handle);
            drop(lease);
            result
        })
        .await;

        let state = settle(outcome);
        self.store.set(id, |v| {
            v.state = state;
            v.progress = None;
        });
        state
    }
}

/// 把 tokio 任务的结果归到七态之一。**取消不能被记成失败。**
pub fn settle(
    outcome: Result<crate::app::error::Result<()>, tokio::task::JoinError>,
) -> TaskState {
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
    use crate::app::runtime::MAX_BLOCKING_THREADS;

    #[test]
    fn quota_budget_matches_blocking_pool() {
        let total: usize = CAPABILITIES.iter().map(|c| c.quota).sum();
        assert_eq!(total, 8, "配额合计应为 8（3+2+1+1+1）");
        assert!(MAX_BLOCKING_THREADS >= total * 2, "阻塞线程需 ≥ 配额合计 × 2");
    }

    #[test]
    fn quotas_are_independent_per_capability() {
        let quotas = Quotas::new();
        assert_eq!(quotas.available(CapabilityId::AccountCenter), 3);
        assert_eq!(quotas.available(CapabilityId::Hub), 1);
    }

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
        let err = crate::app::error::AppError::new("cancelled", "用户取消");
        assert_eq!(settle(Ok(Err(err))), TaskState::Cancelled);
        assert_eq!(settle(Ok(Ok(()))), TaskState::Done);
    }

    /// 用户问题的直接断言：A 能力占满自己的配额，不能影响 B 能力。
    #[tokio::test]
    async fn exhausted_capability_does_not_block_another() {
        use std::time::Duration;
        let quotas = Quotas::new();

        let mut held = Vec::new();
        for _ in 0..3 {
            held.push(quotas.acquire(CapabilityId::AccountCenter).await);
        }
        assert_eq!(quotas.available(CapabilityId::AccountCenter), 0, "3 个槽位应被占满");
        assert_eq!(quotas.available(CapabilityId::Sessions), 2, "会话的配额不受牵连");

        let got = tokio::time::timeout(Duration::from_millis(50), quotas.acquire(CapabilityId::Sessions)).await;
        assert!(got.is_ok(), "账号中心排队时，会话任务必须立刻拿到配额");
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

    /// 排队中被取消：连 work 都不该执行。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_while_queued_skips_work() {
        use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
        let store = TaskStore::new();
        let env = TaskEnv::new(store, resources::ResourceRegistry::new());
        let ran = Arc::new(AtomicBool::new(false));
        let flag = ran.clone();
        let cancel = Arc::new(CancelToken::new());
        cancel.cancel();

        let state = env
            .run(
                CapabilityId::Sessions,
                "扫会话",
                &[],
                cancel,
                move |_h| {
                    flag.store(true, AtomicOrdering::SeqCst);
                    Ok(())
                },
            )
            .await;

        assert_eq!(state, TaskState::Cancelled);
        assert!(!ran.load(AtomicOrdering::SeqCst), "已取消的排队任务不得执行 work");
    }
}
