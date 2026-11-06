//! 存档目录：列条目、读图标、读游戏版本。
//!
//! PCL 的存档页是列表：图标 + 文件夹名 + 创建/最后修改时间。这里在它基础上
//! 多读一样东西——`level.dat` 里的 `Data.Version.Name`（存档存的时候是什么版本）。
//! `level.dat` 是 gzip 包着的 NBT，只认第一层结构里我们要的那几个键，不写通用解析器。

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::Serialize;

/// 一个存档（`saves/<文件夹>`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveEntry {
    /// 文件夹名，也是 `--quickPlaySingleplayer` 要传的名字。
    pub name: String,
    /// `level.dat` 里的 `Data.LevelName`（存档显示名），读不到就用文件夹名。
    pub level_name: Option<String>,
    /// `Data.Version.Name`，例如 `1.21.1`；读不到就是 `None`。
    pub version: Option<String>,
    pub created_ms: u64,
    pub modified_ms: u64,
    pub size: u64,
    /// `icon.png` 的 data URL（存档预览图）。
    pub icon: Option<String>,
}

fn millis(time: std::io::Result<SystemTime>) -> u64 {
    time.ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0_u64;
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            total += dir_size(&entry.path());
        } else {
            total += meta.len();
        }
    }
    total
}

fn icon_data_url(save: &Path) -> Option<String> {
    let path = save.join("icon.png");
    let bytes = fs::read(&path).ok()?;
    if bytes.is_empty() || bytes.len() > 4 * 1024 * 1024 {
        return None;
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Some(format!("data:image/png;base64,{encoded}"))
}

/// 列 `saves/` 下的存档，按最后修改时间从新到旧。
pub fn list(root: &Path) -> Vec<SaveEntry> {
    let mut items = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return items;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let (version, level_name) = read_level_dat(&path.join("level.dat"));
        let modified = meta.modified().ok();
        items.push(SaveEntry {
            name,
            level_name,
            version,
            created_ms: millis(meta.created()),
            modified_ms: millis(modified.ok_or(std::io::Error::other("no mtime"))),
            size: dir_size(&path),
            icon: icon_data_url(&path),
        });
    }
    items.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));
    items
}

/// 从 `level.dat` 里抠 `Data.Version.Name` 和 `Data.LevelName`。
fn read_level_dat(path: &PathBuf) -> (Option<String>, Option<String>) {
    let Ok(bytes) = fs::read(path) else {
        return (None, None);
    };
    let mut decoder = flate2::read::GzDecoder::new(bytes.as_slice());
    let mut data = Vec::new();
    if decoder.read_to_end(&mut data).is_err() {
        return (None, None);
    }
    let mut cursor = Cursor { data: &data, pos: 0 };
    let mut level_name = None;
    let mut version = None;
    // 根节点：TAG_Compound(10) + 名字 + 载荷。
    if cursor.u8() != Some(10) {
        return (None, None);
    }
    cursor.skip_string();
    while let Some(kind) = cursor.u8() {
        if kind == 0 {
            break;
        }
        let Some(key) = cursor.string() else {
            break;
        };
        if kind == 10 && key == "Data" {
            // 进 Data 复合体，找 Version（复合体）与 LevelName（字符串）。
            while let Some(inner) = cursor.u8() {
                if inner == 0 {
                    break;
                }
                let Some(name) = cursor.string() else {
                    break;
                };
                match (inner, name.as_str()) {
                    (8, "LevelName") => level_name = cursor.string(),
                    (10, "Version") => {
                        while let Some(field) = cursor.u8() {
                            if field == 0 {
                                break;
                            }
                            let Some(field_name) = cursor.string() else {
                                break;
                            };
                            if field == 8 && field_name == "Name" {
                                version = cursor.string();
                            } else {
                                cursor.skip_payload(field);
                            }
                        }
                    }
                    _ => cursor.skip_payload(inner),
                }
            }
        } else {
            cursor.skip_payload(kind);
        }
    }
    (version, level_name)
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn take(&mut self, count: usize) -> Option<&[u8]> {
        let end = self.pos.checked_add(count)?;
        let slice = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|slice| slice[0])
    }

    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|slice| u16::from_be_bytes([slice[0], slice[1]]))
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|slice| u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
    }

    fn string(&mut self) -> Option<String> {
        let len = self.u16()? as usize;
        let bytes = self.take(len)?;
        Some(String::from_utf8_lossy(bytes).into_owned())
    }

    fn skip_string(&mut self) {
        let _ = self.string();
    }

    /// 跳过某个 tag 的载荷（按类型长度，够了就不再往下走）。
    fn skip_payload(&mut self, kind: u8) {
        match kind {
            1 => {
                let _ = self.take(1);
            }
            2 => {
                let _ = self.take(2);
            }
            3 | 5 => {
                let _ = self.take(4);
            }
            4 | 6 => {
                let _ = self.take(8);
            }
            7 => {
                let len = self.u32().unwrap_or(0) as usize;
                let _ = self.take(len);
            }
            8 => {
                self.skip_string();
            }
            9 => {
                let item = self.u8().unwrap_or(0);
                let len = self.u32().unwrap_or(0) as usize;
                for _ in 0..len {
                    self.skip_payload(item);
                }
            }
            10 => {
                while let Some(inner) = self.u8() {
                    if inner == 0 {
                        break;
                    }
                    self.skip_string();
                    self.skip_payload(inner);
                }
            }
            11 => {
                let len = self.u32().unwrap_or(0) as usize;
                let _ = self.take(len * 4);
            }
            12 => {
                let len = self.u32().unwrap_or(0) as usize;
                let _ = self.take(len * 8);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手搓一个最小的 level.dat：Data { LevelName, Version { Name } }。
    fn sample() -> Vec<u8> {
        let mut out = Vec::new();
        out.push(10); // root compound
        out.extend_from_slice(&0_u16.to_be_bytes()); // 空名字
        out.push(10); // Data
        out.extend_from_slice(&(4_u16).to_be_bytes());
        out.extend_from_slice(b"Data");
        out.push(8); // LevelName: string
        out.extend_from_slice(&(9_u16).to_be_bytes());
        out.extend_from_slice(b"LevelName");
        out.extend_from_slice(&(4_u16).to_be_bytes());
        out.extend_from_slice(b"test");
        out.push(10); // Version
        out.extend_from_slice(&(7_u16).to_be_bytes());
        out.extend_from_slice(b"Version");
        out.push(8); // Name
        out.extend_from_slice(&(4_u16).to_be_bytes());
        out.extend_from_slice(b"Name");
        out.extend_from_slice(&(6_u16).to_be_bytes());
        out.extend_from_slice(b"1.21.1");
        out.push(0); // end Version
        out.push(0); // end Data
        out.push(0); // end root
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        use std::io::Write as _;
        gz.write_all(&out).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn reads_version_and_level_name() {
        let dir = std::env::temp_dir().join(format!("ybumcl-lvl-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("level.dat");
        fs::write(&path, sample()).unwrap();
        let (version, name) = read_level_dat(&path);
        assert_eq!(version.as_deref(), Some("1.21.1"));
        assert_eq!(name.as_deref(), Some("test"));
        let _ = fs::remove_dir_all(&dir);
    }
}
