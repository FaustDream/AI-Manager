//! 开机自启：HKCU Run 键的读写，菜单勾选即写、取消即删。
//! 只写本启动器自己的值名，不碰安装包/旧桌面版留下的其他 Run 项。

use windows::core::HSTRING;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "AIManagerLauncher";

fn open_key(write: bool) -> Result<HKEY, String> {
    let mut key = HKEY::default();
    let flags = if write {
        KEY_QUERY_VALUE | KEY_SET_VALUE
    } else {
        KEY_QUERY_VALUE
    };
    let subkey = HSTRING::from(RUN_KEY);
    let err = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, &subkey, Some(0), flags, &mut key) };
    if err != ERROR_SUCCESS {
        return Err(format!("打开注册表 Run 键失败（{err:?}）"));
    }
    Ok(key)
}

/// 当前是否已设置开机自启。
pub fn enabled() -> bool {
    let Ok(key) = open_key(false) else {
        return false;
    };
    let value = HSTRING::from(VALUE_NAME);
    let err = unsafe { RegQueryValueExW(key, &value, None, None, None, None) };
    let _ = unsafe { RegCloseKey(key) };
    err == ERROR_SUCCESS
}

/// 写/删自启项。值 = 带引号的启动器完整路径（路径含空格也安全）。
pub fn set(on: bool) -> Result<(), String> {
    let key = open_key(true)?;
    let value = HSTRING::from(VALUE_NAME);
    let result = (|| {
        if on {
            let exe = std::env::current_exe().map_err(|e| format!("定位启动器失败：{e}"))?;
            let text = format!("\"{}\"", exe.display());
            let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            // SAFETY: bytes 指向 wide 的内存，调用期间 wide 未被移动
            let bytes =
                unsafe { std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2) };
            let err = unsafe { RegSetValueExW(key, &value, None, REG_SZ, Some(bytes)) };
            if err != ERROR_SUCCESS {
                return Err(format!("写入自启项失败（{err:?}）"));
            }
            Ok(())
        } else {
            let err = unsafe { RegDeleteValueW(key, &value) };
            if err != ERROR_SUCCESS && err != ERROR_FILE_NOT_FOUND {
                return Err(format!("删除自启项失败（{err:?}）"));
            }
            Ok(())
        }
    })();
    let _ = unsafe { RegCloseKey(key) };
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_clear_roundtrip() {
        // 直接操作真实 HKCU：这是用户级配置键，读写不破坏其他值。
        let before = enabled();
        set(true).expect("写自启项应成功");
        assert!(enabled(), "写入后应读到");
        set(false).expect("删自启项应成功");
        assert!(!enabled(), "删除后应读不到");
        // 恢复测试前的状态，不改变用户的既有选择
        if before {
            set(true).unwrap();
        }
        // 重复删除不报错（幂等）
        set(false).unwrap();
        if before {
            set(true).unwrap();
        }
    }
}
