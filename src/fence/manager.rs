//! 多栅栏管理器
//!
//! 职责：
//! - 为每个 Fence 配置项创建一个分层窗口
//! - 注册 IDropTarget 到每个窗口
//! - 全局热键：Ctrl+Shift+H 切禅模式、Ctrl+Shift+S 桌面清扫
//! - 统一消息循环

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
// OLE 拖放暂时禁用
// use windows::Win32::System::Ole::{IDropTarget, RegisterDragDrop, RevokeDragDrop};
// use crate::dragdrop::target::DropTarget;
// use crate::dragdrop::DragContext;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_CONTROL, MOD_SHIFT, HOT_KEY_MODIFIERS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetMessageW, DispatchMessageW, TranslateMessage, MSG, WM_HOTKEY,
    ShowWindow, SW_HIDE, SW_SHOWNOACTIVATE,
};

use crate::config::Config;
// OLE 拖放暂时禁用
// use crate::dragdrop::target::DropTarget;
// use crate::dragdrop::DragContext;
use crate::platform::desktop;

const HOTKEY_ZEN: i32 = 1;
const HOTKEY_SWEEP: i32 = 2;

pub struct FenceManager {
    config: Arc<Mutex<Config>>,
    desktop: PathBuf,
    config_path: PathBuf,
    hwnds: Vec<(String, HWND)>,
    zen_mode: bool,
}

impl FenceManager {
    pub fn new(
        config: Arc<Mutex<Config>>,
        desktop: PathBuf,
        config_path: PathBuf,
    ) -> Self {
        Self {
            config,
            desktop,
            config_path,
            hwnds: vec![],
            zen_mode: false,
        }
    }

    pub fn run(mut self) {
        unsafe {
            let hinstance =
                GetModuleHandleW(None).unwrap_or_else(|_| HMODULE(std::ptr::null_mut()));

            // 注册窗口类
            crate::fence::window::register_class(hinstance);

            // 找 WorkerW
            let parent = desktop::find_or_create_workerw()
                .unwrap_or_else(|| HWND(std::ptr::null_mut()));

            // 为每个 fence 创建窗口 + 注册拖放
            let fences: Vec<crate::config::Fence> = {
                let cfg = self.config.lock();
                cfg.fences.clone()
            };

            for fence in &fences {
                let hwnd = crate::fence::window::create_fence_window(
                    hinstance,
                    parent,
                    fence,
                    self.config.clone(),
                    self.desktop.clone(),
                    self.config_path.clone(),
                );
                if hwnd.is_invalid() {
                    continue;
                }

                // OLE 拖放暂时禁用
                // let ctx = DragContext { ... };
                // let drop_target: IDropTarget = DropTarget::new(ctx).into();
                // let _ = RegisterDragDrop(hwnd, &drop_target);

                self.hwnds.push((fence.id.clone(), hwnd));
            }

            // 注册全局热键
            // Ctrl+Shift+H = 切禅模式 (H = 0x48)
            let _ = RegisterHotKey(HWND(std::ptr::null_mut()), HOTKEY_ZEN, HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_SHIFT.0), 0x48);
            // Ctrl+Shift+S = 桌面清扫 (S = 0x53)
            let _ = RegisterHotKey(HWND(std::ptr::null_mut()), HOTKEY_SWEEP, HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_SHIFT.0), 0x53);

            tracing::info!("已注册热键: Ctrl+Shift+H=禅模式, Ctrl+Shift+S=桌面清扫");
            tracing::info!("{} 个栅栏已创建，进入消息循环", self.hwnds.len());

            // 消息循环
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                if msg.message == WM_HOTKEY {
                    match msg.wParam.0 as i32 {
                        HOTKEY_ZEN => self.toggle_zen_mode(),
                        HOTKEY_SWEEP => self.sweep_desktop(),
                        _ => {}
                    }
                    continue;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            // 清理
            let _ = UnregisterHotKey(None, HOTKEY_ZEN);
            let _ = UnregisterHotKey(None, HOTKEY_SWEEP);
            for (_, _) in &self.hwnds {
                // let _ = RevokeDragDrop(*hwnd);  // OLE 暂时禁用
            }
        }
    }

    fn toggle_zen_mode(&mut self) {
        self.zen_mode = !self.zen_mode;
        tracing::info!("禅模式: {}", if self.zen_mode { "开启（隐藏所有栅栏）" } else { "关闭" });
        unsafe {
            for (_, hwnd) in &self.hwnds {
                if self.zen_mode {
                    let _ = ShowWindow(*hwnd, SW_HIDE);
                } else {
                    let _ = ShowWindow(*hwnd, SW_SHOWNOACTIVATE);
                }
            }
        }
    }

    fn sweep_desktop(&self) {
        tracing::info!("执行桌面清扫");
        let mut cfg = self.config.lock();
        cfg.sweep_all();
        let _ = cfg.save(&self.config_path);
        drop(cfg);

        // 触发所有窗口重绘
        unsafe {
            for (_, hwnd) in &self.hwnds {
                let _ = InvalidateRect(*hwnd, None, true);
            }
        }
    }
}
