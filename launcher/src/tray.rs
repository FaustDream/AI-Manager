//! 托盘与右键菜单。图标四态由 icons.rs 运行时绘制，菜单项可用性随状态机走。

use crate::autostart;
use crate::icons;
use crate::state::Phase;
use tray_icon::menu::accelerator::Accelerator;
use tray_icon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

pub struct Tray {
    pub tray: TrayIcon,
    pub start_item: MenuItem,
    pub stop_item: MenuItem,
    pub restart_item: MenuItem,
    pub autostart_item: CheckMenuItem,
    icons: Icons,
}

struct Icons {
    running: tray_icon::Icon,
    busy: tray_icon::Icon,
    stopped: tray_icon::Icon,
    failed: tray_icon::Icon,
}

impl Tray {
    pub fn build() -> Result<Tray, String> {
        let none: Option<Accelerator> = None;
        let start_item = MenuItem::with_id("start", "启动项目", true, none);
        let stop_item = MenuItem::with_id("stop", "关闭项目", false, none);
        let restart_item = MenuItem::with_id("restart", "重启项目", false, none);
        let open_item = MenuItem::with_id("open", "打开界面", true, none);
        let logs_item = MenuItem::with_id("logs", "打开日志目录", true, none);
        let autostart_item = CheckMenuItem::with_id(
            "autostart",
            "开机自动启动",
            true,
            autostart::enabled(),
            none,
        );
        let quit_item = MenuItem::with_id("quit", "退出管理程序", true, none);

        let menu = Menu::new();
        let _ = menu.append(&start_item);
        let _ = menu.append(&stop_item);
        let _ = menu.append(&restart_item);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&open_item);
        let _ = menu.append(&logs_item);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&autostart_item);
        let _ = menu.append(&quit_item);

        let icons = Icons {
            running: icons::filled([34, 197, 94]),
            busy: icons::filled([234, 179, 8]),
            failed: icons::filled([239, 68, 68]),
            stopped: icons::ring([113, 128, 150]),
        };

        // 左键留给「查看状态」，菜单固定走右键
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("AI Manager · 已停止")
            .with_menu_on_left_click(false)
            .with_menu_on_right_click(true)
            .with_icon(icons.stopped.clone())
            .build()
            .map_err(|e| format!("托盘创建失败：{e}"))?;
        let _ = tray.set_visible(true);

        Ok(Tray {
            tray,
            start_item,
            stop_item,
            restart_item,
            autostart_item,
            icons,
        })
    }

    /// 状态迁移后的托盘呈现：图标 + tooltip + 菜单可用性。
    pub fn apply(&self, phase: Phase, detail: &str) {
        let icon = match phase.icon_slot() {
            crate::state::IconSlot::Running => &self.icons.running,
            crate::state::IconSlot::Busy => &self.icons.busy,
            crate::state::IconSlot::Stopped => &self.icons.stopped,
            crate::state::IconSlot::Failed => &self.icons.failed,
        };
        let _ = self.tray.set_icon(Some(icon.clone()));
        let _ = self
            .tray
            .set_tooltip(Some(format!("AI Manager · {} · {detail}", phase.label())));
        self.start_item.set_enabled(phase.can_start());
        self.stop_item.set_enabled(phase.can_stop());
        self.restart_item.set_enabled(phase.can_restart());
    }
}
