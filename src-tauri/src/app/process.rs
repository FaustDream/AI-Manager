//! 进程检测、静默启动、单实例、开机自启。全部走 Windows API。
//!
//! 两条硬要求：
//!   - **外部命令不得弹控制台**（契约 20 / AGENTS 红线 11）：CreateProcessW 必须带
//!     `CREATE_NO_WINDOW`，且 STARTUPINFO 用 SW_HIDE 双保险。
//!   - **单实例句柄持有到进程退出**（AGENTS §6）：不手工 CloseHandle，
//!     否则互斥体可能提前释放，第二次启动就能进来。

use crate::app::error::{AppError, Result};
use std::path::Path;
use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_READ, KEY_SET_VALUE, REG_SZ,
};
use windows::Win32::System::Threading::{
    CreateMutexW, CreateProcessW, CREATE_NO_WINDOW, PROCESS_INFORMATION, STARTF_USESHOWWINDOW,
    STARTUPINFOW,
};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const AUTOSTART_NAME: &str = "AIManager";

/// 给定进程名（如 `WorkBuddy.exe`）是否正在运行。比较时不区分大小写。
pub fn process_running(names: &[&str]) -> bool {
    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return false;
    };

    let mut entry = PROCESSENTRY32W::default();
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
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

/// 静默启动一个可执行文件（不带参数，启动客户端够用）。
/// 返回 false 表示互斥体已存在——即已有实例在跑。
pub fn start_hidden(exe: &Path) -> Result<()> {
    if !exe.exists() {
        return Err(AppError::new("io", format!("可执行文件不存在：{}", exe.display())));
    }
    let path = HSTRING::from(exe.to_string_lossy().as_ref());
    let mut startup = STARTUPINFOW::default();
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    startup.dwFlags = STARTF_USESHOWWINDOW;
    startup.wShowWindow = 0; // SW_HIDE，与 CREATE_NO_WINDOW 互为双保险

    let mut info = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessW(
            &path,
            windows::core::PWSTR::null(),
            None,
            None,
            false,
            CREATE_NO_WINDOW,
            None,
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

/// 单实例句柄。**故意不实现 Drop 关闭**——关掉就等于放弃了互斥体。
pub struct SingleInstance {
    _handle: HANDLE,
}

// 安全依据：HANDLE 是内核对象句柄，本身没有线程归属，可跨线程使用与释放。
// 这里只是被 Tauri 的 `setup` 闭包要求 Send，实际只持有、不解引用。
unsafe impl Send for SingleInstance {}
unsafe impl Sync for SingleInstance {}

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

pub fn set_autostart(enabled: bool, exe: &Path) -> Result<()> {
    let subkey = HSTRING::from(RUN_KEY);
    let mut hkey = HKEY::default();
    let status = unsafe {
        RegOpenKeyExW(HKEY_CURRENT_USER, &subkey, 0, KEY_READ | KEY_SET_VALUE, &mut hkey)
    };
    if status.0 != 0 {
        return Err(AppError::new("io", format!("打开开机自启注册表项失败：{status:?}")));
    }

    let name = HSTRING::from(AUTOSTART_NAME);
    let result = if enabled {
        let value = HSTRING::from(format!("\"{}\"", exe.display()));
        let wide = value.as_wide();
        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2)
        };
        unsafe { RegSetValueExW(hkey, &name, 0, REG_SZ, Some(bytes)) }
    } else {
        unsafe { RegDeleteValueW(hkey, &name) }
    };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    if result.0 != 0 {
        return Err(AppError::new("io", format!("设置开机自启失败：{result:?}")));
    }
    Ok(())
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

    #[test]
    fn single_instance_second_call_returns_none() {
        let name = "AI-Manager-Test-Instance";
        let first = acquire_single_instance(name).expect("首次应取得锁");
        assert!(first.is_some());
        let second = acquire_single_instance(name).expect("第二次调用本身不该报错");
        assert!(second.is_none(), "已有实例时第二次必须返回 None");
        // 注意：句柄故意不关闭，进程退出后系统自动回收互斥体
    }

    #[test]
    fn start_hidden_rejects_missing_file() {
        assert!(start_hidden(Path::new("D:/不存在的程序.exe")).is_err());
    }
}
