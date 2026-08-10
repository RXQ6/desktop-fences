//! 分层窗口：WS_EX_LAYERED + UpdateLayeredWindow
//!
//! 拆分成 register_class + create_fence_window，
//! 由 fence/manager.rs 统一管理多窗口生命周期。

use std::ffi::c_void;
use std::sync::Arc;

use parking_lot::Mutex;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::{Config, Fence};
use crate::dragdrop;

const FENCE_CLASS_NAME: PCWSTR = w!("DesktopFenceClass");
const TIMER_ID_REPAINT: usize = 1;
const TIMER_INTERVAL_MS: u32 = 2000; // 2 秒重绘一次

/// 窗口额外数据（通过 GWLP_USERDATA 存取）
struct WindowState {
    config: Arc<Mutex<Config>>,
    desktop: std::path::PathBuf,
    fence_id: String,
    drop_target: *mut c_void,
}

/// 注册窗口类（只调一次）
pub unsafe fn register_class(hinstance: HMODULE) {
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        hInstance: HINSTANCE(hinstance.0),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_else(|_| HCURSOR(std::ptr::null_mut())),
        hbrBackground: HBRUSH::default(),
        lpszClassName: FENCE_CLASS_NAME,
        ..Default::default()
    };
    let atom = RegisterClassExW(&wc);
    if atom == 0 {
        tracing::error!("RegisterClassExW 失败: {}", GetLastError().0);
    }
}

/// 创建一个栅栏窗口
pub unsafe fn create_fence_window(
    hinstance: HMODULE,
    parent: HWND,
    fence: &Fence,
    config: Arc<Mutex<Config>>,
    desktop: std::path::PathBuf,
    config_path: std::path::PathBuf,
) -> HWND {
    let [x, y, w, h] = fence.rect;

    let ex_style = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
    let style = WS_POPUP;

    let hwnd = match CreateWindowExW(
        ex_style,
        FENCE_CLASS_NAME,
        w!("Desktop Fence"),
        style,
        x, y, w, h,
        parent,
        None,
        hinstance,
        None,
    ) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("CreateWindowExW 失败 for {}: {}", fence.id, e);
            return HWND::default();
        }
    };

    if hwnd.is_invalid() {
        tracing::error!("CreateWindowExW 失败 for {}: {}", fence.id, GetLastError().0);
        return hwnd;
    }

    // 注册 OLE 拖放目标（接收拖入的文件）
    let drop_target = dragdrop::register_drop_target(
        hwnd,
        fence.id.clone(),
        config.clone(),
        desktop.clone(),
        config_path.clone(),
    );

    // 存储状态
    let state = Box::new(WindowState {
        config: config.clone(),
        desktop: desktop.clone(),
        fence_id: fence.id.clone(),
        drop_target,
    });
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);

    // 显示
    let _ = ShowWindow(hwnd, SW_SHOWNORMAL);

    // 首次渲染
    render_window(hwnd, &config, &desktop, &fence.id);

    // 定时重绘
    let _ = SetTimer(hwnd, TIMER_ID_REPAINT, TIMER_INTERVAL_MS, None);

    tracing::info!("栅栏窗口已创建: {} ({})", fence.name, fence.id);
    hwnd
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_DESTROY => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowState;
            if !ptr.is_null() {
                let state = &*ptr;
                dragdrop::unregister_drop_target(hwnd, state.drop_target);
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowState;
            if !ptr.is_null() {
                let state = &*ptr;
                let x = (lparam.0 & 0xFFFF) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;

                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);
                let width = rect.right - rect.left;
                let height = rect.bottom - rect.top;

                let cfg = state.config.lock();
                if let Some(fence) = cfg.fences.iter().find(|f| f.id == state.fence_id) {
                    if let Some(idx) = crate::fence::render::hit_test_item(fence, x, y, width, height) {
                        if let Some(item) = fence.items.get(idx) {
                            let abs = state.desktop.join(&item.path);
                            if abs.is_file() {
                                drop(cfg);
                                let _ = dragdrop::start_drag(
                                    hwnd,
                                    vec![abs.to_string_lossy().to_string()],
                                );
                            }
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if wparam.0 == 27 {
                // ESC 退出
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        WM_TIMER => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowState;
            if !ptr.is_null() {
                let state = &*ptr;
                render_window(hwnd, &state.config, &state.desktop, &state.fence_id);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// 处理点击：OLE 拖放暂时禁用
// unsafe fn handle_click_and_drag(...) { ... }

/// 渲染指定栅栏到分层窗口
fn render_window(
    hwnd: HWND,
    config: &Arc<Mutex<Config>>,
    desktop: &std::path::Path,
    fence_id: &str,
) {
    unsafe {
        let rect = {
            let mut r = RECT::default();
            let _ = GetClientRect(hwnd, &mut r);
            r
        };
        let width = (rect.right - rect.left) as i32;
        let height = (rect.bottom - rect.top) as i32;
        if width <= 0 || height <= 0 {
            return;
        }

        let hdc_screen = GetDC(None);
        if hdc_screen.is_invalid() {
            return;
        }
        let hdc_mem = CreateCompatibleDC(hdc_screen);

        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let hbitmap = match CreateDIBSection(hdc_mem, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(h) => h,
            Err(_) => {
                let _ = DeleteDC(hdc_mem);
                let _ = ReleaseDC(None, hdc_screen);
                return;
            }
        };
        if bits.is_null() {
            let _ = DeleteDC(hdc_mem);
            let _ = ReleaseDC(None, hdc_screen);
            return;
        }

        let old_obj = SelectObject(hdc_mem, hbitmap);

        // 清零（完全透明）
        let pixel_count = (width as usize) * (height as usize);
        let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, pixel_count);
        pixels.fill(0);

        // 渲染指定栅栏
        let cfg = config.lock();
        crate::fence::render::render_fence_by_id(hdc_mem, width, height, &cfg, desktop, fence_id);
        drop(cfg);

        // 提交到分层窗口
        let screen_pt = POINT { x: 0, y: 0 };
        let pt_src = POINT { x: 0, y: 0 };
        let size = SIZE { cx: width, cy: height };
        let mut blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };

        let _ = UpdateLayeredWindow(
            hwnd,
            hdc_screen,
            Some(&screen_pt as *const POINT),
            Some(&size as *const SIZE),
            hdc_mem,
            Some(&pt_src as *const POINT),
            COLORREF(0),
            Some(&mut blend as *const BLENDFUNCTION),
            ULW_ALPHA,
        );

        let _ = SelectObject(hdc_mem, old_obj);
        let _ = DeleteObject(hbitmap);
        let _ = DeleteDC(hdc_mem);
        let _ = ReleaseDC(None, hdc_screen);
    }
}
