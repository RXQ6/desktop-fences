//! 桌面文件监听：ReadDirectoryChangesW
//!
//! 用异步 I/O 监听桌面目录的变化。
//! 任何变化都触发对账（简化策略，未来可以做更精细的事件处理）。

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::*;

use crate::config::Config;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct FileNotifyInformation {
    next_entry_offset: u32,
    action: u32,
    file_name_length: u32,
    // 后面跟着 WCHAR file_name[]
}

/// 启动监听线程
pub fn spawn_watcher(
    desktop: PathBuf,
    config: Arc<Mutex<Config>>,
    config_path: PathBuf,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        watch_loop(&desktop, &config, &config_path);
    })
}

fn watch_loop(desktop: &Path, config: &Arc<Mutex<Config>>, config_path: &Path) {
    unsafe {
        let wide: Vec<u16> = desktop.as_os_str().encode_wide().collect();
        let mut wide_buf = wide;
        wide_buf.push(0);

        let handle = CreateFileW(
            windows::core::PCWSTR(wide_buf.as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
            None,
        );

        let handle = match handle {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!("无法打开桌面目录进行监听: {}", e);
                return;
            }
        };

        let mut buf = vec![0u8; 8192];
        let mut overlapped = OVERLAPPED::default();
        overlapped.hEvent = match CreateEventW(None, true, false, None) {
            Ok(h) => h,
            Err(_) => return,
        };

        tracing::info!("文件监听已启动");

        loop {
            // 发起异步读取
            let ok = ReadDirectoryChangesW(
                handle,
                buf.as_mut_ptr() as *mut _,
                buf.len() as u32,
                true,
                FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_DIR_NAME
                    | FILE_NOTIFY_CHANGE_LAST_WRITE,
                None,
                Some(&mut overlapped as *mut OVERLAPPED),
                None,
            );

            if ok.is_err() {
                tracing::warn!("ReadDirectoryChangesW 调用失败");
                break;
            }

            // 等待事件
            let wait_result = WaitForSingleObject(overlapped.hEvent, INFINITE);
            if wait_result != WAIT_OBJECT_0 {
                tracing::warn!("等待监听事件失败: {:?}", wait_result);
                break;
            }

            // 重置事件
            let _ = ResetEvent(overlapped.hEvent);

            // 获取实际读取的字节数
            let mut bytes_transferred: u32 = 0;
            let _ = GetOverlappedResult(handle, &overlapped, &mut bytes_transferred, false);

            if bytes_transferred == 0 {
                continue;
            }

            // 解析 FILE_NOTIFY_INFORMATION 链表
            let mut offset = 0usize;
            loop {
                if offset + 12 > bytes_transferred as usize {
                    break;
                }
                let ptr = buf.as_ptr().add(offset) as *const FileNotifyInformation;
                let entry = &*ptr;
                let action = entry.action;
                let name_len = entry.file_name_length as usize;
                let name_wchars = name_len / 2;
                let name_ptr = (ptr as *const u8).add(12) as *const u16;
                let name_slice = std::slice::from_raw_parts(name_ptr, name_wchars);
                let name = String::from_utf16_lossy(name_slice);

                handle_change(action, &name, desktop, config, config_path);

                if entry.next_entry_offset == 0 {
                    break;
                }
                offset += entry.next_entry_offset as usize;
            }
        }

        let _ = CloseHandle(handle);
    }
}

unsafe fn handle_change(
    action: u32,
    name: &str,
    desktop: &Path,
    config: &Arc<Mutex<Config>>,
    config_path: &Path,
) {
    let action_str = match action {
        1 => "ADDED",
        2 => "REMOVED",
        3 => "MODIFIED",
        4 => "RENAMED_OLD",
        5 => "RENAMED_NEW",
        _ => "UNKNOWN",
    };
    tracing::info!("文件变更: {} {}", action_str, name);

    // 简化策略：任何变更都触发对账
    let mut cfg = config.lock();
    cfg.reconcile(desktop);
    let _ = cfg.save(config_path);
}
