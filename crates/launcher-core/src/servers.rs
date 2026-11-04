//! 读 `servers.dat`：Minecraft 用 NBT 记服务器列表，这里只挑出「名字 + 地址」。

use std::io::Read;
use std::path::Path;

use serde::Serialize;

use crate::{io_error, CoreError, Result};

/// 一条服务器记录。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerEntry {
    pub name: String,
    pub ip: String,
}

struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(len).ok_or_else(bad)?;
        if end > self.data.len() {
            return Err(bad());
        }
        let slice = &self.data[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn string(&mut self) -> Result<String> {
        let len = self.u16()? as usize;
        let bytes = self.take(len)?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }

    /// 跳过一个 tag 的负载（类型编号见 NBT 规范）。
    fn skip(&mut self, tag: u8) -> Result<()> {
        match tag {
            1 => {
                self.take(1)?;
            }
            2 => {
                self.take(2)?;
            }
            3 => {
                self.take(4)?;
            }
            4 => {
                self.take(8)?;
            }
            5 => {
                self.take(4)?;
            }
            6 => {
                self.take(8)?;
            }
            7 => {
                let len = self.u32()? as usize;
                self.take(len)?;
            }
            8 => {
                self.string()?;
            }
            9 => {
                let inner = self.u8()?;
                let len = self.u32()? as usize;
                for _ in 0..len {
                    self.skip(inner)?;
                }
            }
            10 => loop {
                let inner = self.u8()?;
                if inner == 0 {
                    break;
                }
                self.string()?;
                self.skip(inner)?;
            },
            11 => {
                let len = self.u32()? as usize;
                self.take(len.saturating_mul(4))?;
            }
            12 => {
                let len = self.u32()? as usize;
                self.take(len.saturating_mul(8))?;
            }
            other => {
                return Err(CoreError::InvalidInput(format!(
                    "servers.dat 里有不认识的标签类型：{other}"
                )));
            }
        }
        Ok(())
    }
}

fn bad() -> CoreError {
    CoreError::InvalidInput("servers.dat 内容不完整".into())
}

/// 读服务器列表；文件不存在当作空列表，坏文件返回错误让界面能说明。
pub fn read_servers(path: &Path) -> Result<Vec<ServerEntry>> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let mut raw = Vec::new();
    std::fs::File::open(path)
        .map_err(|error| io_error(path, error))?
        .read_to_end(&mut raw)
        .map_err(|error| io_error(path, error))?;
    // 原版写的是未压缩 NBT，但别的启动器可能存成 gzip。
    let data = if raw.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(&raw[..])
            .read_to_end(&mut out)
            .map_err(|error| io_error(path, error))?;
        out
    } else {
        raw
    };

    let mut cursor = Cursor::new(&data);
    let root_tag = cursor.u8()?;
    if root_tag != 10 {
        return Err(CoreError::InvalidInput(
            "servers.dat 的根标签不是 Compound".into(),
        ));
    }
    let _root_name = cursor.string()?;
    let mut servers = Vec::new();
    loop {
        let tag = cursor.u8()?;
        if tag == 0 {
            break;
        }
        let name = cursor.string()?;
        if tag == 9 && name == "servers" {
            let inner = cursor.u8()?;
            let len = cursor.u32()? as usize;
            for _ in 0..len {
                if inner != 10 {
                    cursor.skip(inner)?;
                    continue;
                }
                let mut entry = ServerEntry {
                    name: String::new(),
                    ip: String::new(),
                };
                loop {
                    let field = cursor.u8()?;
                    if field == 0 {
                        break;
                    }
                    let key = cursor.string()?;
                    if field == 8 {
                        let value = cursor.string()?;
                        match key.as_str() {
                            "name" => entry.name = value,
                            "ip" => entry.ip = value,
                            _ => {}
                        }
                    } else {
                        cursor.skip(field)?;
                    }
                }
                if !entry.ip.is_empty() {
                    if entry.name.is_empty() {
                        entry.name = entry.ip.clone();
                    }
                    servers.push(entry);
                }
            }
        } else {
            cursor.skip(tag)?;
        }
    }
    Ok(servers)
}
