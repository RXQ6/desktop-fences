//! 设置面板：可视化增删栅栏 / 收纳规则，免手编 JSON
//!
//! 由 manager 在热键 Ctrl+Shift+O 时调用 `open()` 打开（modeless 窗口）。
//! 其消息走主线程消息循环，WM_COMMAND 由 DispatchMessageW 派发到本窗口过程。
//! 文本输入用内联 EDIT 控件（不用模态对话框，避免嵌套消息循环的黑魔法）。

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{HBRUSH, InvalidateRect, UpdateWindow};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::{Config, Fence, SweepRule};

// ---- 控件 ID ----
const ID_FENCE_NAME_EDIT: u32 = 100;
const ID_FENCE_LIST: u32 = 101;
const ID_RULE_FENCE_EDIT: u32 = 102;
const ID_RULE_PATTERN_EDIT: u32 = 103;
const ID_RULE_LIST: u32 = 104;
const ID_BTN_ADD_FENCE: u32 = 110;
const ID_BTN_DEL_FENCE: u32 = 111;
const ID_BTN_ADD_RULE: u32 = 112;
const ID_BTN_DEL_RULE: u32 = 113;
const ID_BTN_RELOAD: u32 = 114;
const ID_BTN_SAVE: u32 = 115;
const ID_BTN_CLOSE: u32 = 116;

const SETTINGS_CLASS: PCWSTR = w!("FenceSettingsClass");

struct SettingsState {
    config: Arc<Mutex<Config>>,
    config_path: PathBuf,
    hwnds: Arc<Mutex<Vec<(String, HWND)>>>,
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 把数字控件 id 转成 Windows 需要的 HMENU（子窗口 id 借用了 hMenu 字段的指针值）
fn ctrl_id(id: u32) -> HMENU {
    HMENU(id as usize as *mut c_void)
}

/// 打开（或前置已打开的）设置窗口
pub unsafe fn open(
    config: Arc<Mutex<Config>>,
    config_path: PathBuf,
    hwnds: Arc<Mutex<Vec<(String, HWND)>>>,
) {
    // 避免重复打开
    if let Ok(existing) = FindWindowW(SETTINGS_CLASS, None) {
        if !existing.is_invalid() {
            let _ = SetForegroundWindow(existing);
            return;
        }
    }

    let hmodule = GetModuleHandleW(None).ok().unwrap_or_default();
    let hinstance: HINSTANCE = hmodule.into();
    register_class(hinstance);

    let hwnd = match CreateWindowExW(
        WS_EX_APPWINDOW,
        SETTINGS_CLASS,
        w!("栅栏设置"),
        WS_OVERLAPPEDWINDOW,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        420,
        460,
        None,
        HMENU::default(),
        hinstance,
        None,
    ) {
        Ok(h) => h,
        Err(_) => return,
    };
    if hwnd.is_invalid() {
        return;
    }

    let state = Box::new(SettingsState {
        config,
        config_path,
        hwnds,
    });
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);

    create_controls(hwnd);
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = UpdateWindow(hwnd);
}

unsafe fn register_class(hinstance: HINSTANCE) {
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinstance,
        hCursor: LoadCursorW(None, IDC_ARROW).ok().unwrap_or_default(),
        hbrBackground: HBRUSH::default(),
        lpszClassName: SETTINGS_CLASS,
        ..Default::default()
    };
    RegisterClassExW(&wc);
}

unsafe fn create_controls(hwnd: HWND) {
    let hinst: HINSTANCE = GetModuleHandleW(None).ok().unwrap_or_default().into();

    // 栅栏区
    make_static(hwnd, hinst, 10, 10, 180, 18, "新建栅栏名称：");
    CreateWindowExW(
        WS_EX_CLIENTEDGE,
        w!("Edit"),
        w!(""),
        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | ES_LEFT as u32 | WS_BORDER.0),
        10, 30, 180, 24,
        hwnd,
        ctrl_id(ID_FENCE_NAME_EDIT),
        hinst,
        None,
    )
    .ok();
    make_button(hwnd, hinst, 10, 60, 88, 24, "新建栅栏", ID_BTN_ADD_FENCE);
    make_button(hwnd, hinst, 104, 60, 88, 24, "删除选中", ID_BTN_DEL_FENCE);
    CreateWindowExW(
        WS_EX_CLIENTEDGE,
        w!("ListBox"),
        w!(""),
        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_VSCROLL.0 | LBS_NOTIFY as u32),
        10, 92, 180, 200,
        hwnd,
        ctrl_id(ID_FENCE_LIST),
        hinst,
        None,
    )
    .ok();

    // 规则区
    make_static(hwnd, hinst, 200, 10, 190, 18, "规则-目标栅栏 id：");
    CreateWindowExW(
        WS_EX_CLIENTEDGE,
        w!("Edit"),
        w!(""),
        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | ES_LEFT as u32 | WS_BORDER.0),
        200, 30, 190, 24,
        hwnd,
        ctrl_id(ID_RULE_FENCE_EDIT),
        hinst,
        None,
    )
    .ok();
    make_static(hwnd, hinst, 200, 60, 190, 18, "规则-匹配模式：");
    CreateWindowExW(
        WS_EX_CLIENTEDGE,
        w!("Edit"),
        w!(""),
        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | ES_LEFT as u32 | WS_BORDER.0),
        200, 80, 190, 24,
        hwnd,
        ctrl_id(ID_RULE_PATTERN_EDIT),
        hinst,
        None,
    )
    .ok();
    make_button(hwnd, hinst, 200, 112, 92, 24, "新建规则", ID_BTN_ADD_RULE);
    make_button(hwnd, hinst, 298, 112, 92, 24, "删除选中", ID_BTN_DEL_RULE);
    CreateWindowExW(
        WS_EX_CLIENTEDGE,
        w!("ListBox"),
        w!(""),
        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_VSCROLL.0 | LBS_NOTIFY as u32),
        200, 144, 190, 150,
        hwnd,
        ctrl_id(ID_RULE_LIST),
        hinst,
        None,
    )
    .ok();

    // 底部
    make_button(hwnd, hinst, 10, 404, 90, 26, "重新加载", ID_BTN_RELOAD);
    make_button(hwnd, hinst, 110, 404, 100, 26, "保存并应用", ID_BTN_SAVE);
    make_button(hwnd, hinst, 220, 404, 90, 26, "关闭", ID_BTN_CLOSE);

    refresh_lists(hwnd);
}

unsafe fn make_static(
    parent: HWND,
    hinst: HINSTANCE,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    text: &str,
) -> HWND {
    CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("Static"),
        PCWSTR(to_wide(text).as_ptr()),
        WS_CHILD | WS_VISIBLE,
        x, y, w, h,
        parent,
        HMENU::default(),
        hinst,
        None,
    )
    .unwrap_or(HWND::default())
}

unsafe fn make_button(
    parent: HWND,
    hinst: HINSTANCE,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    text: &str,
    id: u32,
) -> HWND {
    CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("Button"),
        PCWSTR(to_wide(text).as_ptr()),
        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | BS_PUSHBUTTON as u32),
        x, y, w, h,
        parent,
        ctrl_id(id),
        hinst,
        None,
    )
    .unwrap_or(HWND::default())
}

unsafe fn get_state(hwnd: HWND) -> *mut SettingsState {
    GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SettingsState
}

unsafe fn get_edit_text(edit: HWND) -> String {
    if edit.is_invalid() {
        return String::new();
    }
    let len = SendMessageW(edit, WM_GETTEXTLENGTH, WPARAM(0), LPARAM(0)).0 as i32;
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; (len as usize) + 1];
    SendMessageW(edit, WM_GETTEXT, WPARAM(buf.len()), LPARAM(buf.as_mut_ptr() as isize));
    String::from_utf16_lossy(&buf[..len as usize])
}

unsafe fn refresh_lists(hwnd: HWND) {
    let state = get_state(hwnd);
    if state.is_null() {
        return;
    }
    let state = &*state;
    let cfg = state.config.lock();

    let fl = match GetDlgItem(hwnd, ID_FENCE_LIST as i32) {
        Ok(h) => h,
        Err(_) => return,
    };
    if !fl.is_invalid() {
        SendMessageW(fl, LB_RESETCONTENT, WPARAM(0), LPARAM(0));
        for f in &cfg.fences {
            let text = format!("{} ({})", f.name, f.id);
            SendMessageW(fl, LB_ADDSTRING, WPARAM(0), LPARAM(to_wide(&text).as_ptr() as isize));
        }
    }

    let rl = match GetDlgItem(hwnd, ID_RULE_LIST as i32) {
        Ok(h) => h,
        Err(_) => return,
    };
    if !rl.is_invalid() {
        SendMessageW(rl, LB_RESETCONTENT, WPARAM(0), LPARAM(0));
        for r in &cfg.sweep_rules {
            let text = format!("{} -> {}", r.fence_id, r.pattern);
            SendMessageW(rl, LB_ADDSTRING, WPARAM(0), LPARAM(to_wide(&text).as_ptr() as isize));
        }
    }
}

unsafe fn on_add_fence(hwnd: HWND) {
    let name = get_edit_text(GetDlgItem(hwnd, ID_FENCE_NAME_EDIT as i32).unwrap_or(HWND::default()));
    if name.trim().is_empty() {
        return;
    }
    let state = get_state(hwnd);
    if state.is_null() {
        return;
    }
    let state = &*state;
    let mut cfg = state.config.lock();
    let id = format!("fence-{}", cfg.fences.len() + 1);
    cfg.fences.push(Fence {
        id,
        name: name.trim().to_string(),
        rect: [100, 100, 280, 220],
        items: vec![],
    });
    let _ = cfg.save(&state.config_path);
    drop(cfg);
    refresh_lists(hwnd);
}

unsafe fn on_del_fence(hwnd: HWND) {
    let fl = match GetDlgItem(hwnd, ID_FENCE_LIST as i32) {
        Ok(h) => h,
        Err(_) => return,
    };
    if fl.is_invalid() {
        return;
    }
    let idx = SendMessageW(fl, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as i32;
    if idx < 0 {
        return;
    }
    let state = get_state(hwnd);
    if state.is_null() {
        return;
    }
    let state = &*state;
    let mut cfg = state.config.lock();
    if idx as usize >= cfg.fences.len() {
        return;
    }
    cfg.fences.remove(idx as usize);
    let _ = cfg.save(&state.config_path);
    drop(cfg);
    refresh_lists(hwnd);
}

unsafe fn on_add_rule(hwnd: HWND) {
    let fence_id = get_edit_text(GetDlgItem(hwnd, ID_RULE_FENCE_EDIT as i32).unwrap_or(HWND::default()));
    let pattern = get_edit_text(GetDlgItem(hwnd, ID_RULE_PATTERN_EDIT as i32).unwrap_or(HWND::default()));
    if fence_id.trim().is_empty() || pattern.trim().is_empty() {
        return;
    }
    let state = get_state(hwnd);
    if state.is_null() {
        return;
    }
    let state = &*state;
    let mut cfg = state.config.lock();
    cfg.sweep_rules.push(SweepRule {
        fence_id: fence_id.trim().to_string(),
        pattern: pattern.trim().to_string(),
        enabled: true,
    });
    let _ = cfg.save(&state.config_path);
    drop(cfg);
    refresh_lists(hwnd);
}

unsafe fn on_del_rule(hwnd: HWND) {
    let rl = match GetDlgItem(hwnd, ID_RULE_LIST as i32) {
        Ok(h) => h,
        Err(_) => return,
    };
    if rl.is_invalid() {
        return;
    }
    let idx = SendMessageW(rl, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as i32;
    if idx < 0 {
        return;
    }
    let state = get_state(hwnd);
    if state.is_null() {
        return;
    }
    let state = &*state;
    let mut cfg = state.config.lock();
    if idx as usize >= cfg.sweep_rules.len() {
        return;
    }
    cfg.sweep_rules.remove(idx as usize);
    let _ = cfg.save(&state.config_path);
    drop(cfg);
    refresh_lists(hwnd);
}

unsafe fn on_save(hwnd: HWND) {
    let state = get_state(hwnd);
    if state.is_null() {
        return;
    }
    let state = &*state;
    {
        let cfg = state.config.lock();
        let _ = cfg.save(&state.config_path);
    }
    let hwnds = state.hwnds.lock();
    for (_, h) in hwnds.iter() {
        let _ = InvalidateRect(*h, None, true);
    }
    drop(hwnds);
    let _ = MessageBoxW(hwnd, w!("已保存并应用到所有栅栏"), w!("栅栏设置"), MB_OK);
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as u32;
            match id {
                ID_BTN_ADD_FENCE => on_add_fence(hwnd),
                ID_BTN_DEL_FENCE => on_del_fence(hwnd),
                ID_BTN_ADD_RULE => on_add_rule(hwnd),
                ID_BTN_DEL_RULE => on_del_rule(hwnd),
                ID_BTN_RELOAD => refresh_lists(hwnd),
                ID_BTN_SAVE => on_save(hwnd),
                ID_BTN_CLOSE => {
                    let _ = DestroyWindow(hwnd);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SettingsState;
            if !ptr.is_null() {
                drop(Box::from_raw(ptr));
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
