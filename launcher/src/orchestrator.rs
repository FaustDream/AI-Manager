//! 后台编排线程：唯一持有后端子进程，启动/停止/重启串行执行。
//! 所有耗时操作（端口探测、等进程退出）都在本线程完成——UI 线程绝不阻塞。

use crate::backend::{self, WEB_PORT};
use crate::state::Phase;
use std::io::{BufRead, BufReader};
use std::process::Child;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// 就绪判定窗口：冷启动含 Windows Defender 扫描也要留够余量
const START_TIMEOUT: Duration = Duration::from_secs(30);
/// 优雅退出的等待上限
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
/// 启动器自身退出时的快速档：不让「退出管理程序」卡住
const QUICK_STOP_TIMEOUT: Duration = Duration::from_secs(4);
/// 退出后端口释放的确认窗口
const PORT_CLOSE_WAIT: Duration = Duration::from_secs(5);
/// 无命令时的自检节奏
const TICK: Duration = Duration::from_millis(250);

pub enum Cmd {
    Start,
    Stop,
    Restart,
    /// 退出启动器：按既定策略处理自己拉起的服务，然后通知 UI 收尾
    Shutdown,
}

pub enum Event {
    /// 状态迁移；detail 是给人看的一句话（含 PID / 原因）
    Phase { phase: Phase, detail: String },
    /// 运行控制台一行（后端输出或启动器动作）
    Console(String),
    /// 退出流程走完，UI 可以 PostQuitMessage
    QuitNow,
}

/// 启动编排线程，返回命令发送端。
pub fn spawn(events: Sender<Event>) -> Sender<Cmd> {
    let (cmd_tx, cmd_rx) = channel::<Cmd>();
    let _ = std::thread::Builder::new()
        .name("aim-launch-orch".into())
        .spawn(move || {
            Orch {
                events,
                cmd_rx,
                phase: Phase::Stopped,
                child: None,
                since: None,
            }
            .run();
        });
    cmd_tx
}

struct Orch {
    events: Sender<Event>,
    cmd_rx: Receiver<Cmd>,
    phase: Phase,
    child: Option<Child>,
    /// 本次 Running 的起点（owned 时才有意义）
    since: Option<Instant>,
}

impl Orch {
    fn run(mut self) {
        loop {
            match self.cmd_rx.recv_timeout(TICK) {
                Ok(Cmd::Start) => self.start(),
                Ok(Cmd::Stop) => self.stop(false),
                Ok(Cmd::Restart) => {
                    self.stop(false);
                    if self.phase.can_start() {
                        self.start();
                    } else {
                        self.console("重启中止：旧服务未能正常退出");
                    }
                }
                Ok(Cmd::Shutdown) => {
                    self.stop(true);
                    let _ = self.events.send(Event::QuitNow);
                    return;
                }
                Err(RecvTimeoutError::Timeout) => self.reconcile(),
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn set(&mut self, phase: Phase, detail: impl Into<String>) {
        self.phase = phase;
        let detail = detail.into();
        let _ = self.events.send(Event::Phase {
            phase,
            detail: detail.clone(),
        });
        self.console(detail);
    }

    fn console(&self, text: impl Into<String>) {
        let _ = self
            .events
            .send(Event::Console(format!("{} {}", now_hms(), text.into())));
    }

    fn start(&mut self) {
        if !self.phase.can_start() {
            return;
        }
        self.set(Phase::Starting, "解析后端落点…");
        let layout = match backend::resolve() {
            Ok(layout) => layout,
            Err(e) => {
                self.set(Phase::Failed, e);
                return;
            }
        };
        if !layout.dist_index.is_file() {
            let msg = format!(
                "前端产物缺失：{} 不存在。请先构建前端（npm run build）再启动服务。",
                layout.dist_index.display()
            );
            self.set(Phase::Failed, msg);
            return;
        }
        if backend::port_open(WEB_PORT) {
            // 已有实例在跑：不重复启动（单实例锁本就拒绝双开），只接管状态显示
            self.set(
                Phase::Running { owned: false },
                "端口已有实例在运行（非本启动器拉起，不重复启动）",
            );
            return;
        }
        let mut child = match backend::spawn_backend(&layout) {
            Ok(child) => child,
            Err(e) => {
                self.set(Phase::Failed, e);
                return;
            }
        };
        let pid = child.id();
        self.attach_output(&mut child);
        self.set(Phase::Starting, format!("PID {pid} · 等待端口就绪…"));
        let started = Instant::now();
        loop {
            if backend::port_open(WEB_PORT) {
                self.child = Some(child);
                self.since = Some(Instant::now());
                self.set(
                    Phase::Running { owned: true },
                    format!(
                        "PID {pid} · 就绪用时 {}",
                        fmt_ms(started.elapsed().as_millis() as u64)
                    ),
                );
                if let Err(e) = backend::open_ui() {
                    self.console(format!("自动打开界面失败：{e}"));
                }
                return;
            }
            if let Ok(Some(status)) = child.try_wait() {
                let code = status.code().unwrap_or(-1);
                self.set(
                    Phase::Failed,
                    format!(
                        "后端进程提前退出（code={code}），详见运行控制台。常见原因：端口被占用，或数据目录正被另一个实例使用。"
                    ),
                );
                return;
            }
            if started.elapsed() >= START_TIMEOUT {
                // 防残留：拉起的进程没就绪也不许挂在后台
                let _ = child.kill();
                let _ = child.wait();
                self.set(
                    Phase::Failed,
                    "启动超时：30s 内端口未就绪，已终止本次拉起的进程",
                );
                return;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    fn stop(&mut self, quick: bool) {
        match self.phase {
            Phase::Stopped | Phase::Failed | Phase::Stopping => return,
            Phase::Starting => {
                // 菜单在 Starting 置灰，这里只兜底防重入
                if let Some(child) = self.child.as_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                self.child = None;
                self.set(Phase::Stopped, "启动已中止");
                return;
            }
            Phase::Running { .. } => {}
        }
        let owned = self.child.is_some();
        self.set(Phase::Stopping, "正在通知后端退出…");
        // 优雅优先：HTTP app_quit（后端 flush 日志后自退，与页面电源按钮同一链路）
        match backend::request_quit(WEB_PORT) {
            Ok(()) => self.console("已发送退出指令，等待进程退出…"),
            Err(e) => {
                self.console(format!("优雅退出失败：{e}"));
                if !owned {
                    // 外部启动的进程没有句柄，不能强杀陌生进程
                    self.set(
                        Phase::Running { owned: false },
                        "停止失败：实例由外部启动，本启动器不强制结束它；可在界面右上角电源按钮退出",
                    );
                    return;
                }
            }
        }
        let deadline = Instant::now()
            + if quick {
                QUICK_STOP_TIMEOUT
            } else {
                STOP_TIMEOUT
            };
        if let Some(child) = self.child.as_mut() {
            while Instant::now() < deadline {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
                let _ = child.wait();
                self.console("优雅退出超时，已强制结束进程");
            }
        }
        // 确认端口释放：不残留、不占端口
        let close_deadline = Instant::now() + PORT_CLOSE_WAIT;
        while backend::port_open(WEB_PORT) {
            if Instant::now() >= close_deadline {
                if owned {
                    self.set(
                        Phase::Failed,
                        "停止失败：进程已退出但端口未释放，可能有其他程序占用 5274",
                    );
                } else {
                    self.set(
                        Phase::Running { owned: false },
                        "停止失败：端口仍被外部实例占用",
                    );
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        if let Some(child) = self.child.take() {
            let mut child = child;
            let _ = child.wait();
        }
        self.since = None;
        self.set(Phase::Stopped, "后端已停止，端口已释放");
    }

    /// 无命令时的对账：外部把后端杀掉 / 后端自己崩掉都要反映到托盘。
    fn reconcile(&mut self) {
        match self.phase {
            Phase::Running { owned: true } => {
                let Some(child) = self.child.as_mut() else {
                    return;
                };
                if let Ok(Some(status)) = child.try_wait() {
                    let code = status.code().unwrap_or(-1);
                    let mut child = self.child.take().expect("刚在等它退出");
                    let _ = child.wait();
                    self.since = None;
                    if code == 0 {
                        self.set(Phase::Stopped, "后端已退出（界面电源按钮或外部正常关闭）");
                    } else {
                        self.set(
                            Phase::Failed,
                            format!("后端进程异常退出（code={code}），详见运行控制台"),
                        );
                    }
                }
            }
            Phase::Running { owned: false } => {
                if !backend::port_open(WEB_PORT) {
                    self.set(Phase::Stopped, "外部实例已退出");
                }
            }
            _ => {}
        }
    }

    /// 后端 stdout/stderr 各起一个读线程，行级转发进运行控制台。
    fn attach_output(&self, child: &mut Child) {
        if let Some(out) = child.stdout.take() {
            let events = self.events.clone();
            std::thread::spawn(move || read_lines(out, "[后端]", events));
        }
        if let Some(err) = child.stderr.take() {
            let events = self.events.clone();
            std::thread::spawn(move || read_lines(err, "[后端·err]", events));
        }
    }
}

fn read_lines<R: std::io::Read + Send + 'static>(stream: R, tag: &str, events: Sender<Event>) {
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let count = line.chars().count();
        let mut clipped: String = line.chars().take(500).collect();
        if count > 500 {
            clipped.push_str("…（已截断）");
        }
        let _ = events.send(Event::Console(format!("{tag} {clipped}")));
    }
}

pub fn fmt_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.2}s", ms as f64 / 1000.0)
    }
}

/// 本地时间 HH:MM:SS（GetLocalTime，不引 chrono）。
fn now_hms() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let st = unsafe { GetLocalTime() };
    format!("{:02}:{:02}:{:02}", st.wHour, st.wMinute, st.wSecond)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_ms_matches_backend_style() {
        assert_eq!(fmt_ms(523), "523ms");
        assert_eq!(fmt_ms(3210), "3.21s");
    }
}
