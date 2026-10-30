//! 资源租约：按**物理资源**加锁，不是按功能。
//!
//! v1 的全局串行池把「写 mcp.json」和「写 accounts.dat」也互斥掉了，
//! 而两者毫无交集。这里改成按资源粒度，只有真冲突才等待。
//!
//! **死锁免疫靠两点**（不是靠纪律）：
//!   1. keys 排序后一次性申请 → 所有任务按同一全序，等待图无环
//!   2. 租约是 RAII，panic 也会释放

use crate::app::error::{AppError, Result};
use crate::contracts::TargetKey;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ResourceKey {
    // L0 全局一致视图（备份 / 迁移要求跨表快照一致）
    GlobalStore,
    // L1 应用内
    AppDbWrite,
    CatalogDb,
    AccountStore,
    SkillLibrary,
    // L2 外部资源（按路径）
    ClientDb(Arc<str>),
    Profile(Arc<str>),
    // L3 平台目标
    TargetMcp(TargetKey),
    TargetSkillDir(TargetKey),
    // L4 单账号（最细，绝大多数真冲突在这一层）
    Account(Arc<str>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

/// 租约。Drop 即释放。
pub struct Lease {
    keys: Vec<ResourceKey>,
    registry: Arc<ResourceRegistry>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.registry.release(&self.keys);
    }
}

impl Lease {
    pub fn keys(&self) -> &[ResourceKey] {
        &self.keys
    }
}

pub struct ResourceRegistry {
    /// 当前被持有的资源。用同步 Mutex：临界区极短且不能跨 await。
    held: Mutex<HashMap<ResourceKey, Access>>,
}

impl ResourceRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { held: Mutex::new(HashMap::new()) })
    }

    /// 申请租约。等待发生在 async 侧（sleep 而非阻塞线程）。
    /// 调用方须包 `timeout`，否则 bug 会表现为「界面永远转圈」。
    pub async fn lease(self: &Arc<Self>, reqs: &[(ResourceKey, Access)]) -> Result<Lease> {
        self.lease_inner(reqs, None).await
    }

    pub async fn lease_with_timeout(
        self: &Arc<Self>,
        reqs: &[(ResourceKey, Access)],
        timeout: Duration,
    ) -> Result<Lease> {
        self.lease_inner(reqs, Some(timeout)).await
    }

    async fn lease_inner(
        self: &Arc<Self>,
        reqs: &[(ResourceKey, Access)],
        timeout: Option<Duration>,
    ) -> Result<Lease> {
        let keys = Self::normalize(reqs);
        if keys.is_empty() {
            return Ok(Lease { keys: Vec::new(), registry: self.clone() });
        }
        let deadline = timeout.map(|t| std::time::Instant::now() + t);

        loop {
            {
                // guard 在这个块结束时 drop，绝不会跨 await
                let mut held = self
                    .held
                    .lock()
                    .map_err(|_| AppError::new("io", "资源表锁被污染"))?;
                if keys.iter().all(|(k, _)| !held.contains_key(k)) {
                    for (k, access) in &keys {
                        held.insert(k.clone(), *access);
                    }
                    return Ok(Lease { keys: keys.into_iter().map(|(k, _)| k).collect(), registry: self.clone() });
                }
            }

            if let Some(deadline) = deadline {
                if std::time::Instant::now() >= deadline {
                    return Err(AppError::new("invalid", "等待资源超时（可能有任务持锁未释放）"));
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// 非阻塞探测。用于「可选优化路径」，绝不用于等待。
    pub fn try_lease(self: &Arc<Self>, reqs: &[(ResourceKey, Access)]) -> Result<Option<Lease>> {
        let keys = Self::normalize(reqs);
        let mut held = self.held.lock().map_err(|_| AppError::new("io", "资源表锁被污染"))?;
        if keys.iter().all(|(k, _)| !held.contains_key(k)) {
            for (k, access) in &keys {
                held.insert(k.clone(), *access);
            }
            drop(held);
            Ok(Some(Lease {
                keys: keys.into_iter().map(|(k, _)| k).collect(),
                registry: self.clone(),
            }))
        } else {
            Ok(None)
        }
    }

    /// 当前等待中的资源数，用于观测「锁是不是太粗了」。
    pub fn held_count(&self) -> usize {
        self.held.lock().map(|g| g.len()).unwrap_or(0)
    }

    fn release(&self, keys: &[ResourceKey]) {
        if let Ok(mut held) = self.held.lock() {
            for k in keys {
                held.remove(k);
            }
        }
    }

    /// 去重 + 同 key 取最强权限 + 排序（死锁免疫的关键一行）
    fn normalize(reqs: &[(ResourceKey, Access)]) -> Vec<(ResourceKey, Access)> {
        let mut map: HashMap<ResourceKey, Access> = HashMap::new();
        for (key, access) in reqs {
            let entry = map.entry(key.clone()).or_insert(*access);
            if *access == Access::Write {
                *entry = Access::Write;
            }
        }
        let mut keys: Vec<_> = map.into_iter().collect();
        keys.sort_by(|a, b| a.0.cmp(&b.0));
        keys
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> ResourceKey {
        ResourceKey::Account(Arc::from(s))
    }

    #[test]
    fn different_resources_do_not_block_each_other() {
        let registry = ResourceRegistry::new();
        let a = registry
            .try_lease(&[(key("qoder:1"), Access::Write)])
            .unwrap()
            .expect("首个租约应成功");
        // 不同账号 → 无交集，必须能同时拿到
        let b = registry
            .try_lease(&[(key("qoder:2"), Access::Write)])
            .unwrap()
            .expect("不同资源不该互斥");
        assert_eq!(registry.held_count(), 2);
        drop(a);
        drop(b);
        assert_eq!(registry.held_count(), 0, "Drop 必须释放");
    }

    #[test]
    fn same_resource_is_exclusive() {
        let registry = ResourceRegistry::new();
        let _a = registry.try_lease(&[(key("qoder:1"), Access::Write)]).unwrap().unwrap();
        assert!(
            registry.try_lease(&[(key("qoder:1"), Access::Write)]).unwrap().is_none(),
            "同一资源必须互斥"
        );
    }

    #[test]
    fn normalize_sorts_and_keeps_strongest_access() {
        let reqs = vec![
            (key("b"), Access::Write),
            (key("a"), Access::Read),
            (key("b"), Access::Read),
        ];
        let out = ResourceRegistry::normalize(&reqs);
        assert_eq!(out.len(), 2, "同 key 应去重");
        assert_eq!(out[0].0, key("a"), "必须排序，这是死锁免疫的关键");
        let b = &out.iter().find(|(k, _)| *k == key("b")).unwrap().1;
        assert_eq!(*b, Access::Write, "同 key 多次出现取最强权限");
    }
}
