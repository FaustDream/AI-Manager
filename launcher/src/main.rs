//! AI Manager 启动器：托盘常驻的小程序，只管后端服务的生命周期。
//!
//! 职责边界（核心原则）：不承载业务逻辑、不参与界面命令面，只做
//! 拉起 / 停止 / 重启 / 状态呈现。原 start.cmd 的「最小化控制台当日志窗口」
//! 由状态窗口接替：后端 stdout/stderr 管道捕获 + app.jsonl 增量 tail。
//!
//! 线程模型：
//! - UI 线程：消息泵 + 托盘/菜单事件 + 状态窗口，250ms 心跳消化全部事件；
//! - 编排线程：唯一持有子进程，串行执行启动/停止/重启；
//! - 两个管道读线程：后端 stdout/stderr 行级转发。

// 窗口子系统：双击启动不弹控制台（后端才是控制台子系统，契约 19 与它无关）。
// 所有输出都进状态窗口，GUI 子系统下没有 stdout 可写也不会阻塞。
#![windows_subsystem = "windows"]

mod autostart;
mod backend;
mod icons;
mod logsink;
mod orchestrator;
mod state;
mod tray;
mod win;

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, IsIconic, MessageBoxW, PostQuitMessage, SetForegroundWindow, ShowWindow,
    MB_ICONERROR, MB_OK, SW_RESTORE, SW_SHOW,
};

use crate::orchestrator::{Cmd, Event};
use crate::state::Phase;
use crate::win::Hooks;

const LAUNCHER_MUTEX: &str = "Local\\AIManager.Launcher";
/// 环形缓冲上限：控制台 2000 行 / 业务日志 500 行
const CONSOLE_CAP: usize = 2000;
const JSONL_CAP: usize = 500;

fn main() {
    unsafe { run() };
}

unsafe fn run() {
    let mutex = acquire_launcher_mutex();
    if matches!(mutex, Acquire::AlreadyRunning) {
        focus_existing_window();
        return;
    }
    // Failed = 锁拿不到但不硬退（与后端单实例锁同口径：留痕继续跑）
    let _keep = mutex;

    let (ev_tx, ev_rx) = std::sync::mpsc::channel::<Event>();
    let cmd_tx = orchestrator::spawn(ev_tx);

    let tray = match tray::Tray::build() {
        Ok(tray) => tray,
        Err(e) => {
            fatal(&e);
            return;
        }
    };
    let widgets = match win::create() {
        Ok(widgets) => widgets,
        Err(e) => {
            fatal(&e);
            return;
        }
    };
    win::set_hooks(Hooks {
        tick: Some(Box::new(dispatch_tick)),
        close: Some(Box::new(dispatch_close)),
        session_end: Some(Box::new(dispatch_session_end)),
        layout: Some(Box::new(dispatch_layout)),
    });

    // 启动即拉起服务（复刻 start.cmd 的默认行为）；端口已被占用时
    // 编排线程会把它识别为外部实例，不会重复启动。
    let _ = cmd_tx.send(Cmd::Start);

    let app = App::new(cmd_tx, ev_rx, tray, widgets);
    APP.with(|slot| *slot.borrow_mut() = Some(Box::new(app)));

    win::pump_until_quit();
    // 托盘图标随 Tray drop 移除；单实例锁随句柄 drop 释放
}

enum Acquire {
    /// 持有句柄活到进程结束：提前 Drop 会放进第二个实例
    Acquired(#[allow(dead_code)] HANDLE),
    AlreadyRunning,
    Failed,
}

unsafe fn acquire_launcher_mutex() -> Acquire {
    let name = HSTRING::from(LAUNCHER_MUTEX);
    match CreateMutexW(None, false, &name) {
        Ok(handle) => {
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(handle);
                Acquire::AlreadyRunning
            } else {
                Acquire::Acquired(handle)
            }
        }
        Err(_) => Acquire::Failed,
    }
}

/// 二次启动：唤出已有状态窗口后退出，不出现第二个托盘图标。
unsafe fn focus_existing_window() {
    if let Ok(hwnd) = FindWindowW(None, w!("AI Manager 启动器")) {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        } else {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        let _ = SetForegroundWindow(hwnd);
    }
}

unsafe fn fatal(message: &str) {
    MessageBoxW(
        None,
        &HSTRING::from(message),
        w!("AI Manager 启动器"),
        MB_ICONERROR | MB_OK,
    );
}

// ---------------------------------------------------------------- App

struct App {
    cmd_tx: Sender<Cmd>,
    ev_rx: Receiver<Event>,
    tray: tray::Tray,
    widgets: win::Widgets,
    phase: Phase,
    detail: String,
    since_running: Option<Instant>,
    console: logsink::Ring,
    jsonl_ring: logsink::Ring,
    tailer: logsink::JsonlTail,
    tail_tick: u32,
    status_tick: u32,
}

thread_local! {
    static APP: RefCell<Option<Box<App>>> = const { RefCell::new(None) };
}

pub(crate) fn dispatch_tick() {
    APP.with(|slot| {
        if let Ok(mut guard) = slot.try_borrow_mut() {
            if let Some(app) = guard.as_mut() {
                unsafe { app.tick() };
            }
        }
    });
}

pub(crate) fn dispatch_close() {
    APP.with(|slot| {
        if let Ok(mut guard) = slot.try_borrow_mut() {
            if let Some(app) = guard.as_mut() {
                unsafe { win::hide(&app.widgets) };
            }
        }
    });
}

pub(crate) fn dispatch_session_end() {
    APP.with(|slot| {
        if let Ok(mut guard) = slot.try_borrow_mut() {
            if let Some(app) = guard.as_mut() {
                app.session_end();
            }
        }
    });
}

pub(crate) fn dispatch_layout(cx: i32, cy: i32) {
    APP.with(|slot| {
        if let Ok(mut guard) = slot.try_borrow_mut() {
            if let Some(app) = guard.as_mut() {
                win::layout(&app.widgets, cx, cy);
            }
        }
    });
}

impl App {
    fn new(
        cmd_tx: Sender<Cmd>,
        ev_rx: Receiver<Event>,
        tray: tray::Tray,
        widgets: win::Widgets,
    ) -> App {
        let mut app = App {
            cmd_tx,
            ev_rx,
            tray,
            widgets,
            phase: Phase::Stopped,
            detail: String::new(),
            since_running: None,
            console: logsink::Ring::new(CONSOLE_CAP),
            jsonl_ring: logsink::Ring::new(JSONL_CAP),
            tailer: logsink::JsonlTail::open(PathBuf::new()),
            tail_tick: 0,
            status_tick: 0,
        };
        unsafe {
            app.push_console(format!(
                "AI Manager 启动器 v{} 就绪。右键托盘图标可启动 / 关闭 / 重启项目。",
                env!("CARGO_PKG_VERSION")
            ));
            match backend::resolve() {
                Ok(layout) => {
                    app.push_console(format!(
                        "后端落点：{}（{:?}布局）",
                        layout.exe.display(),
                        layout.kind
                    ));
                    app.tailer = logsink::JsonlTail::open(layout.app_jsonl());
                    let seed = app.tailer.seed(300);
                    if seed.is_empty() {
                        app.push_console(
                            "业务日志：data/logs/app.jsonl 暂无内容，后端启动后自动跟随。",
                        );
                    } else {
                        for line in seed {
                            app.jsonl_ring.push(line);
                        }
                        win::set_text(app.widgets.jsonl, &app.jsonl_ring.render());
                    }
                }
                Err(e) => {
                    app.push_console(e);
                }
            }
        }
        app
    }

    unsafe fn tick(&mut self) {
        // 菜单事件 → 编排命令 / 窗口动作
        while let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            match event.id.0.as_str() {
                "start" => {
                    let _ = self.cmd_tx.send(Cmd::Start);
                }
                "stop" => {
                    let _ = self.cmd_tx.send(Cmd::Stop);
                }
                "restart" => {
                    let _ = self.cmd_tx.send(Cmd::Restart);
                }
                "open" => {
                    if let Err(e) = backend::open_ui() {
                        self.push_console(format!("打开界面失败：{e}"));
                    }
                }
                "logs" => self.open_logs(),
                "autostart" => self.toggle_autostart(),
                "quit" => {
                    let _ = self.cmd_tx.send(Cmd::Shutdown);
                }
                _ => {}
            }
        }
        // 托盘左键 = 查看状态（右键菜单由 tray-icon 处理）
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                win::show(&self.widgets);
            }
        }
        // 编排线程的结论
        while let Ok(event) = self.ev_rx.try_recv() {
            match event {
                Event::Phase { phase, detail } => self.on_phase(phase, detail),
                Event::Console(line) => self.push_console(line),
                Event::QuitNow => {
                    PostQuitMessage(0);
                    return;
                }
            }
        }
        // 业务调用链日志：1s 一轮增量
        self.tail_tick += 1;
        if self.tail_tick.is_multiple_of(4) {
            let lines = self.tailer.poll();
            if !lines.is_empty() {
                let mut trimmed = false;
                for line in &lines {
                    trimmed |= self.jsonl_ring.push(line.clone());
                }
                if trimmed {
                    win::set_text(self.widgets.jsonl, &self.jsonl_ring.render());
                } else {
                    for line in &lines {
                        win::append_line(self.widgets.jsonl, line);
                    }
                }
            }
        }
        // 运行时长每 2s 刷新一次
        self.status_tick += 1;
        if self.status_tick.is_multiple_of(8)
            && matches!(self.phase, Phase::Running { owned: true })
        {
            self.status_refresh();
        }
    }

    unsafe fn on_phase(&mut self, phase: Phase, detail: String) {
        self.phase = phase;
        self.detail = detail.clone();
        self.since_running = match phase {
            Phase::Running { owned: true } => Some(Instant::now()),
            _ => None,
        };
        self.tray.apply(phase, &detail);
        self.status_refresh();
    }

    unsafe fn push_console(&mut self, line: impl Into<String>) {
        let line = line.into();
        let trimmed = self.console.push(line.clone());
        if trimmed {
            win::set_text(self.widgets.console, &self.console.render());
        } else {
            win::append_line(self.widgets.console, &line);
        }
    }

    unsafe fn status_refresh(&self) {
        win::set_text(self.widgets.status, &self.status_text());
    }

    fn status_text(&self) -> String {
        let mut text = format!("{} · {}", self.phase.label(), self.detail);
        if matches!(self.phase, Phase::Running { owned: true }) {
            if let Some(since) = self.since_running {
                let secs = since.elapsed().as_secs();
                let uptime = if secs < 60 {
                    format!("{secs} 秒")
                } else {
                    format!("{} 分 {} 秒", secs / 60, secs % 60)
                };
                text.push_str(&format!(" · 已运行 {uptime}"));
            }
        }
        text
    }

    unsafe fn open_logs(&mut self) {
        match backend::resolve() {
            Ok(layout) => {
                let _ = std::fs::create_dir_all(layout.logs_dir());
                let dir = layout.logs_dir().display().to_string();
                if let Err(e) = backend::open_target(&dir) {
                    self.push_console(format!("打开日志目录失败：{e}"));
                }
            }
            Err(e) => self.push_console(e),
        }
    }

    unsafe fn toggle_autostart(&mut self) {
        let on = !autostart::enabled();
        match autostart::set(on) {
            Ok(()) => {
                self.tray.autostart_item.set_checked(on);
                self.push_console(format!(
                    "开机自启已{}（HKCU Run · AIManagerLauncher）",
                    if on { "开启" } else { "关闭" }
                ));
            }
            Err(e) => {
                self.tray.autostart_item.set_checked(autostart::enabled());
                self.push_console(format!("开机自启设置失败：{e}"));
            }
        }
    }

    /// 注销/关机：按既定策略停掉自己拉起的服务（尽力而为，时间有限）。
    fn session_end(&mut self) {
        let _ = self.cmd_tx.send(Cmd::Shutdown);
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            match self.ev_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Event::QuitNow) | Err(_) => break,
                Ok(_) => {}
            }
        }
    }
}
