//! 把目录丢进回收站（PCL CE 的删除实例默认就是这么干的，还能捞回来）。

/// 用 `SHFileOperationW` 删除到回收站。成功返回 true；
/// 失败（比如被占用、驱动器不支持回收站）返回 false，调用方自己决定要不要硬删。
#[cfg(windows)]
pub fn to_recycle_bin(path: &std::path::Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW,
        SHFileOperationW,
    };

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    // SHFileOperationW 要求双 \0 结尾。
    wide.push(0);
    wide.push(0);
    let mut operation: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
    operation.wFunc = FO_DELETE as u32;
    operation.pFrom = wide.as_ptr();
    operation.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI) as u16;
    let code = unsafe { SHFileOperationW(&mut operation) };
    code == 0 && operation.fAnyOperationsAborted == 0
}

#[cfg(not(windows))]
pub fn to_recycle_bin(_path: &std::path::Path) -> bool {
    false
}
