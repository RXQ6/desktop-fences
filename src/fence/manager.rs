//! 多栅栏管理器
//!
//! 职责：
//! - 为每个 Fence 配置项创建一个分层窗口
//! - 注册 IDropTarget 到每个窗口
//! - 全局热键：Ctrl+Shift+H 切禅模式、Ctrl+Shift+S 桌面清扫
//! - 统一消息循环

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_CONTROL, MOD_SHIFT, HOT_KEY_MODIFIERS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetMessageW, DispatchMessageW, TranslateMessage, MSG, WM_HOTKEY,
    ShowWindow, SW_HIDE, SW_SHOWNOACTIVATE,
};

use crate::config::Config;
use crate::platform::desktop;
use crate::settings;

const HOTKEY_ZEN: i32 = 1;
const HOTKEY_SWEEP: i32 = 2;
const HOTKEY_SETTINGS: i32 = 3;

pub struct FenceManager {
    config: Arc<Mutex<Config>>,
    desktop: PathBuf,
    config_path: PathBuf,
    hwnds: Arc<Mutex<Vec<(String, HWND)>>>,
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
            hwnds: Arc::new(Mutex::new(vec![])),
            zen_mode: false,
        }
    }

    pub fn run(mut self) {
        unsafe {
            let hinstance =
                GetModuleHandleW(None).unwrap_or_else(|_| HMODULE(std::ptr::null_mut()));

            // 注册窗口类
            crate::fence::window::register_class(hinstance);

            // [诊断] 输出桌面窗口层级，确认图标层/WorkerW 的真实结构
            desktop::probe_desktop_layers();

            // 顶层窗口（parent = None）：确保稳定收到鼠标输入、可被拖动。
            // 不嵌入 WorkerW —— 嵌入式会被桌面图标层挡在后面，导致点击/拖动失效。
            let parent = HWND::default();

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

                self.hwnds.lock().push((fence.id.clone(), hwnd));
            }

            // 隐藏已归入栅栏的桌面图标（去重显示，避免原图标和栅栏里重复）
            desktop::hide_icons_for_names(&self.hidden_names());

            // 注册全局热键
            // Ctrl+Shift+H = 切禅模式 (H = 0x48)
            let _ = RegisterHotKey(HWND(std::ptr::null_mut()), HOTKEY_ZEN, HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_SHIFT.0), 0x48);
            // Ctrl+Shift+S = 桌面清扫 (S = 0x53)
            let _ = RegisterHotKey(HWND(std::ptr::null_mut()), HOTKEY_SWEEP, HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_SHIFT.0), 0x53);
            // Ctrl+Shift+O = 设置面板 (O = 0x4F)
            let _ = RegisterHotKey(HWND(std::ptr::null_mut()), HOTKEY_SETTINGS, HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_SHIFT.0), 0x4F);

            tracing::info!("已注册热键: Ctrl+Shift+H=禅模式, Ctrl+Shift+S=桌面清扫, Ctrl+Shift+O=设置");
            tracing::info!("{} 个栅栏已创建，进入消息循环", self.hwnds.lock().len());

            // 消息循环
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                if msg.message == WM_HOTKEY {
                    match msg.wParam.0 as i32 {
                        HOTKEY_ZEN => self.toggle_zen_mode(),
                        HOTKEY_SWEEP => self.sweep_desktop(),
                        HOTKEY_SETTINGS => {
                            settings::open(
                                self.config.clone(),
                                self.config_path.clone(),
                                self.hwnds.clone(),
                            );
                        }
                        _ => {}
                    }
                    continue;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            // 清理
            let _ = UnregisterHotKey(HWND::default(), HOTKEY_ZEN);
            let _ = UnregisterHotKey(HWND::default(), HOTKEY_SWEEP);
            let _ = UnregisterHotKey(HWND::default(), HOTKEY_SETTINGS);
            for (_, _) in self.hwnds.lock().iter() {
                // RevokeDragDrop 在窗口销毁时由 WindowState 处理
            }
        }
    }

    /// 收集当前所有已归入栅栏的文件名（用于隐藏桌面图标）
    fn hidden_names(&self) -> HashSet<String> {
        let cfg = self.config.lock();
        let mut set = HashSet::new();
        for f in &cfg.fences {
            for it in &f.items {
                set.insert(it.path.clone());
            }
        }
        set
    }

    fn toggle_zen_mode(&mut self) {
        self.zen_mode = !self.zen_mode;
        tracing::info!("禅模式: {}", if self.zen_mode { "开启（隐藏所有栅栏）" } else { "关闭" });
        let hwnds = self.hwnds.lock();
        unsafe {
            for (_, hwnd) in hwnds.iter() {
                if self.zen_mode {
                    let _ = ShowWindow(*hwnd, SW_HIDE);
                } else {
                    let _ = ShowWindow(*hwnd, SW_SHOWNOACTIVATE);
                }
            }
        }
        drop(hwnds);
        if self.zen_mode {
            // 禅模式：露出干净的原始桌面
            desktop::show_all_icons();
        } else {
            // 退出禅模式：重新隐藏已归入栅栏的图标
            desktop::hide_icons_for_names(&self.hidden_names());
        }
    }

    fn sweep_desktop(&self) {
        tracing::info!("执行桌面清扫");
        {
            let mut cfg = self.config.lock();
            cfg.sweep_all();
            let _ = cfg.save(&self.config_path);
        }

        // 触发所有窗口重绘
        let hwnds = self.hwnds.lock();
        unsafe {
            for (_, hwnd) in hwnds.iter() {
                let _ = InvalidateRect(*hwnd, None, true);
            }
        }
        drop(hwnds);

        // 按新分组重新隐藏桌面图标
        desktop::hide_icons_for_names(&self.hidden_names());
    }
}
