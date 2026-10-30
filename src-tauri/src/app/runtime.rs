//! tokio 运行时。参数按 `docs/TASK-CONCURRENCY.md` §3 定死，不要凭感觉调。

use std::sync::OnceLock;
use std::time::Duration;

/// async 侧只做 IO 协调（等锁、等网络、等配额），4 个 worker 足够。
pub const WORKER_THREADS: usize = 4;
/// 默认是 512 且队列无界：超出上限不是排队报错，而是继续开线程直到 512，
/// 会把「某能力忘了申请配额」这个 bug 掩盖成偶发的 database is locked。
pub const MAX_BLOCKING_THREADS: usize = 16;
/// 空闲后阻塞线程退到 0，长跑时不占内存。
pub const KEEP_ALIVE_SECS: u64 = 10;

static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

pub fn runtime() -> &'static tokio::runtime::Runtime {
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(WORKER_THREADS)
            .max_blocking_threads(MAX_BLOCKING_THREADS)
            .thread_keep_alive(Duration::from_secs(KEEP_ALIVE_SECS))
            .thread_name("aim-task")
            .enable_all()
            .build()
            .expect("tokio runtime 初始化失败")
    })
}

/// 同步阻塞调用（rusqlite、文件写入、DPAPI）必须走这里，
/// 直接调用会阻塞 WebView 的命令通道，表现为界面卡死而非报错。
pub async fn blocking<F, T>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    runtime().spawn_blocking(f).await.expect("阻塞任务 panic")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_can_spawn_and_run_blocking() {
        let handle = runtime();
        // 在运行时内部取结果，避免嵌套 runtime 导致 panic
        let value: i32 = handle.block_on(async {
            blocking(|| 6 * 7).await
        });
        assert_eq!(value, 42, "spawn_blocking 应能执行同步闭包并返回结果");
    }

    #[test]
    fn blocking_pool_budget_matches_capability_quota() {
        // 配额合计 8（3+2+1+1+1），阻塞池必须 ≥ 配额 × 2，否则会互相饿死
        let quota_total: usize = 3 + 2 + 1 + 1 + 1;
        assert_eq!(quota_total, 8);
        assert!(
            MAX_BLOCKING_THREADS >= quota_total * 2,
            "阻塞线程数需 ≥ 配额合计 × 2"
        );
    }
}
