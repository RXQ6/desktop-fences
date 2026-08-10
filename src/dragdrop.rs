//! OLE 拖放：手搓 COM vtable（绕过 windows 0.58 implement 宏的 trait bound 问题）
//!
//! 实现三个 COM 对象（全部用 `#[repr(C)]` + 静态 vtable，自己管引用计数）：
//! - `DropTarget`：作为窗口的拖放目标，接收 Explorer/桌面拖入的文件（CF_HDROP），
//!   把它们记录到对应栅栏的配置里（文件物理位置不变，只更新映射 —— 虚拟分组）。
//! - `DataObject`：作为拖拽源的数据对象，向 Explorer 提供 CF_HDROP。
//! - `DropSource`：拖拽源的"是否继续 / 反馈"回调。

use std::collections::HashSet;
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use windows::core::{GUID, HRESULT, Interface, IUnknown_Vtbl};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::{MODIFIERKEYS_FLAGS, MK_LBUTTON, MK_RBUTTON};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::Config;
use crate::platform::desktop;

// ---- 接口 IID（与 windows-rs define_interface! 生成的一致）----
const IID_IUNKNOWN: GUID = GUID::from_u128(0x0000_0000_0000_0000_c000_0000_0000_0046);
const IID_IDROPTARGET: GUID = GUID::from_u128(0x0000_0122_0000_0000_c000_0000_0000_0046);
const IID_IDATAOBJECT: GUID = GUID::from_u128(0x0000_010e_0000_0000_c000_0000_0000_0046);
const IID_IDROPSOURCE: GUID = GUID::from_u128(0x0000_0121_0000_0000_c000_0000_0000_0046);

// ---- 部分 HRESULT 常量（crate 未导出 DV_E_FORMATETC / DRAGDROP_S_*）----
const DV_E_FORMATETC: HRESULT = HRESULT(0x8004_0064_u32 as _);
const DRAGDROP_S_DROP: HRESULT = HRESULT(0x0004_0100_u32 as _);
const DRAGDROP_S_CANCEL: HRESULT = HRESULT(0x0004_0101_u32 as _);

const TIMER_ID_REPAINT: usize = 1;

// ============================ DropTarget ============================

#[repr(C)]
#[allow(non_snake_case)]
struct DropTarget {
    lpVtbl: *const IDropTarget_Vtbl,
    ref_count: AtomicU32,
    hwnd: HWND,
    fence_id: String,
    config: Arc<Mutex<Config>>,
    desktop: PathBuf,
    config_path: PathBuf,
}

static DROPTARGET_VTBL: IDropTarget_Vtbl = IDropTarget_Vtbl {
    base__: IUnknown_Vtbl {
        QueryInterface: drop_target_query_interface,
        AddRef: drop_target_add_ref,
        Release: drop_target_release,
    },
    DragEnter: drop_target_drag_enter,
    DragOver: drop_target_drag_over,
    DragLeave: drop_target_drag_leave,
    Drop: drop_target_drop,
};

unsafe extern "system" fn drop_target_query_interface(
    this: *mut c_void,
    iid: *const GUID,
    interface: *mut *mut c_void,
) -> HRESULT {
    if *iid == IID_IUNKNOWN || *iid == IID_IDROPTARGET {
        *interface = this;
        drop_target_add_ref(this);
        S_OK
    } else {
        *interface = std::ptr::null_mut();
        E_NOINTERFACE
    }
}

unsafe extern "system" fn drop_target_add_ref(this: *mut c_void) -> u32 {
    let obj = this as *mut DropTarget;
    (*obj).ref_count.fetch_add(1, Ordering::SeqCst) + 1
}

unsafe extern "system" fn drop_target_release(this: *mut c_void) -> u32 {
    let obj = this as *mut DropTarget;
    let prev = (*obj).ref_count.fetch_sub(1, Ordering::SeqCst);
    if prev == 1 {
        drop(Box::from_raw(obj));
        0
    } else {
        prev - 1
    }
}

unsafe extern "system" fn drop_target_drag_enter(
    _this: *mut c_void,
    _pdataobj: *mut c_void,
    _grfkeystate: MODIFIERKEYS_FLAGS,
    _pt: POINTL,
    pdweffect: *mut DROPEFFECT,
) -> HRESULT {
    *pdweffect = DROPEFFECT_COPY;
    S_OK
}

unsafe extern "system" fn drop_target_drag_over(
    _this: *mut c_void,
    _grfkeystate: MODIFIERKEYS_FLAGS,
    _pt: POINTL,
    pdweffect: *mut DROPEFFECT,
) -> HRESULT {
    *pdweffect = DROPEFFECT_COPY;
    S_OK
}

unsafe extern "system" fn drop_target_drag_leave(_this: *mut c_void) -> HRESULT {
    S_OK
}

unsafe extern "system" fn drop_target_drop(
    this: *mut c_void,
    pdataobj: *mut c_void,
    _grfkeystate: MODIFIERKEYS_FLAGS,
    _pt: POINTL,
    pdweffect: *mut DROPEFFECT,
) -> HRESULT {
    *pdweffect = DROPEFFECT_COPY;
    let obj = &*(this as *const DropTarget);

    // 把 COM 指针包成 IDataObject 用一下（我们不是它的 owner，用完 forget 防止误 Release）
    let data_obj = IDataObject::from_raw(pdataobj);
    let fmt = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let mut medium = match data_obj.GetData(&fmt) {
        Ok(m) => m,
        Err(_) => {
            std::mem::forget(data_obj); // 不是它的 owner，别 Release
            return S_OK;
        }
    };
    std::mem::forget(data_obj); // 不是它的 owner，用完 forget 防止误 Release

    if medium.tymed == TYMED_HGLOBAL.0 as u32 {
        let hdrop = HDROP(medium.u.hGlobal.0);
        if !hdrop.is_invalid() {
            let count = DragQueryFileW(hdrop, 0xFFFF_FFFF, None);
            let mut added = false;
            let mut moved_names: HashSet<String> = HashSet::new();
            for i in 0..count {
                let mut buf = [0u16; 260];
                let len = DragQueryFileW(hdrop, i, Some(&mut buf));
                if len > 0 {
                    let full = String::from_utf16_lossy(&buf[..len as usize]);
                    let fname = std::path::Path::new(&full)
                        .file_name()
                        .and_then(|f| f.to_str())
                        .unwrap_or("")
                        .to_string();
                    if !fname.is_empty() {
                        let mut cfg = obj.config.lock();
                        cfg.move_item_to_fence(&obj.fence_id, &fname);
                        let _ = cfg.save(&obj.config_path);
                        moved_names.insert(fname);
                        added = true;
                    }
                }
            }
            if added {
                // 立即重绘该栅栏
                let _ = PostMessageW(obj.hwnd, WM_TIMER, WPARAM(TIMER_ID_REPAINT), LPARAM(0));
                // 把刚归入栅栏的文件从桌面图标层隐藏（去重显示）
                desktop::hide_icons_for_names(&moved_names);
            }
        }
    }
    ReleaseStgMedium(&mut medium);
    S_OK
}

// ============================ DataObject ============================

#[repr(C)]
#[allow(non_snake_case)]
struct DataObject {
    lpVtbl: *const IDataObject_Vtbl,
    ref_count: AtomicU32,
    files: Vec<String>, // 绝对路径
}

static DATAOBJECT_VTBL: IDataObject_Vtbl = IDataObject_Vtbl {
    base__: IUnknown_Vtbl {
        QueryInterface: data_object_query_interface,
        AddRef: data_object_add_ref,
        Release: data_object_release,
    },
    GetData: data_object_get_data,
    GetDataHere: data_object_get_data_here,
    QueryGetData: data_object_query_get_data,
    GetCanonicalFormatEtc: data_object_get_canonical_format_etc,
    SetData: data_object_set_data,
    EnumFormatEtc: data_object_enum_format_etc,
    DAdvise: data_object_dadvise,
    DUnadvise: data_object_dunadvise,
    EnumDAdvise: data_object_enum_dadvise,
};

unsafe extern "system" fn data_object_query_interface(
    this: *mut c_void,
    iid: *const GUID,
    interface: *mut *mut c_void,
) -> HRESULT {
    if *iid == IID_IUNKNOWN || *iid == IID_IDATAOBJECT {
        *interface = this;
        data_object_add_ref(this);
        S_OK
    } else {
        *interface = std::ptr::null_mut();
        E_NOINTERFACE
    }
}

unsafe extern "system" fn data_object_add_ref(this: *mut c_void) -> u32 {
    let obj = this as *mut DataObject;
    (*obj).ref_count.fetch_add(1, Ordering::SeqCst) + 1
}

unsafe extern "system" fn data_object_release(this: *mut c_void) -> u32 {
    let obj = this as *mut DataObject;
    let prev = (*obj).ref_count.fetch_sub(1, Ordering::SeqCst);
    if prev == 1 {
        drop(Box::from_raw(obj));
        0
    } else {
        prev - 1
    }
}

unsafe extern "system" fn data_object_get_data(
    this: *mut c_void,
    pformatetc: *const FORMATETC,
    pmedium: *mut STGMEDIUM,
) -> HRESULT {
    let fmt = &*pformatetc;
    if fmt.cfFormat != CF_HDROP.0 || (fmt.tymed & (TYMED_HGLOBAL.0 as u32)) == 0 {
        return DV_E_FORMATETC;
    }
    let obj = &*(this as *const DataObject);
    let hglobal = build_hdrop(&obj.files);
    if hglobal.is_invalid() {
        return E_OUTOFMEMORY;
    }
    *pmedium = STGMEDIUM {
        tymed: TYMED_HGLOBAL.0 as u32,
        u: STGMEDIUM_0 { hGlobal: hglobal },
        pUnkForRelease: ManuallyDrop::new(None),
    };
    S_OK
}

unsafe extern "system" fn data_object_get_data_here(
    this: *mut c_void,
    pformatetc: *const FORMATETC,
    pmedium: *mut STGMEDIUM,
) -> HRESULT {
    data_object_get_data(this, pformatetc, pmedium)
}

unsafe extern "system" fn data_object_query_get_data(
    this: *mut c_void,
    pformatetc: *const FORMATETC,
) -> HRESULT {
    let _ = this;
    let fmt = &*pformatetc;
    if fmt.cfFormat == CF_HDROP.0 && (fmt.tymed & (TYMED_HGLOBAL.0 as u32)) != 0 {
        S_OK
    } else {
        DV_E_FORMATETC
    }
}

unsafe extern "system" fn data_object_get_canonical_format_etc(
    _this: *mut c_void,
    _pformatetc_in: *const FORMATETC,
    pformatetc_out: *mut FORMATETC,
) -> HRESULT {
    if !pformatetc_out.is_null() {
        *pformatetc_out = FORMATETC {
            cfFormat: 0,
            ptd: std::ptr::null_mut(),
            dwAspect: 0,
            lindex: 0,
            tymed: 0,
        };
    }
    S_OK
}

unsafe extern "system" fn data_object_set_data(
    _this: *mut c_void,
    _pformatetc: *const FORMATETC,
    _pmedium: *const STGMEDIUM,
    _frelease: BOOL,
) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn data_object_enum_format_etc(
    _this: *mut c_void,
    _dir: u32,
    ppenum: *mut *mut c_void,
) -> HRESULT {
    let fmts = [FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }];
    match SHCreateStdEnumFmtEtc(&fmts) {
        Ok(e) => {
            *ppenum = &e as *const IEnumFORMATETC as *mut c_void;
            std::mem::forget(e); // 把枚举器所有权转交给调用方
            S_OK
        }
        Err(_) => E_OUTOFMEMORY,
    }
}

unsafe extern "system" fn data_object_dadvise(
    _this: *mut c_void,
    _pformatetc: *const FORMATETC,
    _advf: u32,
    _advsink: *mut c_void,
    _pdwconnection: *mut u32,
) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn data_object_dunadvise(_this: *mut c_void, _dwconnection: u32) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn data_object_enum_dadvise(
    _this: *mut c_void,
    _ppenumadvise: *mut *mut c_void,
) -> HRESULT {
    E_NOTIMPL
}

/// 构造一个 HDROP（DROPFILES + 双 NUL 结尾的宽字符文件列表）
unsafe fn build_hdrop(files: &[String]) -> HGLOBAL {
    let mut data: Vec<u16> = Vec::new();
    for f in files {
        data.extend_from_slice(&f.encode_utf16().collect::<Vec<u16>>());
        data.push(0);
    }
    data.push(0); // 末尾额外 NUL

    let header = std::mem::size_of::<DROPFILES>() as usize;
    let total = header + data.len() * 2;

    let hglobal = match GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, total) {
        Ok(h) => h,
        Err(_) => return HGLOBAL::default(),
    };
    let ptr = GlobalLock(hglobal);
    if ptr.is_null() {
        return HGLOBAL::default();
    }
    let df = DROPFILES {
        pFiles: header as u32,
        pt: POINT { x: 0, y: 0 },
        fNC: BOOL(0),
        fWide: BOOL(1),
    };
    std::ptr::write(ptr as *mut DROPFILES, df);
    let data_ptr = (ptr as *mut u8).add(header) as *mut u16;
    std::ptr::copy_nonoverlapping(data.as_ptr(), data_ptr, data.len());
    let _ = GlobalUnlock(hglobal);
    hglobal
}

// ============================ DropSource ============================

#[repr(C)]
#[allow(non_snake_case)]
struct DropSource {
    lpVtbl: *const IDropSource_Vtbl,
    ref_count: AtomicU32,
}

static DROPSOURCE_VTBL: IDropSource_Vtbl = IDropSource_Vtbl {
    base__: IUnknown_Vtbl {
        QueryInterface: drop_source_query_interface,
        AddRef: drop_source_add_ref,
        Release: drop_source_release,
    },
    QueryContinueDrag: drop_source_query_continue_drag,
    GiveFeedback: drop_source_give_feedback,
};

unsafe extern "system" fn drop_source_query_interface(
    this: *mut c_void,
    iid: *const GUID,
    interface: *mut *mut c_void,
) -> HRESULT {
    if *iid == IID_IUNKNOWN || *iid == IID_IDROPSOURCE {
        *interface = this;
        drop_source_add_ref(this);
        S_OK
    } else {
        *interface = std::ptr::null_mut();
        E_NOINTERFACE
    }
}

unsafe extern "system" fn drop_source_add_ref(this: *mut c_void) -> u32 {
    let obj = this as *mut DropSource;
    (*obj).ref_count.fetch_add(1, Ordering::SeqCst) + 1
}

unsafe extern "system" fn drop_source_release(this: *mut c_void) -> u32 {
    let obj = this as *mut DropSource;
    let prev = (*obj).ref_count.fetch_sub(1, Ordering::SeqCst);
    if prev == 1 {
        drop(Box::from_raw(obj));
        0
    } else {
        prev - 1
    }
}

unsafe extern "system" fn drop_source_query_continue_drag(
    _this: *mut c_void,
    fescapepressed: BOOL,
    grfkeystate: MODIFIERKEYS_FLAGS,
) -> HRESULT {
    if fescapepressed.as_bool() {
        return DRAGDROP_S_CANCEL;
    }
    // 左键松开 = 落下；右键松开 = 取消
    if (grfkeystate.0 & MK_LBUTTON.0) == 0 && (grfkeystate.0 & MK_RBUTTON.0) == 0 {
        return DRAGDROP_S_DROP;
    }
    if (grfkeystate.0 & MK_RBUTTON.0) != 0 {
        return DRAGDROP_S_CANCEL;
    }
    S_OK
}

unsafe extern "system" fn drop_source_give_feedback(_this: *mut c_void, _dweffect: DROPEFFECT) -> HRESULT {
    S_OK
}

// ============================ 对外 API ============================

/// 为窗口注册拖放目标。返回 COM 对象裸指针，供 unregister 时释放。
pub unsafe fn register_drop_target(
    hwnd: HWND,
    fence_id: String,
    config: Arc<Mutex<Config>>,
    desktop: PathBuf,
    config_path: PathBuf,
) -> *mut c_void {
    let boxed = Box::new(DropTarget {
        lpVtbl: &DROPTARGET_VTBL,
        ref_count: AtomicU32::new(1),
        hwnd,
        fence_id,
        config,
        desktop,
        config_path,
    });
    let raw = Box::into_raw(boxed);
    let idrop = IDropTarget::from_raw(raw as *mut c_void);
    let _ = RegisterDragDrop(hwnd, &idrop);
    std::mem::forget(idrop); // 防止 wrapper drop 误 Release（OLE 仍持有裸指针）
    raw as *mut c_void
}

/// 撤销拖放目标并释放 COM 对象。
pub unsafe fn unregister_drop_target(hwnd: HWND, raw: *mut c_void) {
    let _ = RevokeDragDrop(hwnd);
    if !raw.is_null() {
        let idrop = IDropTarget::from_raw(raw);
        drop(idrop); // Release -> refcount 0 -> 释放
    }
}

/// 发起一次拖拽（在 WM_LBUTTONDOWN 里调用，会阻塞到拖拽结束）。
pub unsafe fn start_drag(hwnd: HWND, files: Vec<String>) {
    let data_raw = Box::into_raw(Box::new(DataObject {
        lpVtbl: &DATAOBJECT_VTBL,
        ref_count: AtomicU32::new(1),
        files,
    }));
    let src_raw = Box::into_raw(Box::new(DropSource {
        lpVtbl: &DROPSOURCE_VTBL,
        ref_count: AtomicU32::new(1),
    }));

    let idata = IDataObject::from_raw(data_raw as *mut c_void);
    let isrc = IDropSource::from_raw(src_raw as *mut c_void);
    let mut effect = DROPEFFECT_NONE;
    let _ = DoDragDrop(&idata, &isrc, DROPEFFECT_COPY, &mut effect);
    drop(idata);
    drop(isrc);
    // DoDragDrop 结束后引用计数回到 1，上面 drop 触发 Release -> 释放
    let _ = hwnd;
}

// ============================ 测试（headless 下验证 HDROP 字节布局/数据路径）============================

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::Shell::HDROP;

    /// 直接验证 build_hdrop 生成的 HGLOBAL 能被 DragQueryFileW 正确解析。
    #[test]
    fn hdrop_bytes_roundtrip() {
        unsafe {
            let files = vec![
                "C:\\Users\\test\\a.txt".to_string(),
                "C:\\Users\\test\\子目录\\b.png".to_string(),
            ];
            let hglobal = build_hdrop(&files);
            assert!(!hglobal.is_invalid(), "GlobalAlloc 应成功");

            let hdrop = HDROP(hglobal.0);
            let count = DragQueryFileW(hdrop, 0xFFFF_FFFF, None);
            assert_eq!(count, 2, "应解析出 2 个文件");

            let mut buf = [0u16; 260];
            let len = DragQueryFileW(hdrop, 0, Some(&mut buf));
            let first = String::from_utf16_lossy(&buf[..len as usize]);
            assert_eq!(first, "C:\\Users\\test\\a.txt");

            let len2 = DragQueryFileW(hdrop, 1, Some(&mut buf));
            let second = String::from_utf16_lossy(&buf[..len2 as usize]);
            assert_eq!(second, "C:\\Users\\test\\子目录\\b.png");

            let _ = GlobalFree(hglobal);
        }
    }

    /// 走完整 DataObject COM 路径：from_raw -> GetData -> 解析 HDROP -> ReleaseStgMedium -> drop。
    #[test]
    fn data_object_get_data_roundtrip() {
        unsafe {
            let files = vec!["C:\\Users\\test\\拖放源.txt".to_string()];
            let raw = Box::into_raw(Box::new(DataObject {
                lpVtbl: &DATAOBJECT_VTBL,
                ref_count: AtomicU32::new(1),
                files,
            }));
            let idata = IDataObject::from_raw(raw as *mut c_void);

            let fmt = FORMATETC {
                cfFormat: CF_HDROP.0,
                ptd: std::ptr::null_mut(),
                dwAspect: DVASPECT_CONTENT.0,
                lindex: -1,
                tymed: TYMED_HGLOBAL.0 as u32,
            };

            let mut medium = idata.GetData(&fmt).expect("GetData 应成功返回 STGMEDIUM");
            assert_eq!(medium.tymed, TYMED_HGLOBAL.0 as u32, "tymed 应为 HGLOBAL");

            let hdrop = HDROP(medium.u.hGlobal.0);
            let count = DragQueryFileW(hdrop, 0xFFFF_FFFF, None);
            assert_eq!(count, 1);
            let mut buf = [0u16; 260];
            let len = DragQueryFileW(hdrop, 0, Some(&mut buf));
            let name = String::from_utf16_lossy(&buf[..len as usize]);
            assert_eq!(name, "C:\\Users\\test\\拖放源.txt");

            ReleaseStgMedium(&mut medium);
            drop(idata); // Release -> refcount 0 -> 释放 box（无泄漏）
        }
    }
}

