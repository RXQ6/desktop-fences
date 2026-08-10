//! WorkerW 嵌入：找到桌面壁纸之上的 WorkerW 窗口
//!
//! Windows 没有官方 API 让你"在桌面图标下面放个窗口"。
//! 标准做法：向 Progman 发送未公开消息 0x052C，让它创建一个 WorkerW
//! 子窗口来托管壁纸动画。这个 WorkerW 就是我们要嵌入的目标。

use windows::core::w;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowW, GetClassNameW, GetWindow, SendMessageTimeoutW, GW_HWNDNEXT,
    SMTO_NORMAL,
};

/// 找到或创建一个 WorkerW 窗口
pub fn find_or_create_workerw() -> Option<HWND> {
    unsafe {
        let progman = FindWindowW(w!("Progman"), None).ok()?;

        // 触发 Progman 创建 WorkerW（未公开消息 0x052C）
        let mut result = 0usize;
        let _ = SendMessageTimeoutW(
            progman,
            0x052C,
            WPARAM(0),
            LPARAM(0),
            SMTO_NORMAL,
            1000,
            Some(&mut result as *mut usize),
        );

        // 找 SHELLDLL_DefView（Explorer 持有的桌面图标宿主）
        let mut shell_view: HWND = HWND(std::ptr::null_mut());
        let _ = EnumWindows(
            Some(find_shell_def_view),
            LPARAM(&mut shell_view as *mut HWND as isize),
        );

        // 如果没找到，重试一次
        if shell_view.is_invalid() {
            let _ = SendMessageTimeoutW(
                progman,
                0x052C,
                WPARAM(0),
                LPARAM(0),
                SMTO_NORMAL,
                1000,
                Some(&mut result as *mut usize),
            );
            shell_view = HWND(std::ptr::null_mut());
            let _ = EnumWindows(
                Some(find_shell_def_view),
                LPARAM(&mut shell_view as *mut HWND as isize),
            );
        }

        if shell_view.is_invalid() {
            tracing::warn!("找不到 SHELLDLL_DefView，栅栏将作为普通顶层窗口运行");
            return None;
        }

        // SHELLDLL_DefView 的下一个兄弟窗口就是 WorkerW
        let workerw = match GetWindow(shell_view, GW_HWNDNEXT) {
            Ok(w) => w,
            Err(_) => HWND(std::ptr::null_mut()),
        };
        if workerw.is_invalid() {
            tracing::warn!("找不到 WorkerW，栅栏将作为普通顶层窗口运行");
            None
        } else {
            tracing::info!("已找到 WorkerW");
            Some(workerw)
        }
    }
}

unsafe extern "system" fn find_shell_def_view(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let storage = &mut *(lparam.0 as *mut HWND);
    let mut buf = [0u16; 256];
    let len = GetClassNameW(hwnd, &mut buf);
    if len > 0 {
        let name = String::from_utf16_lossy(&buf[..len as usize]);
        if name == "SHELLDLL_DefView" {
            *storage = hwnd;
            return BOOL(0); // 停止枚举
        }
    }
    BOOL(1) // 继续
}
