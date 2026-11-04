//! 便携化路径解析：启动器所有可变状态都放在 exe 同级目录里。
//!
//! 查找顺序：
//! 1. 命令行 `--data-dir <path>` / `--data-dir=<path>`
//! 2. exe 同级 `ybumcl.cfg`（一行，写文件夹名或绝对路径）
//! 3. exe 同级唯一一个含 `config.json` 的文件夹（用户手动改名后自动接管）
//! 4. 默认 `exe同级/YBUMCL`
//!
//! 目标位置不可写（只读介质、Program Files 等）时，回退到
//! `%LOCALAPPDATA%\YBUMCL` 并把 `portable` 标为 false，由界面提示用户。

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "YBUMCL";
pub const WINDOW_TITLE: &str = "延边大学我的世界启动器";
pub const POINTER_FILE: &str = "ybumcl.cfg";
pub const GAME_DIR_NAME: &str = ".minecraft";

#[derive(Debug, Clone)]
pub struct Paths {
    exe_dir: PathBuf,
    data_dir: PathBuf,
    portable: bool,
}

impl Paths {
    /// 解析并准备好数据目录，同时把结果写回 `ybumcl.cfg`。
    pub fn resolve() -> io::Result<Self> {
        let exe = std::env::current_exe()?;
        let exe_dir = exe
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "无法定位 exe 所在目录"))?
            .to_path_buf();

        // cfg 指向的目录若已被用户改名（不存在），先尝试自动接管同级那个
        // 唯一带 config.json 的文件夹，而不是傻乎乎再建一个空 YBUMCL。
        let candidate = match command_line_data_dir(&exe_dir) {
            Some(explicit) => Some(explicit),
            None => match pointer_data_dir(&exe_dir) {
                Some(pointing) if pointing.is_dir() => Some(pointing),
                _ => adopted_data_dir(&exe_dir),
            },
        }
        .unwrap_or_else(|| exe_dir.join(APP_NAME));

        match Self::prepare(candidate, exe_dir.clone(), true) {
            Ok(paths) => {
                let _ = paths.write_pointer();
                Ok(paths)
            }
            Err(portable_err) => {
                let fallback = local_app_data_dir().join(APP_NAME);
                Self::prepare(fallback, exe_dir, false).map_err(|_| portable_err)
            }
        }
    }

    fn prepare(data_dir: PathBuf, exe_dir: PathBuf, portable: bool) -> io::Result<Self> {
        for sub in ["", "logs", "webview2", "cache", "wallpapers"] {
            let dir = if sub.is_empty() {
                data_dir.clone()
            } else {
                data_dir.join(sub)
            };
            fs::create_dir_all(&dir)?;
        }

        // 真正写一次文件才算可写：目录权限够但介质只读时 create_dir_all 也会成功。
        let probe = data_dir.join(".write-probe");
        fs::write(&probe, b"ok")?;
        let _ = fs::remove_file(&probe);

        let paths = Self {
            exe_dir,
            data_dir,
            portable,
        };
        paths.ensure_config()?;
        Ok(paths)
    }

    fn ensure_config(&self) -> io::Result<()> {
        let config = self.data_dir.join("config.json");
        if config.exists() {
            return Ok(());
        }
        let body = format!(
            "{{\n  \"schemaVersion\": 1,\n  \"createdBy\": \"{}\",\n  \"portable\": {}\n}}\n",
            APP_NAME, self.portable
        );
        write_atomic(&config, body.as_bytes())
    }

    fn write_pointer(&self) -> io::Result<()> {
        if !self.portable {
            return Ok(());
        }
        // 数据目录就在 exe 同级时只写文件夹名，整个目录被搬走也不会失效。
        let value = match self.data_dir.parent() {
            Some(parent) if parent == self.exe_dir => self
                .data_dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.data_dir.to_string_lossy().into_owned()),
            _ => self.data_dir.to_string_lossy().into_owned(),
        };
        write_atomic(
            &self.exe_dir.join(POINTER_FILE),
            format!("{value}\n").as_bytes(),
        )
    }

    /// 写一行启动日志；调用方保证 logs 目录已建好。
    pub fn write_startup_log(&self, version: &str) {
        let line = format!(
            "[{}] {} v{} 启动 portable={} data={}
",
            local_hhmm(),
            APP_NAME,
            version,
            self.portable,
            self.data_dir.display()
        );
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log_dir().join("launcher.log"))
        {
            let _ = file.write_all(line.as_bytes());
        }
    }

    pub fn exe_dir(&self) -> &Path {
        &self.exe_dir
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn portable(&self) -> bool {
        self.portable
    }

    pub fn webview_dir(&self) -> PathBuf {
        self.data_dir.join("webview2")
    }

    pub fn log_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    #[allow(dead_code)] // M2 起由前端/资源模块使用
    pub fn cache_dir(&self) -> PathBuf {
        self.data_dir.join("cache")
    }

    #[allow(dead_code)] // M2 起用于自定义背景
    pub fn wallpapers_dir(&self) -> PathBuf {
        self.data_dir.join("wallpapers")
    }

    /// 游戏目录（exe 同级 `.minecraft`）。**只在真正需要下载时才创建。**
    pub fn game_dir(&self) -> PathBuf {
        self.exe_dir.join(GAME_DIR_NAME)
    }
}

fn command_line_data_dir(exe_dir: &Path) -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix("--data-dir=") {
            return Some(absolutize(exe_dir, value));
        }
        if arg == "--data-dir" {
            if let Some(value) = args.next() {
                return Some(absolutize(exe_dir, value));
            }
        }
    }
    None
}

fn pointer_data_dir(exe_dir: &Path) -> Option<PathBuf> {
    let raw = fs::read_to_string(exe_dir.join(POINTER_FILE)).ok()?;
    let value = raw.lines().map(str::trim).find(|line| !line.is_empty())?;
    Some(absolutize(exe_dir, value))
}

fn adopted_data_dir(exe_dir: &Path) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(exe_dir).ok()?.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join("config.json").is_file() {
            found.push(path);
        }
    }
    (found.len() == 1).then(|| found.remove(0))
}

fn absolutize(base: &Path, value: impl AsRef<Path>) -> PathBuf {
    let value = value.as_ref();
    if value.is_absolute() {
        value.to_path_buf()
    } else {
        base.join(value)
    }
}

/// 回退根目录（调用方再 join(`APP_NAME`)）。
fn local_app_data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(dir);
    }
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        return PathBuf::from(profile).join(".local");
    }
    std::env::temp_dir()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path).or_else(|_| {
        let _ = fs::remove_file(&tmp);
        fs::write(path, bytes)
    })
}

/// 本地时间 `YYYY-MM-DD HH:MM:SS`，给错误报告用。
pub fn local_stamp() -> String {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        let mut value = unsafe { std::mem::zeroed::<windows_sys::Win32::Foundation::SYSTEMTIME>() };
        unsafe { GetLocalTime(&mut value) };
        return format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            value.wYear, value.wMonth, value.wDay, value.wHour, value.wMinute, value.wSecond
        );
    }
    #[cfg(not(windows))]
    {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_secs() as i64)
            .unwrap_or(0);
        let days = seconds.div_euclid(86_400);
        let rest = seconds.rem_euclid(86_400);
        // 1970-01-01 起按民用历换算，只用来填报告里的时间。
        let (year, month, day) = civil_from_days(days);
        format!(
            "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
            rest / 3600,
            (rest / 60) % 60,
            rest % 60
        )
    }
}

#[cfg(not(windows))]
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 本地时间 `HH:MM`，给日志用（Windows 直接问系统，其它平台退化成 UTC 偏移）。
pub fn local_hhmm() -> String {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        let mut value = unsafe { std::mem::zeroed::<windows_sys::Win32::Foundation::SYSTEMTIME>() };
        unsafe { GetLocalTime(&mut value) };
        return format!("{:02}:{:02}", value.wHour, value.wMinute);
    }
    #[cfg(not(windows))]
    {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_secs())
            .unwrap_or(0);
        let minutes = (seconds / 60) % (24 * 60);
        format!("{:02}:{:02}", minutes / 60, minutes % 60)
    }
}
