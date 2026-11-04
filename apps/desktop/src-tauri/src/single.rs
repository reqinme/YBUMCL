//! 单实例：同一个数据目录只允许跑一个启动器。
//!
//! 便携版的「实例」是数据目录，不是 exe——两份 exe 指向同一个数据目录
//! 才会互相打架，所以互斥键用数据目录路径而不是全局唯一名字。
//! 第二个进程不会静默退出，而是把已有窗口拉到前台，用户感觉是「点了一下」。
#[cfg(windows)]
pub fn acquire_or_focus(data_dir: &std::path::Path) -> bool {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let name = format!("Local\\ybumcl-{:016x}", hash_path(data_dir));
    let wide: Vec<u16> = OsStr::new(&name).encode_wide().chain(Some(0)).collect();
    // SAFETY: 传的是以 NUL 结尾的宽字符串；句柄在进程结束前一直有效。
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, wide.as_ptr());
        if handle.is_null() {
            // 建不出互斥量就别拦着用户启动。
            return true;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            focus_existing_window();
            return false;
        }
        // 不 CloseHandle：句柄留到进程退出由系统释放（这里只是忽略返回值）。
        let _ = handle;
    }
    true
}

#[cfg(windows)]
fn focus_existing_window() {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SW_RESTORE, SetForegroundWindow, ShowWindow,
    };

    let title = crate::portable::WINDOW_TITLE;
    let wide: Vec<u16> = OsStr::new(title).encode_wide().chain(Some(0)).collect();
    // SAFETY: 类名传 null、标题是 NUL 结尾的宽字符串；窗口句柄只在本进程查询后立刻使用。
    unsafe {
        let hwnd = FindWindowW(std::ptr::null(), wide.as_ptr());
        if !hwnd.is_null() {
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
        }
    }
}

#[cfg(windows)]
fn hash_path(path: &std::path::Path) -> u64 {
    // FNV-1a：不需要稳定跨平台，只要同一台机器上同路径算出同一个数。
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let text = path.to_string_lossy().to_lowercase();
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(not(windows))]
pub fn acquire_or_focus(_data_dir: &std::path::Path) -> bool {
    true
}
