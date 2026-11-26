//! Windows 内存优化：清空系统文件缓存与待机列表。
//!
//! 和 PCL 的「启动前进行内存优化」是同一类操作：提权后用 `NtSetSystemInformation`
//! 让内核把暂不用的物理页还回来。这些文件页下次访问要重新从磁盘读，所以机械硬盘上
//! 会卡一小段——界面上的开关默认是关的，只有用户自己开才会跑。

#![allow(non_snake_case, non_camel_case_types)]

use std::ptr;

type HANDLE = isize;
type BOOL = i32;
type NTSTATUS = i32;

const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_ADJUST_PRIVILEGES: u32 = 0x0020;
const SE_PRIVILEGE_ENABLED: u32 = 0x0000_0002;
const TOKEN_ELEVATION_CLASS: u32 = 20;
/// `SystemFileCacheInformation`
const SYSTEM_FILE_CACHE_INFORMATION: i32 = 81;
/// `SystemMemoryListInformation`
const SYSTEM_MEMORY_LIST_INFORMATION: i32 = 80;
/// 依次：清空工作集、刷新已修改页、清空待机列表、再清一次工作集。
const MEMORY_LIST_COMMANDS: [u32; 4] = [2, 3, 4, 5];

#[repr(C)]
struct TokenElevation {
    token_is_elevated: u32,
}

#[repr(C)]
struct LuidAndAttributes {
    luid: i64,
    attributes: u32,
}

#[repr(C)]
struct TokenPrivileges {
    privilege_count: u32,
    privileges: LuidAndAttributes,
}

/// 与 `SYSTEM_FILECACHE_INFORMATION` 同布局；只写前两个字段（其余是内核返回的输出）。
#[repr(C)]
#[derive(Default)]
struct SystemFileCacheInformation {
    minimum_working_set: usize,
    maximum_working_set: usize,
    current_size: usize,
    peak_size: usize,
    page_fault_count: usize,
    hit_count: usize,
    miss_count: usize,
}

#[link(name = "ntdll")]
extern "system" {
    fn NtSetSystemInformation(
        information_class: i32,
        information: *mut core::ffi::c_void,
        length: u32,
    ) -> NTSTATUS;
}

#[link(name = "advapi32")]
extern "system" {
    fn OpenProcessToken(process: HANDLE, access: u32, token: *mut HANDLE) -> BOOL;
    fn LookupPrivilegeValueW(system: *const u16, name: *const u16, luid: *mut i64) -> BOOL;
    fn AdjustTokenPrivileges(
        token: HANDLE,
        disable_all: BOOL,
        new_state: *const TokenPrivileges,
        buffer_length: u32,
        previous_state: *mut TokenPrivileges,
        return_length: *mut u32,
    ) -> BOOL;
    fn GetTokenInformation(
        token: HANDLE,
        class: u32,
        information: *mut core::ffi::c_void,
        length: u32,
        return_length: *mut u32,
    ) -> BOOL;
}

#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteExW(info: *mut ShellExecuteInfo) -> BOOL;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> HANDLE;
    fn CloseHandle(handle: HANDLE) -> BOOL;
    fn GetLastError() -> u32;
    fn WaitForSingleObject(handle: HANDLE, milliseconds: u32) -> u32;
    fn GetExitCodeProcess(process: HANDLE, code: *mut u32) -> BOOL;
}

#[repr(C)]
struct ShellExecuteInfo {
    cb_size: u32,
    f_mask: u32,
    hwnd: HANDLE,
    lp_verb: *const u16,
    lp_file: *const u16,
    lp_parameters: *const u16,
    lp_directory: *const u16,
    n_show: i32,
    h_inst_app: HANDLE,
    lp_id_list: *mut core::ffi::c_void,
    lp_class: *const u16,
    h_key_class: HANDLE,
    dw_hot_key: u32,
    h_icon: HANDLE,
    h_process: HANDLE,
}

const SEE_MASK_NOCLOSEPROCESS: u32 = 0x0000_0040;
const SEE_MASK_NOASYNC: u32 = 0x0000_0100;
const SW_HIDE: i32 = 0;
const ERROR_CANCELLED: u32 = 1223;
const INFINITE: u32 = u32::MAX;

fn wide(name: &str) -> Vec<u16> {
    name.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 当前进程是否已经带着管理员令牌在跑。
pub fn is_elevated() -> bool {
    unsafe {
        let mut token: HANDLE = 0;
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TokenElevation {
            token_is_elevated: 0,
        };
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TOKEN_ELEVATION_CLASS,
            &mut elevation as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<TokenElevation>() as u32,
            &mut returned,
        );
        CloseHandle(token);
        ok != 0 && elevation.token_is_elevated != 0
    }
}

fn enable_privilege(name: &str) -> Result<(), String> {
    let wide_name = wide(name);
    unsafe {
        let mut token: HANDLE = 0;
        if OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        ) == 0
        {
            return Err(format!("打开进程令牌失败（错误码 {}）", GetLastError()));
        }
        let mut luid = 0i64;
        if LookupPrivilegeValueW(ptr::null(), wide_name.as_ptr(), &mut luid) == 0 {
            let code = GetLastError();
            CloseHandle(token);
            return Err(format!("查不到权限 {name}（错误码 {code}）"));
        }
        let privileges = TokenPrivileges {
            privilege_count: 1,
            privileges: LuidAndAttributes {
                luid,
                attributes: SE_PRIVILEGE_ENABLED,
            },
        };
        let ok = AdjustTokenPrivileges(
            token,
            false as BOOL,
            &privileges,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
        );
        // AdjustTokenPrivileges 在「部分成功」时也返回非零，要看错误码 1300。
        let code = GetLastError();
        CloseHandle(token);
        if ok == 0 || code == 1300 {
            return Err(format!("当前账户没有 {name} 权限（错误码 {code}）"));
        }
        Ok(())
    }
}

/// 清空系统文件缓存与待机列表。需要管理员权限，失败时返回原因。
pub fn optimize() -> Result<(), String> {
    enable_privilege("SeProfileSingleProcessPrivilege")?;
    // 这个权限在个别系统上默认不给，失败不致命——清待机列表主要靠上面那条。
    let _ = enable_privilege("SeIncreaseQuotaPrivilege");

    let mut results = Vec::new();
    unsafe {
        let mut cache = SystemFileCacheInformation {
            minimum_working_set: usize::MAX,
            maximum_working_set: usize::MAX,
            ..Default::default()
        };
        results.push(NtSetSystemInformation(
            SYSTEM_FILE_CACHE_INFORMATION,
            &mut cache as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<SystemFileCacheInformation>() as u32,
        ));
        for command in MEMORY_LIST_COMMANDS {
            let mut value = command;
            results.push(NtSetSystemInformation(
                SYSTEM_MEMORY_LIST_INFORMATION,
                &mut value as *mut _ as *mut core::ffi::c_void,
                std::mem::size_of::<u32>() as u32,
            ));
        }
    }
    // 一个都没成功才算失败；部分成功说明内存已经还回去了。
    if results.iter().all(|status| *status < 0) {
        return Err(format!("系统拒绝了内存优化请求（状态 {results:?}）"));
    }
    Ok(())
}

/// 用管理员权限再拉一次自己（`--memory-optimize`），等它跑完。
///
/// 用户点了 UAC 的「否」会返回错误，调用方据此跳过优化。
pub fn optimize_elevated(executable: &std::path::Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    let verb = wide("runas");
    let parameters = wide("--memory-optimize");
    let file: Vec<u16> = executable
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut info = ShellExecuteInfo {
        cb_size: std::mem::size_of::<ShellExecuteInfo>() as u32,
        f_mask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        hwnd: 0,
        lp_verb: verb.as_ptr(),
        lp_file: file.as_ptr(),
        lp_parameters: parameters.as_ptr(),
        lp_directory: ptr::null(),
        n_show: SW_HIDE,
        h_inst_app: 0,
        lp_id_list: ptr::null_mut(),
        lp_class: ptr::null(),
        h_key_class: 0,
        dw_hot_key: 0,
        h_icon: 0,
        h_process: 0,
    };
    unsafe {
        if ShellExecuteExW(&mut info) == 0 {
            let code = GetLastError();
            if code == ERROR_CANCELLED {
                return Err("没有拿到管理员权限（用户取消了授权）".into());
            }
            return Err(format!("拉起管理员进程失败（错误码 {code}）"));
        }
        if info.h_process == 0 {
            return Err("管理员进程没有返回句柄".into());
        }
        WaitForSingleObject(info.h_process, INFINITE);
        let mut code = 0u32;
        let ok = GetExitCodeProcess(info.h_process, &mut code);
        CloseHandle(info.h_process);
        if ok == 0 {
            return Err("读不到管理员进程的退出码".into());
        }
        if code != 0 {
            return Err(format!("管理员进程没能完成内存优化（退出码 {code}）"));
        }
    }
    Ok(())
}
