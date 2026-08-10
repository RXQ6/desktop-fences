//! 文件图标提取：用 SHGetFileInfoW

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::UI::Shell::{
    SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_USEFILEATTRIBUTES,
};
use windows::Win32::UI::WindowsAndMessaging::HICON;
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;

/// 取文件的图标。失败返回 None。
///
/// 注意：调用方负责 DestroyIcon 释放返回的 HICON。
pub fn get_file_icon(path: &Path) -> Option<HICON> {
    unsafe {
        let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        let mut wide_buf = wide;
        wide_buf.push(0);

        let mut shfi = SHFILEINFOW::default();
        let flags = SHGFI_ICON | SHGFI_LARGEICON | SHGFI_USEFILEATTRIBUTES;

        let result = SHGetFileInfoW(
            windows::core::PCWSTR(wide_buf.as_ptr()),
            FILE_ATTRIBUTE_NORMAL,
            Some(&mut shfi),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            flags,
        );

        if result != 0 && !shfi.hIcon.is_invalid() {
            Some(shfi.hIcon)
        } else {
            None
        }
    }
}
