//! 进程检测、静默启动、单实例、开机自启。全部走 Windows API。
//!
//! 三条硬要求：
//!   - **外部命令不得弹控制台**（契约 20 / AGENTS 红线 11）：CreateProcessW 必须带
//!     `CREATE_NO_WINDOW`，且 STARTUPINFO 用 SW_HIDE 双保险。
//!   - **GUI 客户端不得继承本进程的控制台**：非 hidden 启动（`start_detached`、浏览器）
//!     必须带 `DETACHED_PROCESS`。否则 AI Manager 一旦从终端/控制台被拉起，被它启动的
//!     WorkBuddy 会绑定同一个控制台，终端一关客户端就跟着 STATUS_CONTROL_C_EXIT 退出
//!     （反复重启根因之一，见 docs/workbuddy-反复重启排查.md §2.3）。
//!   - **单实例句柄持有到进程退出**（AGENTS §6）：不手工 CloseHandle，
//!     否则互斥体可能提前释放，第二次启动就能进来。

use crate::app::error::{AppError, ErrorKind, Result};
use std::path::{Path, PathBuf};
use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_SZ,
};
use windows::Win32::System::Threading::{
    CreateMutexW, CreateProcessW, OpenProcess, TerminateProcess, CREATE_NO_WINDOW,
    CREATE_UNICODE_ENVIRONMENT, DETACHED_PROCESS, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
    PROCESS_TERMINATE, STARTF_USESHOWWINDOW, STARTUPINFOW,
};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const AUTOSTART_NAME: &str = "AIManager";

/// 给定进程名（如 `WorkBuddy.exe`）是否正在运行。比较时不区分大小写。
pub fn process_running(names: &[&str]) -> bool {
    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return false;
    };

    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut found = false;

    unsafe {
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let raw = String::from_utf16_lossy(&entry.szExeFile);
                let name = raw.trim_end_matches('\0');
                if names.iter().any(|n| name.eq_ignore_ascii_case(n)) {
                    found = true;
                    break;
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    found
}

/// 关掉所有同名进程，返回关掉了几个。
///
/// 写客户端库之前必须调它：带着活进程写，客户端会把内存里的旧状态回写覆盖，
/// 表现就是「删掉的会话第二天又回来了」。
pub fn kill_by_name(name: &str) -> Result<usize> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .map_err(|e| AppError::new(ErrorKind::Io, format!("枚举进程失败：{e}")))?;
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut killed = 0usize;
    unsafe {
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let raw = String::from_utf16_lossy(&entry.szExeFile);
                let proc_name = raw.trim_end_matches('\0');
                if proc_name.eq_ignore_ascii_case(name) {
                    if let Ok(handle) = OpenProcess(PROCESS_TERMINATE, false, entry.th32ProcessID) {
                        if TerminateProcess(handle, 1).is_ok() {
                            killed += 1;
                        }
                        let _ = CloseHandle(handle);
                    }
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    Ok(killed)
}

/// 正常启动一个 GUI 程序（客户端）。**不能用 SW_HIDE**：
/// 窗口出来就是隐藏的，用户以为没启动。
/// 走 `DETACHED_PROCESS`：客户端不继承本进程的控制台，关终端不会连累它。
pub fn start_detached(exe: &Path) -> Result<()> {
    spawn(exe, false)
}

/// 用系统默认程序打开一个 URL（浏览器登录页）。
/// 走 ShellExecuteW：不产生控制台窗口，浏览器由 shell 拉起、不挂在我们的进程树上。
pub fn open_url(url: &str) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let verb = HSTRING::from("open");
    let file = HSTRING::from(url);
    let result = unsafe { ShellExecuteW(None, &verb, &file, None, None, SW_SHOWNORMAL) };
    // 返回值 > 32 才算成功（历史约定：它不是句柄）
    result.0 as usize > 32
}

/// 读注册表原始值。字符串型用 `reg_read_string`，DWORD 用 `reg_read_dword`。
///
/// 与开机自启共用同一组 API（`RegOpenKeyExW` + `RegQueryValueExW`），
/// 不额外引入 windows feature；调用方自己按类型解析。
pub(crate) fn reg_read_raw(root: HKEY, subkey: &str, name: &str) -> Option<Vec<u8>> {
    let subkey = HSTRING::from(subkey);
    let name = HSTRING::from(name);
    let mut hkey = HKEY::default();
    if unsafe { RegOpenKeyExW(root, &subkey, 0, KEY_READ, &mut hkey) }.0 != 0 {
        return None;
    }
    let mut size = 0u32;
    let probe = unsafe { RegQueryValueExW(hkey, &name, None, None, None, Some(&mut size)) };
    if probe.0 != 0 || size == 0 {
        let _ = unsafe { RegCloseKey(hkey) };
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    let read = unsafe {
        RegQueryValueExW(
            hkey,
            &name,
            None,
            None,
            Some(buf.as_mut_ptr()),
            Some(&mut size),
        )
    };
    let _ = unsafe { RegCloseKey(hkey) };
    if read.0 != 0 {
        return None;
    }
    Some(buf)
}

/// 读 `REG_SZ`。`REG_SZ` 是 UTF-16LE、以 0 结尾。
pub(crate) fn reg_read_string(root: HKEY, subkey: &str, name: &str) -> Option<String> {
    let buf = reg_read_raw(root, subkey, name)?;
    let units: Vec<u16> = buf
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .take_while(|unit| *unit != 0)
        .collect();
    String::from_utf16(&units).ok()
}

/// 读 `REG_DWORD`。
pub(crate) fn reg_read_dword(root: HKEY, subkey: &str, name: &str) -> Option<u32> {
    let buf = reg_read_raw(root, subkey, name)?;
    let bytes: [u8; 4] = buf.get(..4)?.try_into().ok()?;
    Some(u32::from_le_bytes(bytes))
}

fn spawn(exe: &Path, hidden: bool) -> Result<()> {
    spawn_with_args(exe, &[], hidden)
}

/// 把裸程序名（如 `explorer.exe`）解析成绝对路径后再预检。
///
/// `Path::exists` 只认当前工作目录，而 CreateProcessW 对裸名会按
/// 「System32 → Windows → PATH」搜索——dev 模式工作目录是仓库根，
/// 「explorer.exe」就被误报「可执行文件不存在」。这里按同一顺序解析，
/// 解析到就交给 CreateProcessW 绝对路径；带目录的路径维持原语义只查存在性。
fn resolve_bare_exe(exe: &Path) -> Result<PathBuf> {
    let bare = exe.parent().is_none_or(|p| p.as_os_str().is_empty());
    if !bare {
        if exe.exists() {
            return Ok(exe.to_path_buf());
        }
        return Err(AppError::new(
            ErrorKind::Io,
            format!("可执行文件不存在：{}", exe.display()),
        ));
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(windir) = std::env::var("WINDIR") {
        let windir = PathBuf::from(windir);
        candidates.push(windir.join("System32").join(exe));
        candidates.push(windir.join(exe));
    }
    if let Some(path_var) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path_var).map(|p| p.join(exe)));
    }
    candidates.into_iter().find(|c| c.is_file()).ok_or_else(|| {
        AppError::new(
            ErrorKind::Io,
            format!("可执行文件不存在：{}", exe.display()),
        )
    })
}

/// 带参数启动。`CREATE_NO_WINDOW` 与 SW_HIDE 仍然双保险（契约 20）。
///
/// **启动 GUI 客户端时不继承本进程的环境**：如果 AI Manager 是从某个 Electron 工具
/// 的终端里拉起来的，环境里会带着 `ELECTRON_RUN_AS_NODE=1` / `NODE_OPTIONS`，
/// 被启动的客户端会以 Node 模式跑一遍就退出（退出码 0、没有窗口），
/// 表现就是「客户端关了再也起不来」。这几个变量在这里剥掉。
pub fn spawn_with_args(exe: &Path, args: &[&str], hidden: bool) -> Result<()> {
    let exe = &resolve_bare_exe(exe)?;
    let path = HSTRING::from(exe.to_string_lossy().as_ref());
    let mut command_line = build_command_line(exe, args);
    let command_ptr = if command_line.is_empty() {
        windows::core::PWSTR::null()
    } else {
        windows::core::PWSTR(command_line.as_mut_ptr())
    };
    let mut startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    if hidden {
        startup.dwFlags = STARTF_USESHOWWINDOW;
        startup.wShowWindow = 0; // SW_HIDE，与 CREATE_NO_WINDOW 互为双保险
    }

    // 环境块必须活到 CreateProcessW 返回
    let env = env_block_without(&[
        "ELECTRON_RUN_AS_NODE",
        "ELECTRON_NO_ATTACH_CONSOLE",
        "NODE_OPTIONS",
    ]);
    let env_ptr = if env.is_empty() {
        None
    } else {
        Some(env.as_ptr() as *const core::ffi::c_void)
    };

    let mut info = PROCESS_INFORMATION::default();
    unsafe {
        // 传了环境块就必须声明它是 Unicode 块，否则 CreateProcessW 直接报参数错误
        let mut flags = base_flags(hidden);
        if env_ptr.is_some() {
            flags |= CREATE_UNICODE_ENVIRONMENT;
        }
        CreateProcessW(
            &path,
            command_ptr,
            None,
            None,
            false,
            flags,
            env_ptr,
            None,
            &startup,
            &mut info,
        )?;
        // 进程/线程句柄与后续无关，可以关；互斥体句柄不能关（见 SingleInstance）
        let _ = CloseHandle(info.hProcess);
        let _ = CloseHandle(info.hThread);
    }
    Ok(())
}

/// 启动外部进程用的基础创建标志。单抽出来是为了能测——标志拼错只会在真机上表现为
/// 「从终端拉起 AI Manager 后，被它启动的客户端一关终端就跟着退出」。
///
/// - `hidden`（控制台应用静默执行）：`CREATE_NO_WINDOW`，既不继承本进程控制台也不弹窗。
/// - 非 `hidden`（GUI 客户端 / 浏览器）：`DETACHED_PROCESS`。GUI 程序本来不需要控制台，
///   但若不显式脱离，`CreateProcessW` 会把它挂到父进程的控制台上；父进程的控制台被
///   关闭时，GUI 程序会收到 CTRL_CLOSE_EVENT，以 STATUS_CONTROL_C_EXIT（0xC000013A）
///   退出。两者不能同时给——微软文档明确 `CREATE_NO_WINDOW` 与 `DETACHED_PROCESS`
///   并用时前者会被忽略，所以这里二选一。
fn base_flags(hidden: bool) -> PROCESS_CREATION_FLAGS {
    if hidden {
        CREATE_NO_WINDOW
    } else {
        DETACHED_PROCESS
    }
}

/// 复制当前环境并剔掉指定变量（大小写不敏感），返回 Windows 要求的 UTF-16 块：
/// 每条 `KEY=VALUE\0`，整体再补一个 `\0`，且按不区分大小写排序。
fn env_block_without(drop: &[&str]) -> Vec<u16> {
    let mut items: Vec<String> = std::env::vars()
        .filter(|(key, _)| !drop.iter().any(|d| d.eq_ignore_ascii_case(key)))
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    items.sort_by_key(|item| item.to_uppercase());
    let mut block = Vec::new();
    for item in items {
        block.extend(item.encode_utf16());
        block.push(0);
    }
    block.push(0);
    block
}

/// 拼命令行：每个参数加引号并转义内部引号，Windows 只接受一整条命令行。
/// 单独抽出来是为了能测——拼错了只会在真机上表现为「参数没传进去」。
fn build_command_line(exe: &Path, args: &[&str]) -> Vec<u16> {
    if args.is_empty() {
        return Vec::new();
    }
    let mut line = format!("\"{}\"", exe.display());
    for arg in args {
        line.push(' ');
        line.push('"');
        line.push_str(&arg.replace('"', "\\\""));
        line.push('"');
    }
    line.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 单实例句柄。**故意不实现 Drop 关闭**——关掉就等于放弃了互斥体。
pub struct SingleInstance {
    _handle: HANDLE,
}

// 安全依据：HANDLE 是内核对象句柄，本身没有线程归属，可跨线程使用与释放。
// 这里只是被 Tauri 的 `setup` 闭包要求 Send，实际只持有、不解引用。
unsafe impl Send for SingleInstance {}
unsafe impl Sync for SingleInstance {}

/// 静默执行一个程序并取回 stdout，超时直接杀掉。
///
/// Qoder 的 `resources/umid/runtime-info.exe` 靠它产出设备标识；不弹控制台这条
/// 红线仍然成立（`CREATE_NO_WINDOW`），只是这次需要读回输出。
pub fn run_capture(exe: &Path, timeout: std::time::Duration) -> Result<String> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const NO_WINDOW: u32 = 0x0800_0000; // CREATE_NO_WINDOW

    if !exe.exists() {
        return Err(AppError::new(
            ErrorKind::Io,
            format!("可执行文件不存在：{}", exe.display()),
        ));
    }
    let mut child = Command::new(exe)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(NO_WINDOW)
        .spawn()
        .map_err(|e| AppError::new(ErrorKind::Io, format!("启动失败：{e}")))?;

    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AppError::new(ErrorKind::Io, "子进程执行超时"));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(e) => return Err(AppError::new(ErrorKind::Io, e.to_string())),
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| AppError::new(ErrorKind::Io, e.to_string()))?;
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// 尝试取得单实例锁。返回 None 表示已有一个实例在跑。
pub fn acquire_single_instance(name: &str) -> Result<Option<SingleInstance>> {
    // Local\ 命名空间：同一用户会话内互斥，不跨会话
    let mutex_name = HSTRING::from(format!("Local\\{name}"));
    let handle = unsafe { CreateMutexW(None, false, &mutex_name)? };
    let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if already {
        return Ok(None);
    }
    Ok(Some(SingleInstance { _handle: handle }))
}

/// 开机自启现在是否开着：Run 值存在**且带 `--autostart` 参数**。
///
/// 只查「值存在」不够——旧版本写的是裸 exe 路径，那种值开机拉起识别不出
/// 「这是自启」，「自启最小化」随之失效（见 `repair_autostart`）。
pub fn autostart_enabled() -> bool {
    reg_read_string(HKEY_CURRENT_USER, RUN_KEY, AUTOSTART_NAME)
        .is_some_and(|value| is_autostart_command(&value))
}

/// Run 值是否为当前写法（带 `--autostart` 参数）。
fn is_autostart_command(value: &str) -> bool {
    value.contains("--autostart")
}

/// 自愈旧格式的 Run 值：存在但缺 `--autostart` 就按当前写法重写。
///
/// 老版本写入的值开机拉起不带参数，`desktop::run()` 识别不出自启场景，
/// 「自启最小化」永远不生效。启动时顺手修一次，下一次开机就正常。
pub fn repair_autostart() {
    let Some(value) = reg_read_string(HKEY_CURRENT_USER, RUN_KEY, AUTOSTART_NAME) else {
        return; // 没开自启：没有值可修
    };
    if is_autostart_command(&value) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    match set_autostart(true, &exe) {
        Ok(()) => crate::app::log::log(
            crate::app::log::Level::Info,
            "app",
            format!(
                "开机自启 Run 值是旧格式（缺 --autostart），已按当前写法重写：{}",
                exe.display()
            ),
        ),
        Err(e) => crate::app::log::log(
            crate::app::log::Level::Warn,
            "app",
            format!("开机自启 Run 值是旧格式且重写失败：{}", e.message),
        ),
    }
}

pub fn set_autostart(enabled: bool, exe: &Path) -> Result<()> {
    let subkey = HSTRING::from(RUN_KEY);
    let mut hkey = HKEY::default();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            &subkey,
            0,
            KEY_READ | KEY_SET_VALUE,
            &mut hkey,
        )
    };
    if status.0 != 0 {
        return Err(AppError::new(
            ErrorKind::Io,
            format!("打开开机自启注册表项失败：{status:?}"),
        ));
    }

    let name = HSTRING::from(AUTOSTART_NAME);
    let result = if enabled {
        // 带 --autostart：开机自启启动时进程据此识别「这是自启拉起」，
        // 配合配置「自启后最小化到托盘」决定是否弹主窗口（见 desktop/mod.rs）。
        let value = HSTRING::from(format!("\"{}\" --autostart", exe.display()));
        let wide = value.as_wide();
        let bytes: &[u8] =
            unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };
        unsafe { RegSetValueExW(hkey, &name, 0, REG_SZ, Some(bytes)) }
    } else {
        unsafe { RegDeleteValueW(hkey, &name) }
    };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    if result.0 != 0 {
        return Err(AppError::new(
            ErrorKind::Io,
            format!("设置开机自启失败：{result:?}"),
        ));
    }
    Ok(())
}

/// 把已有主窗口叫到前台（第二个实例启动时用）。
/// 窗口可能被收进托盘（隐藏）或最小化，两种状态都要还原成可见。
pub fn focus_window(title: &str) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
    };

    let hwnd = unsafe {
        let Ok(hwnd) = FindWindowW(None, &HSTRING::from(title)) else {
            return false;
        };
        if hwnd.0.is_null() {
            return false;
        }
        let _ = ShowWindow(hwnd, SW_SHOW);
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
        hwnd
    };
    !hwnd.0.is_null()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_a_running_process() {
        // 测试进程本身一定在跑（cargo test 的子进程）
        assert!(process_running(&["cargo.exe", "rustc.exe", "explorer.exe"]));
    }

    #[test]
    fn unknown_process_is_not_running() {
        assert!(!process_running(&["definitely-not-a-real-process-42.exe"]));
    }

    /// 真实故障：dev 模式工作目录是仓库根，`Path::exists` 对裸名「explorer.exe」
    /// 只查当前目录而误报「可执行文件不存在」。解析必须对齐 CreateProcessW
    /// 的搜索顺序（System32 → Windows → PATH）。
    #[test]
    fn bare_system_exe_resolves_to_absolute_path() {
        let resolved = resolve_bare_exe(Path::new("explorer.exe"))
            .expect("explorer.exe 应能按系统搜索顺序解析到");
        assert!(
            resolved.is_absolute(),
            "应解析成绝对路径：{}",
            resolved.display()
        );
        assert!(resolved.exists());

        // 带目录的路径维持原语义：只查存在性，不做搜索
        assert!(resolve_bare_exe(Path::new("definitely-missing-42.exe")).is_err());
        let abs_missing = std::env::temp_dir().join("definitely-missing-42.exe");
        assert!(resolve_bare_exe(&abs_missing).is_err());
    }

    #[test]
    fn single_instance_second_call_returns_none() {
        let name = "AI-Manager-Test-Instance";
        let first = acquire_single_instance(name).expect("首次应取得锁");
        assert!(first.is_some());
        let second = acquire_single_instance(name).expect("第二次调用本身不该报错");
        assert!(second.is_none(), "已有实例时第二次必须返回 None");
        // 注意：句柄故意不关闭，进程退出后系统自动回收互斥体
    }

    /// GUI 客户端（非 hidden）必须带 `DETACHED_PROCESS`：否则继承父进程控制台，
    /// 控制台关闭时客户端会跟着 STATUS_CONTROL_C_EXIT 退出（反复重启根因）。
    #[test]
    fn gui_spawn_detaches_from_parent_console() {
        assert!(
            base_flags(false).contains(DETACHED_PROCESS),
            "GUI 客户端启动必须脱离父进程控制台"
        );
        assert!(
            !base_flags(false).contains(CREATE_NO_WINDOW),
            "GUI 客户端不能带 CREATE_NO_WINDOW（它会让窗口隐藏，用户以为没启动）"
        );
    }

    /// 控制台应用静默执行（hidden）走 `CREATE_NO_WINDOW`，且不与 `DETACHED_PROCESS`
    /// 混用（微软文档：两者并用时 CREATE_NO_WINDOW 被忽略）。
    #[test]
    fn hidden_spawn_suppresses_console() {
        assert!(base_flags(true).contains(CREATE_NO_WINDOW));
        assert!(!base_flags(true).contains(DETACHED_PROCESS));
    }

    /// 旧版本写的 Run 值只有裸 exe 路径，那种值开机拉起识别不出「这是自启」；
    /// 只有带 `--autostart` 的当前写法才算数。
    #[test]
    fn autostart_command_requires_flag() {
        assert!(is_autostart_command(r#""C:\x\ai-manager.exe" --autostart"#));
        assert!(!is_autostart_command(r#""C:\x\ai-manager.exe""#));
        assert!(!is_autostart_command(""));
    }
}
