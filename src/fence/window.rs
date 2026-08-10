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
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
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
    config_path: std::path::PathBuf,
    fence_id: String,
    drop_target: *mut c_void,
    dragging: bool,
    drag_off_x: i32,
    drag_off_y: i32,
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

/// 关闭其他进程中同类的旧栅栏窗口，避免用户双击新 exe 时仍在跑旧的（未修复）窗口。
/// 旧进程收到 WM_CLOSE 后会走 WM_DESTROY → PostQuitMessage 自行退出。
pub unsafe fn close_existing_instances() {
    for _ in 0..10 {
        match FindWindowW(FENCE_CLASS_NAME, None) {
            Ok(hwnd) if !hwnd.is_invalid() => {
                let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            _ => break,
        }
    }
    // 给旧进程一点时间退出，避免新旧窗口短暂重叠
    std::thread::sleep(std::time::Duration::from_millis(300));
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

    // 注意：不再用 WS_EX_NOACTIVATE —— 顶层窗口配 NOACTIVATE 时点击不会被
    // 正常派发（且永远拿不到焦点导致 ESC 退出失效）。改为可激活，点击后栅栏
    // 获得输入焦点，WM_LBUTTONDOWN / WM_KEYDOWN(ESC) 都能收到。
    let ex_style = WS_EX_LAYERED | WS_EX_TOOLWINDOW;
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
        config_path: config_path.clone(),
        fence_id: fence.id.clone(),
        drop_target,
        dragging: false,
        drag_off_x: 0,
        drag_off_y: 0,
    });
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);

    // 显示
    let _ = ShowWindow(hwnd, SW_SHOWNORMAL);

    // 首次渲染
    render_window(hwnd, &config, &desktop, &fence.id);

    // 关键修复：分层窗口的透明像素(Alpha=0)默认点击穿透，
    // 导致标题栏/空白区域收不到 WM_LBUTTONDOWN，整框无法拖动。
    // 用 SetWindowRgn 把整个客户区设为命中区域，使点击在框内任意位置
    // 都能到达 wnd_proc（拖动/图标拖出逻辑即可生效），而视觉仍由分层
    // 位图决定（保持透明）。区域坐标用客户区矩形 (0,0,w,h)。
    let rgn = CreateRectRgn(0, 0, w, h);
    if !rgn.is_invalid() {
        let _ = SetWindowRgn(hwnd, rgn, true);
    }

    // 定时重绘
    let _ = SetTimer(hwnd, TIMER_ID_REPAINT, TIMER_INTERVAL_MS, None);

    // [诊断] 输出窗口信息：句柄 / 父窗口 / 样式 / 屏幕矩形
    unsafe {
        let parent = GetParent(hwnd).unwrap_or_default();
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let mut wr = RECT::default();
        let _ = GetWindowRect(hwnd, &mut wr);
        tracing::info!(
            "[诊断] 栅栏窗口 [{}] hwnd={:p} parent={:p} style=0x{:X} exstyle=0x{:X} rect=({},{},{},{})",
            fence.id,
            hwnd.0,
            parent.0,
            style,
            ex,
            wr.left, wr.top, wr.right, wr.bottom
        );
    }

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
            if ptr.is_null() {
                return LRESULT(0);
            }
            let state = &mut *ptr;
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;

            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let width = rect.right - rect.left;
            let height = rect.bottom - rect.top;

            // 先判断点中的是不是某个图标
            let hit_item = {
                let cfg = state.config.lock();
                cfg.fences
                    .iter()
                    .find(|f| f.id == state.fence_id)
                    .and_then(|f| crate::fence::render::hit_test_item(f, x, y, width, height))
            };

            if hit_item.is_some() {
                // 命中图标：走原有的 OLE 拖出（文件物理不动）
                tracing::info!("[{}] 命中图标，开始 OLE 拖出", state.fence_id);
                let cfg = state.config.lock();
                if let Some(fence) = cfg.fences.iter().find(|f| f.id == state.fence_id) {
                    if let Some(idx) = hit_item {
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
            } else {
                // 点中标题栏 / 空白处：拖动整个框
                let mut wr = RECT::default();
                let _ = GetWindowRect(hwnd, &mut wr);
                let mut cur = POINT::default();
                let _ = GetCursorPos(&mut cur);
                state.dragging = true;
                state.drag_off_x = cur.x - wr.left;
                state.drag_off_y = cur.y - wr.top;
                let prev = SetCapture(hwnd);
                tracing::info!(
                    "[{}] 进入移动模式: 点击({},{}), 窗口rect=({},{}), 捕获上一窗口={:p}",
                    state.fence_id, x, y, wr.left, wr.top, prev.0
                );
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowState;
            if !ptr.is_null() {
                let state = &mut *ptr;
                if state.dragging {
                    let mut cur = POINT::default();
                    let _ = GetCursorPos(&mut cur);
                    let new_x = cur.x - state.drag_off_x;
                    let new_y = cur.y - state.drag_off_y;
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        new_x,
                        new_y,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    // 立即重绘，让分层位图跟随窗口移动
                    render_window(hwnd, &state.config, &state.desktop, &state.fence_id);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowState;
            if !ptr.is_null() {
                let state = &mut *ptr;
                if state.dragging {
                    state.dragging = false;
                    let _ = ReleaseCapture();
                    // 持久化新位置到配置
                    let mut wr = RECT::default();
                    let _ = GetWindowRect(hwnd, &mut wr);
                    tracing::info!(
                        "[{}] 移动结束，新位置=({},{}), 保存配置",
                        state.fence_id, wr.left, wr.top
                    );
                    let mut cfg = state.config.lock();
                    if let Some(fence) = cfg.fences.iter_mut().find(|f| f.id == state.fence_id) {
                        fence.rect[0] = wr.left;
                        fence.rect[1] = wr.top;
                    }
                    let _ = cfg.save(&state.config_path);
                }
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowState;
            if !ptr.is_null() {
                let state = &mut *ptr;
                if state.dragging {
                    tracing::info!("[{}] 捕获被抢占（拖动被取消）", state.fence_id);
                }
                state.dragging = false;
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

        // 关键修复：分层窗口的命中测试按"每像素 alpha"判断是否属于窗口。
        // 背景被清零(alpha=0) → 整框点击穿透，WM_LBUTTONDOWN 收不到 → 拖不动。
        // 这里把整张位图 alpha 抬高到至少 MIN_ALPHA：框内任意位置都算窗口的一部分
        // （可点击、可拖动），而 MIN_ALPHA 很小时视觉上仍是淡面板/透明外观。
        // 想更透明就把 MIN_ALPHA 调小（1 = 几乎全透明但依然可点击）。
        const MIN_ALPHA: u32 = 40;
        for px in pixels.iter_mut() {
            let a = (*px >> 24) & 0xFF;
            if a < MIN_ALPHA {
                *px = (*px & 0x00FF_FFFF) | (MIN_ALPHA << 24);
            }
        }

        // 用窗口当前屏幕位置作为分层窗口位置，避免每次重绘被钉回 (0,0)
        let mut win_rect = RECT::default();
        let _ = GetWindowRect(hwnd, &mut win_rect);
        let screen_pt = POINT {
            x: win_rect.left,
            y: win_rect.top,
        };
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
