//! 栅栏渲染：用 GDI 绘制背景 + 图标网格 + 文字
//!
//! 注意：分层窗口的 DIB 是 32-bit ARGB，需要 premultiplied alpha。
//! GDI 的 FillRect/CreateSolidBrush 不直接支持 alpha，
//! 所以这里用了一种简化做法：纯色背景用 GDI 画，
//! 然后靠 SourceConstantAlpha 控制整体透明度。

use std::path::Path;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::{Config, Fence};

// 配色（BGR for COLORREF）
const FENCE_BG: u32 = 0x3C_2A_2A; // 深蓝灰
const FENCE_BORDER: u32 = 0x66_63_F1; // 紫色边框 (BGR)
const TEXT_COLOR: u32 = 0xF9_F5_F1; // 浅色文字 (BGR)
const HEADER_BG: u32 = 0x2A_1F_3C; // 标题栏背景
const EMPTY_HINT: u32 = 0xB8_A3_94; // 空栅栏提示色

pub const HEADER_H: i32 = 28;
pub const ICON_SIZE: i32 = 48;
pub const ICON_GAP: i32 = 8;
pub const TEXT_H: i32 = 16;
pub const CELL_W: i32 = ICON_SIZE + ICON_GAP;
pub const CELL_H: i32 = ICON_SIZE + 4 + TEXT_H;
pub const GRID_PAD: i32 = 4;

pub fn render_fence_by_id(
    hdc: HDC,
    width: i32,
    height: i32,
    config: &Config,
    desktop: &Path,
    fence_id: &str,
) {
    unsafe {
        // 1. 填充背景（半透明深色）
        fill_rect(hdc, 0, 0, width, height, FENCE_BG);

        // 2. 标题栏背景
        fill_rect(hdc, 0, 0, width, HEADER_H, HEADER_BG);

        // 找指定栅栏
        let fence = config.fences.iter().find(|f| f.id == fence_id);

        // 3. 标题文字
        let title = fence
            .map(|f| f.name.as_str())
            .unwrap_or("栅栏");
        draw_text(hdc, title, 12, 6, HEADER_H - 6, TEXT_COLOR);

        // 4. 边框
        draw_border(hdc, 0, 0, width - 1, height - 1, FENCE_BORDER);

        // 5. 网格区域
        let grid_y = HEADER_H + GRID_PAD;
        let grid_h = height - grid_y - GRID_PAD;
        let grid_w = width - 2 * GRID_PAD;
        let cols = (grid_w / CELL_W).max(1);
        let rows = (grid_h / CELL_H).max(1);
        let max_items = (cols * rows) as usize;

        // 6. 取栅栏里的文件
        let items: Vec<String> = fence
            .map(|f| f.items.iter().map(|i| i.path.clone()).collect())
            .unwrap_or_default();

        // 7. 渲染每个文件
        for (i, path) in items.iter().take(max_items).enumerate() {
            let col = (i as i32) % cols;
            let row = (i as i32) / cols;
            let x = GRID_PAD + col * CELL_W;
            let y = grid_y + row * CELL_H;

            let full_path = desktop.join(path);

            // 取图标，失败画占位矩形
            if let Some(hicon) = crate::fs::icon::get_file_icon(&full_path) {
                draw_icon(hdc, hicon, x, y, ICON_SIZE);
                let _ = DestroyIcon(hicon);
            } else {
                fill_rect(hdc, x, y, x + ICON_SIZE, y + ICON_SIZE, 0x55_47_69);
            }

            // 文件名（截断到 9 字符 + …）
            let name = full_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?");
            let display = truncate_name(name, 9);
            draw_text(hdc, &display, x, y + ICON_SIZE + 2, TEXT_H, TEXT_COLOR);
        }

        // 8. 空栅栏提示
        if items.is_empty() {
            let hint = "(空栅栏 — Phase 2 才支持拖入)";
            let hint_y = height / 2 - 8;
            draw_text(hdc, hint, 12, hint_y, 16, EMPTY_HINT);
        }

        // 9. 底部状态栏
        let status = format!("{} 个文件", items.len());
        draw_text(hdc, &status, 8, height - 18, 14, EMPTY_HINT);
    }
}

fn truncate_name(name: &str, max_chars: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max_chars {
        name.to_string()
    } else {
        let mut s: String = chars[..max_chars - 1].iter().collect();
        s.push('…');
        s
    }
}

unsafe fn fill_rect(hdc: HDC, x1: i32, y1: i32, x2: i32, y2: i32, color: u32) {
    let brush = CreateSolidBrush(COLORREF(color));
    let rect = RECT {
        left: x1,
        top: y1,
        right: x2,
        bottom: y2,
    };
    let _ = FillRect(hdc, &rect, brush);
    let _ = DeleteObject(brush);
}

unsafe fn draw_border(hdc: HDC, x1: i32, y1: i32, x2: i32, y2: i32, color: u32) {
    let pen = CreatePen(PS_SOLID, 2, COLORREF(color));
    let old = SelectObject(hdc, pen);
    let _ = MoveToEx(hdc, x1, y1, None);
    let _ = LineTo(hdc, x2, y1);
    let _ = LineTo(hdc, x2, y2);
    let _ = LineTo(hdc, x1, y2);
    let _ = LineTo(hdc, x1, y1);
    SelectObject(hdc, old);
    let _ = DeleteObject(pen);
}

unsafe fn draw_text(hdc: HDC, text: &str, x: i32, y: i32, h: i32, color: u32) {
    let _ = SetTextColor(hdc, COLORREF(color));
    let _ = SetBkMode(hdc, TRANSPARENT);

    let mut wide: Vec<u16> = text.encode_utf16().collect();
    let mut rect = RECT {
        left: x,
        top: y,
        right: x + 400,
        bottom: y + h,
    };
    let _ = DrawTextW(
        hdc,
        &mut wide,
        &mut rect,
        DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS,
    );
}

unsafe fn draw_icon(hdc: HDC, hicon: HICON, x: i32, y: i32, size: i32) {
    let _ = DrawIconEx(hdc, x, y, hicon, size, size, 0, None, DI_NORMAL);
}

/// 命中测试：给定栅栏窗口内点击坐标，返回对应的 items 下标（无命中返回 None）。
/// 网格布局必须与 render_fence_by_id 保持一致。
pub fn hit_test_item(fence: &Fence, x: i32, y: i32, width: i32, height: i32) -> Option<usize> {
    let grid_y = HEADER_H + GRID_PAD;
    if y < grid_y {
        return None;
    }
    let grid_w = width - 2 * GRID_PAD;
    let grid_h = height - grid_y - GRID_PAD;
    let cols = (grid_w / CELL_W).max(1);
    let rows = (grid_h / CELL_H).max(1);
    let max_items = (cols * rows) as usize;
    if max_items == 0 {
        return None;
    }
    let col = ((x - GRID_PAD) / CELL_W).clamp(0, cols - 1);
    let row = ((y - grid_y) / CELL_H).clamp(0, rows - 1);
    let idx = (row * cols + col) as usize;
    if idx < max_items && idx < fence.items.len() {
        Some(idx)
    } else {
        None
    }
}
