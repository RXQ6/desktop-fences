//! 开机自启：读写 HKCU\Software\Microsoft\Windows\CurrentVersion\Run

use windows::core::{w, HRESULT, PCWSTR};
use windows::Win32::Foundation::{E_FAIL, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
};

/// 把 windows-rs 的 WIN32_ERROR 转成可 ? 传播的 Result<(), Error>
#[allow(dead_code)]
fn win32_to_result(e: WIN32_ERROR) -> windows::core::Result<()> {
    if e == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(windows::core::Error::new(
            HRESULT::from_win32(e.0),
            "Windows API 调用失败",
        ))
    }
}

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const APP_NAME: PCWSTR = w!("DesktopFences");

/// 开启开机自启
#[allow(dead_code)]
pub fn enable_startup() -> windows::core::Result<()> {
    unsafe {
        let exe = std::env::current_exe()
            .map_err(|e| windows::core::Error::new(E_FAIL, e.to_string()))?;
        let exe_str = exe.to_string_lossy().to_string();

        let mut wide: Vec<u16> = exe_str.encode_utf16().collect();
        wide.push(0);

        let mut key = HKEY::default();
        win32_to_result(RegOpenKeyExW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            0,
            KEY_SET_VALUE,
            &mut key,
        ))?;

        let data_bytes: &[u8] = std::slice::from_raw_parts(
            wide.as_ptr() as *const u8,
            wide.len() * 2,
        );
        let r = RegSetValueExW(key, APP_NAME, 0, REG_SZ, Some(data_bytes));
        let _ = RegCloseKey(key);
        win32_to_result(r)
    }
}

/// 关闭开机自启
#[allow(dead_code)]
pub fn disable_startup() -> windows::core::Result<()> {
    unsafe {
        let mut key = HKEY::default();
        win32_to_result(RegOpenKeyExW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            0,
            KEY_SET_VALUE,
            &mut key,
        ))?;
        let r = RegDeleteValueW(key, APP_NAME);
        let _ = RegCloseKey(key);
        // 如果值不存在，也视为成功
        match win32_to_result(r) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == HRESULT::from_win32(2) => Ok(()), // ERROR_FILE_NOT_FOUND
            other => other,
        }
    }
}

/// 查询是否已开启开机自启
pub fn is_startup_enabled() -> bool {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, KEY_QUERY_VALUE, &mut key).is_err() {
            return false;
        }
        let exists = RegQueryValueExW(key, APP_NAME, None, None, None, None).is_ok();
        let _ = RegCloseKey(key);
        exists
    }
}
