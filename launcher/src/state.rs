//! 启动器状态机：托盘图标、菜单可用性与状态行都由它驱动。
//!
//! 状态迁移在编排线程（orchestrator）里执行；本模块只定义相位与
//! 「这个相位下允许什么动作」，保证菜单置灰和图标换色有唯一依据。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// 后端没在跑（或刚被本启动器停止）
    Stopped,
    /// 正在拉起、等端口就绪
    Starting,
    /// 端口就绪。owned = 是本启动器拉起的子进程（可强杀）；外部启动的只走优雅退出
    Running { owned: bool },
    /// 已通知退出，等进程消失与端口释放
    Stopping,
    /// 启动失败 / 后端异常退出
    Failed,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Stopped => "已停止",
            Phase::Starting => "启动中",
            Phase::Running { owned: true } => "运行中",
            Phase::Running { owned: false } => "运行中（外部启动）",
            Phase::Stopping => "停止中",
            Phase::Failed => "启动失败",
        }
    }

    /// 托盘图标档位：运行（绿）/ 切换中（黄）/ 失败（红）/ 停止（灰环）
    pub fn icon_slot(self) -> IconSlot {
        match self {
            Phase::Running { .. } => IconSlot::Running,
            Phase::Starting | Phase::Stopping => IconSlot::Busy,
            Phase::Failed => IconSlot::Failed,
            Phase::Stopped => IconSlot::Stopped,
        }
    }

    pub fn can_start(self) -> bool {
        matches!(self, Phase::Stopped | Phase::Failed)
    }

    pub fn can_stop(self) -> bool {
        matches!(self, Phase::Running { .. })
    }

    pub fn can_restart(self) -> bool {
        matches!(self, Phase::Running { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconSlot {
    Running,
    Busy,
    Stopped,
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_availability_follows_phase() {
        // 停止/失败态只能「启动」
        assert!(Phase::Stopped.can_start());
        assert!(Phase::Failed.can_start());
        assert!(!Phase::Stopped.can_stop());
        assert!(!Phase::Failed.can_restart());

        // 运行态（含外部启动）可停可重启，但不能再启动一次
        for phase in [
            Phase::Running { owned: true },
            Phase::Running { owned: false },
        ] {
            assert!(phase.can_stop());
            assert!(phase.can_restart());
            assert!(!phase.can_start());
        }

        // 过渡态什么都不要做，避免重入
        for phase in [Phase::Starting, Phase::Stopping] {
            assert!(!phase.can_start());
            assert!(!phase.can_stop());
            assert!(!phase.can_restart());
        }
    }

    #[test]
    fn icon_slot_reflects_visibility() {
        assert_eq!(
            Phase::Running { owned: false }.icon_slot(),
            IconSlot::Running
        );
        assert_eq!(Phase::Starting.icon_slot(), IconSlot::Busy);
        assert_eq!(Phase::Stopping.icon_slot(), IconSlot::Busy);
        assert_eq!(Phase::Failed.icon_slot(), IconSlot::Failed);
        assert_eq!(Phase::Stopped.icon_slot(), IconSlot::Stopped);
    }
}
