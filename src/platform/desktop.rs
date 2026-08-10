//! WorkerW 嵌入：找到桌面壁纸之上的 WorkerW 窗口
//!
//! Windows 没有官方 API 让你"在桌面图标下面放个窗口"。
//! 标准做法：向 Progman 发送未公开消息 0x052C，让它创建一个 WorkerW
//! 子窗口来托管壁纸动画。这个 WorkerW 就是我们要嵌入的目标。

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex as StdMutex, OnceLock};

use windows::core::{w, PWSTR};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, WPARAM};
use windows::Win32::UI::Controls::{
    LVIF_TEXT, LVITEMW, LVM_GETITEMCOUNT, LVM_GETITEMPOSITION, LVM_GETITEMTEXTW,
    LVM_SETITEMPOSITION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, FindWindowW, GetClassNameW, GetWindow,
    SendMessageTimeoutW, SendMessageW, GW_HWNDNEXT, SMTO_NORMAL,
};

/// 把 (x, y) 打包成 LVM_SETITEMPOSITION 需要的 LPARAM（等价于 MAKELPARAM）
fn lparam_point(x: u16, y: u16) -> LPARAM {
    LPARAM(((y as isize) << 16) | (x as isize))
}

/// 找到或创建一个 WorkerW 窗口
/// （注：当前栅栏窗口改为顶层窗口，不再嵌入 WorkerW，此函数暂未使用，保留以备回退）
#[allow(dead_code)]
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

#[allow(dead_code)]
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

// ============================ 桌面图标隐藏（去重显示）============================
//
// 思路：Explorer 桌面的文件其实是 SysListView32 控件里的项。
// 把已归入栅栏的文件对应的图标移到屏幕外（坐标极大），桌面就"看起来干净"了，
// 只剩栅栏里的视图。原坐标先存进 SAVED_POSITIONS，禅模式/恢复时再移回去。
//
// 注意：这是对 Explorer 内部 ListView 的"黑魔法"，依赖 Windows 版本；
// 在 headless / 无 Explorer 环境下 find_desktop_listview 返回 None，自动降级为 no-op。

static SAVED_POSITIONS: OnceLock<StdMutex<HashMap<String, (i32, i32)>>> = OnceLock::new();

fn saved_positions() -> &'static StdMutex<HashMap<String, (i32, i32)>> {
    SAVED_POSITIONS.get_or_init(|| StdMutex::new(HashMap::new()))
}

/// [诊断] 输出桌面窗口层级：Progman / SHELLDLL_DefView / WorkerW / SysListView32
pub fn probe_desktop_layers() {
    unsafe {
        let progman = FindWindowW(w!("Progman"), None).ok();
        let mut defview: HWND = HWND::default();
        let _ = EnumWindows(
            Some(enum_find_defview),
            LPARAM(&mut defview as *mut HWND as isize),
        );
        let workerw = if defview.is_invalid() {
            None
        } else {
            GetWindow(defview, GW_HWNDNEXT)
                .ok()
                .filter(|h| !h.is_invalid())
        };
        let lv = find_desktop_listview();
        tracing::info!(
            "[诊断] 桌面层级: Progman={:p}, SHELLDLL_DefView={:p}, WorkerW(DefView下一兄弟)={:p}, SysListView32={:p}",
            progman.map(|h| h.0).unwrap_or(std::ptr::null_mut()),
            defview.0,
            workerw.map(|h| h.0).unwrap_or(std::ptr::null_mut()),
            lv.map(|h| h.0).unwrap_or(std::ptr::null_mut())
        );
    }
}

/// 找到桌面 SysListView32 控件句柄
pub fn find_desktop_listview() -> Option<HWND> {
    unsafe {
        let mut defview: HWND = HWND::default();
        let _ = EnumWindows(
            Some(enum_find_defview),
            LPARAM(&mut defview as *mut HWND as isize),
        );
        if defview.is_invalid() {
            return None;
        }
        let lv = match FindWindowExW(defview, None, w!("SysListView32"), None) {
            Ok(h) => h,
            Err(_) => return None,
        };
        if lv.is_invalid() {
            None
        } else {
            Some(lv)
        }
    }
}

unsafe extern "system" fn enum_find_defview(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let storage = &mut *(lparam.0 as *mut HWND);
    let mut buf = [0u16; 256];
    let len = GetClassNameW(hwnd, &mut buf);
    if len > 0 {
        let name = String::from_utf16_lossy(&buf[..len as usize]);
        if name == "SHELLDLL_DefView" {
            *storage = hwnd;
            return BOOL(0);
        }
    }
    BOOL(1)
}

unsafe fn listview_item_text(lv: HWND, i: i32) -> String {
    let mut buf = [0u16; 260];
    let mut item = LVITEMW {
        mask: LVIF_TEXT,
        iItem: i,
        pszText: PWSTR(buf.as_mut_ptr()),
        cchTextMax: buf.len() as i32,
        ..Default::default()
    };
    SendMessageW(
        lv,
        LVM_GETITEMTEXTW,
        WPARAM(i as usize),
        LPARAM(&mut item as *mut _ as isize),
    );
    let nul = buf.iter().position(|&c| c == 0).unwrap_or(0);
    String::from_utf16_lossy(&buf[..nul])
}

/// 把 names 里的文件对应的桌面图标移到屏幕外（隐藏）
pub fn hide_icons_for_names(names: &HashSet<String>) {
    unsafe {
        let Some(lv) = find_desktop_listview() else {
            return;
        };
        let count = SendMessageW(lv, LVM_GETITEMCOUNT, WPARAM(0), LPARAM(0)).0 as i32;
        let mut saved = saved_positions().lock().unwrap();
        for i in 0..count {
            let name = listview_item_text(lv, i);
            if names.contains(&name) {
                let mut pt = POINT::default();
                SendMessageW(
                    lv,
                    LVM_GETITEMPOSITION,
                    WPARAM(i as usize),
                    LPARAM(&mut pt as *mut _ as isize),
                );
                saved.entry(name.clone()).or_insert((pt.x, pt.y));
                // 移到屏幕外（坐标打包进 LPARAM 的低/高位）
                SendMessageW(lv, LVM_SETITEMPOSITION, WPARAM(i as usize), lparam_point(0xF000, 0xF000));
            }
        }
    }
}

/// 恢复所有曾被隐藏的桌面图标到原始位置
pub fn show_all_icons() {
    unsafe {
        let Some(lv) = find_desktop_listview() else {
            return;
        };
        let count = SendMessageW(lv, LVM_GETITEMCOUNT, WPARAM(0), LPARAM(0)).0 as i32;
        let saved = saved_positions().lock().unwrap();
        for i in 0..count {
            let name = listview_item_text(lv, i);
            if let Some(&(x, y)) = saved.get(&name) {
                SendMessageW(
                    lv,
                    LVM_SETITEMPOSITION,
                    WPARAM(i as usize),
                    lparam_point(x as u16, y as u16),
                );
            }
        }
    }
}
