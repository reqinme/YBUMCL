//! 世界整包的落地：把下载下来的 zip 解压到实例的 `saves`。
//!
//! CurseForge 的世界包基本只有两种结构：
//! - `<世界名>/level.dat`：最正常的一种，解压后 `saves/<世界名>` 就是存档；
//! - `level.dat` 直接躺在根上：只能铺进 `saves`，和 PCL 的 `ExtractFile` 行为一致。
//!
//! 解压前会把每个条目的路径过一遍 [`safe_relative`]，挡住 `..` 与绝对路径（zip slip）。

use std::{
    fs,
    io,
    path::{Component, Path, PathBuf},
};

use crate::{CoreError, Result};

/// 把世界压缩包解压到 `saves`，返回落在 `saves` 下的顶层名字（铺平的情况返回 "saves"）。
pub fn extract_world(archive: &Path, saves: &Path) -> Result<String> {
    let file = fs::File::open(archive).map_err(|error| io_error(archive, error))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|error| CoreError::Archive(error.to_string()))?;
    fs::create_dir_all(saves).map_err(|error| io_error(saves, error))?;

    // 顶层目录名；一旦出现第二种类别或根上的文件，就当成「铺平」处理
    let mut top: Option<String> = None;
    let mut flat = false;
    let mut written = 0usize;

    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| CoreError::Archive(error.to_string()))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_owned();
        // 有些包带 macOS 的元数据目录，跳过省得在存档里出现杂项
        if name.starts_with("__MACOSX/") || name.ends_with(".DS_Store") {
            continue;
        }
        let Some(relative) = safe_relative(&name) else {
            continue;
        };
        let parts: Vec<String> = relative
            .components()
            .filter_map(|part| match part {
                Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        if parts.len() == 1 {
            flat = true;
        } else if let Some(first) = parts.first() {
            match &top {
                None => top = Some(first.clone()),
                Some(existing) if existing != first => flat = true,
                _ => {}
            }
        }

        let target = saves.join(&relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
        }
        let mut out = fs::File::create(&target).map_err(|error| io_error(&target, error))?;
        io::copy(&mut entry, &mut out).map_err(|error| io_error(&target, error))?;
        written += 1;
    }

    if written == 0 {
        return Err(CoreError::Archive("压缩包里没有可解压的文件".into()));
    }
    Ok(match (flat, top) {
        (false, Some(name)) => name,
        _ => "saves".into(),
    })
}

/// 只接受普通相对路径，挡住 `..` 与绝对路径（zip slip）。
fn safe_relative(path: &str) -> Option<PathBuf> {
    let path = Path::new(path);
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

fn io_error(path: &Path, error: io::Error) -> CoreError {
    CoreError::Io {
        path: path.display().to_string(),
        source: error,
    }
}
