//! 往 Windows 剪贴板里放/取「文件列表」（CF_HDROP）。
//!
//! 之前用 `powershell -Command Set-Clipboard`，每次要起一个 PowerShell，点一下要等两三秒。
//! 这里直接调 Win32：`SetClipboardData(CF_HDROP)` / `GetClipboardData(CF_HDROP)`。

use std::path::PathBuf;

pub fn set_files(paths: &[PathBuf]) -> Result<(), String> {
    unsafe { set_files_inner(paths) }
}

pub fn get_files() -> Result<Vec<PathBuf>, String> {
    unsafe { get_files_inner() }
}

#[cfg(windows)]
unsafe fn set_files_inner(paths: &[PathBuf]) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows_sys::Win32::System::Ole::CF_HDROP;
    use windows_sys::Win32::UI::Shell::DROPFILES;

    if paths.is_empty() {
        return Err("没有要复制的文件".into());
    }

    // DROPFILES 后面跟一串 UTF-16 路径，每条以 \0 结尾，最后再来一个 \0。
    let mut wide: Vec<u16> = Vec::new();
    for path in paths {
        wide.extend(path.as_os_str().encode_wide());
        wide.push(0);
    }
    wide.push(0);

    let header = std::mem::size_of::<DROPFILES>();
    let total = header + wide.len() * 2;
    let memory = GlobalAlloc(GMEM_MOVEABLE, total);
    if memory.is_null() {
        return Err("申请剪贴板内存失败".into());
    }
    let base = GlobalLock(memory) as *mut u8;
    if base.is_null() {
        return Err("锁定剪贴板内存失败".into());
    }
    std::ptr::write_bytes(base, 0, total);
    let drop = base as *mut DROPFILES;
    (*drop).pFiles = header as u32;
    (*drop).fWide = 1;
    std::ptr::copy_nonoverlapping(wide.as_ptr(), base.add(header) as *mut u16, wide.len());
    let _ = GlobalUnlock(memory);

    if OpenClipboard(std::ptr::null_mut()) == 0 {
        return Err("打不开剪贴板".into());
    }
    let _ = EmptyClipboard();
    let placed = SetClipboardData(CF_HDROP as u32, memory);
    let _ = CloseClipboard();
    if placed.is_null() {
        return Err("剪贴板没写进去".into());
    }
    Ok(())
}

#[cfg(windows)]
unsafe fn get_files_inner() -> Result<Vec<PathBuf>, String> {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    };
    use windows_sys::Win32::System::Ole::CF_HDROP;
    use windows_sys::Win32::UI::Shell::DragQueryFileW;

    if OpenClipboard(std::ptr::null_mut()) == 0 {
        return Err("打不开剪贴板".into());
    }
    let result = (|| {
        if IsClipboardFormatAvailable(CF_HDROP as u32) == 0 {
            return Ok(Vec::new());
        }
        let handle = GetClipboardData(CF_HDROP as u32);
        if handle.is_null() {
            return Ok(Vec::new());
        }
        let drop = handle;
        let count = DragQueryFileW(drop, u32::MAX, std::ptr::null_mut(), 0);
        let mut paths = Vec::new();
        for index in 0..count {
            let len = DragQueryFileW(drop, index, std::ptr::null_mut(), 0);
            if len == 0 {
                continue;
            }
            let mut buffer = vec![0_u16; len as usize + 1];
            let written = DragQueryFileW(drop, index, buffer.as_mut_ptr(), buffer.len() as u32);
            if written == 0 {
                continue;
            }
            buffer.truncate(written as usize);
            paths.push(PathBuf::from(String::from_utf16_lossy(&buffer)));
        }
        Ok(paths)
    })();
    let _ = CloseClipboard();
    result
}

#[cfg(not(windows))]
unsafe fn set_files_inner(_paths: &[PathBuf]) -> Result<(), String> {
    Err("只有 Windows 支持复制文件".into())
}

#[cfg(not(windows))]
unsafe fn get_files_inner() -> Result<Vec<PathBuf>, String> {
    Ok(Vec::new())
}
