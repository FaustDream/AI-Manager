//! 后端进程的落点解析与生命周期原语。
//!
//! 只回答四个问题：后端 exe 在哪、怎么拉起、端口是否就绪、怎么优雅退出。
//! 数据目录规则与 src-tauri/src/app/paths.rs 对齐：debug 构建用仓库根 data/
//! （编译期 CARGO_MANIFEST_DIR），release 用 exe 同级 data/，AIMANAGER_DATA_DIR 可覆盖。

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// 与 src-tauri/src/desktop/web.rs 的 WEB_PORT、vite.config.ts 的 proxy 成对改。
pub const WEB_PORT: u16 = 5274;

/// 后端是控制台子系统程序（契约 19）：不给它弹控制台，输出走管道进启动器窗口。
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutKind {
    /// AIMANAGER_BACKEND_EXE 指定
    Env,
    /// 启动器同级（打包布局：portable zip / 安装目录）
    Sibling,
    /// 仓库开发布局（src-tauri/target/<profile>）
    Repo,
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub kind: LayoutKind,
    pub exe: PathBuf,
    pub cwd: PathBuf,
    pub data_dir: PathBuf,
    pub dist_index: PathBuf,
}

impl Layout {
    pub fn app_jsonl(&self) -> PathBuf {
        self.data_dir.join("logs").join("app.jsonl")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }
}

/// 从 start 目录向上找含 src-tauri/Cargo.toml 的仓库根。
fn repo_root(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start.to_path_buf());
    while let Some(d) = dir {
        if d.join("src-tauri").join("Cargo.toml").is_file() {
            return Some(d);
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    None
}

/// 缺 exe 时的指引：启动器不构建（保持轻量），只把路指清楚。
fn missing_hint() -> String {
    "未找到后端 ai-manager.exe。请先完成构建：开发期在仓库根运行 start.cmd \
     （或 npm run build 后 cargo build）；安装版请把 ai-manager.exe 放在本启动器同级目录。"
        .to_string()
}

/// 定位后端与配套目录。候选顺序：环境变量 → 启动器同级 → 仓库 target/debug → target/release。
pub fn resolve() -> Result<Layout, String> {
    let launcher_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .ok_or("定位启动器自身失败")?;

    let mut picked: Option<(PathBuf, LayoutKind)> = None;
    if let Some(exe) = std::env::var_os("AIMANAGER_BACKEND_EXE").map(PathBuf::from) {
        if exe.is_file() {
            picked = Some((exe, LayoutKind::Env));
        }
    }
    if picked.is_none() {
        let sibling = launcher_dir.join("ai-manager.exe");
        if sibling.is_file() {
            picked = Some((sibling, LayoutKind::Sibling));
        }
    }
    if picked.is_none() {
        if let Some(root) = repo_root(&launcher_dir) {
            for profile in ["debug", "release"] {
                let exe = root
                    .join("src-tauri")
                    .join("target")
                    .join(profile)
                    .join("ai-manager.exe");
                if exe.is_file() {
                    picked = Some((exe, LayoutKind::Repo));
                    break;
                }
            }
        }
    }
    let (exe, kind) = picked.ok_or_else(missing_hint)?;

    // 仓库布局以 src-tauri/Cargo.toml 为锚：cwd 与数据目录都按仓库根（同 start.cmd 的 /D 仓库根）。
    let exe_dir = exe.parent().unwrap_or(Path::new(".")).to_path_buf();
    let root = repo_root(&exe_dir);
    let data_dir = std::env::var_os("AIMANAGER_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| root.as_ref().map(|r| r.join("data")))
        .unwrap_or_else(|| exe_dir.join("data"));
    let dist_index = root
        .as_ref()
        .map(|r| r.join("dist").join("index.html"))
        .unwrap_or_else(|| exe_dir.join("dist").join("index.html"));
    let cwd = root.unwrap_or(exe_dir);

    Ok(Layout {
        kind,
        exe,
        cwd,
        data_dir,
        dist_index,
    })
}

/// 拉起后端：CREATE_NO_WINDOW + 输出管道。工作目录与 start.cmd 的 /D 一致。
pub fn spawn_backend(layout: &Layout) -> Result<Child, String> {
    use std::os::windows::process::CommandExt;
    Command::new(&layout.exe)
        .current_dir(&layout.cwd)
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| format!("拉起后端失败（{}）：{e}", layout.exe.display()))
}

/// 端口就绪判定：后端 axum 只绑 127.0.0.1，直连探测，不走系统代理。
pub fn port_open(port: u16) -> bool {
    let Ok(addr) = format!("127.0.0.1:{port}").parse::<SocketAddr>() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok()
}

/// 优雅退出请求。后端安全面要求自定义头 `x-ai-manager-web`（web.rs security_guard）；
/// 本工具形态（无 Origin）与 curl 同类，放行。app_quit 回包后 200ms 内自退（shell.rs）。
fn quit_request(port: u16) -> String {
    format!(
        "POST /cmd/app_quit HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Content-Type: application/json\r\n\
         x-ai-manager-web: 1\r\n\
         Content-Length: 2\r\n\
         Connection: close\r\n\
         \r\n\
         {{}}"
    )
}

/// 通知后端优雅退出（复用界面电源按钮同一条命令链路，不新增后端接口）。
pub fn request_quit(port: u16) -> Result<(), String> {
    let addr: SocketAddr = format!("127.0.0.1:{port}")
        .parse()
        .map_err(|e| format!("端口解析失败：{e}"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))
        .map_err(|e| format!("连接后端失败：{e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
    stream
        .write_all(quit_request(port).as_bytes())
        .map_err(|e| format!("发送退出指令失败：{e}"))?;
    let mut buf = String::new();
    let _ = stream.read_to_string(&mut buf);
    let head = buf.lines().next().unwrap_or_default();
    if head.contains(" 200 ") {
        Ok(())
    } else {
        Err(format!("后端未确认退出指令：{head}"))
    }
}

/// 用默认程序打开 URL 或目录（打开界面 / 日志目录）。与后端 process::open_url 同一 API。
pub fn open_target(target: &str) -> Result<(), String> {
    use windows::core::HSTRING;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let verb = HSTRING::from("open");
    let file = HSTRING::from(target);
    let code = unsafe { ShellExecuteW(None, &verb, &file, None, None, SW_SHOWNORMAL) }.0 as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(format!("系统拒绝打开：{target}"))
    }
}

pub fn open_ui() -> Result<(), String> {
    open_target(&format!("http://127.0.0.1:{WEB_PORT}/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_root_walks_up_to_marker() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        let deep = root.join("a").join("b");
        std::fs::create_dir_all(root.join("src-tauri")).unwrap();
        std::fs::write(root.join("src-tauri").join("Cargo.toml"), "[package]").unwrap();
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(repo_root(&deep), Some(root));
    }

    #[test]
    fn repo_root_is_none_without_marker() {
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("a").join("b");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(repo_root(&deep), None);
    }

    #[test]
    fn quit_request_carries_security_header_and_body() {
        let req = quit_request(WEB_PORT);
        assert!(req.starts_with("POST /cmd/app_quit HTTP/1.1"), "{req}");
        assert!(req.contains("x-ai-manager-web: 1"), "{req}");
        assert!(req.contains("Content-Length: 2"), "{req}");
        assert!(req.ends_with("{}"), "{req}");
    }

    #[test]
    fn unused_local_port_is_not_open() {
        // 127.0.0.1:1 拒绝连接是稳定的（无代理介入，直连探测）
        assert!(!port_open(1));
    }

    #[test]
    fn missing_hint_names_the_build_entry() {
        assert!(missing_hint().contains("start.cmd"));
        assert!(missing_hint().contains("ai-manager.exe"));
    }
}
