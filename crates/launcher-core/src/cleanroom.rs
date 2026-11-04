//! Cleanroom 的启动杂项。
//!
//! Cleanroom 首次启动会在渲染线程上同步下载一份 35 MB 的中文字体
//! （`source_han_sans_hw_vf.ttf`），本机实测单连接只有 ~70 KB/s，窗口会「未响应」
//! 十几分钟。它的缓存布局是 `<用户目录>/.cleanroom/cache/kirino/fonts/<名字>`
//! 加上 `.meta`（Java Properties：sha256/version/checked/url/modified）和
//! `.versions/<sha 前 16 位>/<文件名>` 的内容文件；三样齐全就直接命中。
//!
//! 这里在启动前用多连接 Range 把字体补齐（同一台机器上比它的单连接快一个数量级），
//! 于是「第一次启动」也不用等。拿不到网络时不影响启动，游戏会自己想办法。

use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const FONT_URL: &str =
    "https://cdn.cleanroommc.com/kirino/fonts/source_han_sans/source_han_sans_hw_vf.ttf";
const FONT_NAME: &str = "source_han_sans_hw_vf.ttf";
const FONT_SIZE: u64 = 35_445_188;
const PARALLEL: u64 = 16;

/// classpath 里出现 cleanroom 的 universal jar 就当它是 Cleanroom 实例。
pub fn looks_like_cleanroom(args: &[String]) -> bool {
    args.iter().any(|arg| {
        let lower = arg.to_ascii_lowercase();
        lower.contains("cleanroom") && lower.contains(".jar")
    })
}

/// 字体缓存已经就位就什么都不做；补上了就返回一句日志。
pub fn ensure_font_cached() -> std::result::Result<Option<String>, String> {
    let home = match std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
        Ok(home) if !home.trim().is_empty() => PathBuf::from(home),
        _ => return Ok(None),
    };
    let dir = home
        .join(".cleanroom")
        .join("cache")
        .join("kirino")
        .join("fonts")
        .join("source_han_sans");
    let blob = dir.join(FONT_NAME);
    if let Ok(meta) = fs::metadata(&blob) {
        if meta.len() == FONT_SIZE {
            return Ok(None);
        }
    }
    fs::create_dir_all(&dir).map_err(|error| format!("建目录 {}: {error}", dir.display()))?;
    let bytes = match fetch_font() {
        Ok(bytes) => bytes,
        // 拿不到就交给游戏自己下，别把启动卡在这里。
        Err(_) => return Ok(None),
    };
    if bytes.len() as u64 != FONT_SIZE {
        return Ok(None);
    }
    let sha = sha256_hex(&bytes);
    fs::write(&blob, &bytes).map_err(|error| format!("写字体 {}: {error}", blob.display()))?;
    let shard = dir.join(format!("{FONT_NAME}.versions")).join(&sha[..16]);
    fs::create_dir_all(&shard).map_err(|error| format!("建分片目录 {}: {error}", shard.display()))?;
    fs::write(shard.join(FONT_NAME), &bytes).map_err(|error| format!("写分片: {error}"))?;
    let meta = format!(
        "sha256={sha}\nversion={version}\nchecked={checked}\nurl={FONT_URL}\nmodified={modified}\n",
        version = FONT_SIZE,
        checked = unix_now(),
        modified = unix_now(),
    );
    fs::write(dir.join(format!("{FONT_NAME}.meta")), meta).map_err(|error| format!("写 meta: {error}"))?;
    Ok(Some(format!(
        "已预置 Cleanroom 中文字体（{:.1} MB）：{}",
        bytes.len() as f64 / 1_048_576.0,
        blob.display()
    )))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 16 个 Range 请求并发抓同一个文件，再按顺序拼起来。
fn fetch_font() -> std::result::Result<Vec<u8>, String> {
    let chunk = (FONT_SIZE + PARALLEL - 1) / PARALLEL;
    let handles: Vec<_> = (0..PARALLEL)
        .map(|index| {
            let start = index * chunk;
            let end = ((index + 1) * chunk).min(FONT_SIZE) - 1;
            thread::spawn(move || fetch_range(start, end))
        })
        .collect();
    let mut out = Vec::with_capacity(FONT_SIZE as usize);
    for handle in handles {
        let mut part = handle
            .join()
            .map_err(|_| "Cleanroom 字体分片线程崩溃".to_owned())??;
        out.append(&mut part);
    }
    Ok(out)
}

fn fetch_range(start: u64, end: u64) -> std::result::Result<Vec<u8>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(FONT_URL)
        .header("Range", format!("bytes={start}-{end}"))
        .send()
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("Cleanroom 字体下载返回 {}", response.status()));
    }
    let mut bytes = Vec::with_capacity((end - start + 1) as usize);
    response
        .take(end - start + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

/// SHA-256（只为了给 Cleanroom 的缓存元数据填值，不引新依赖）。
fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = data.to_vec();
    let bits = (data.len() as u64) * 8;
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bits.to_be_bytes());
    for block in message.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (index, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([
                block[index * 4],
                block[index * 4 + 1],
                block[index * 4 + 2],
                block[index * 4 + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }
        let mut a = h;
        for index in 0..64 {
            let s1 = a[4].rotate_right(6) ^ a[4].rotate_right(11) ^ a[4].rotate_right(25);
            let ch = (a[4] & a[5]) ^ (!a[4] & a[6]);
            let t1 = a[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let s0 = a[0].rotate_right(2) ^ a[0].rotate_right(13) ^ a[0].rotate_right(22);
            let maj = (a[0] & a[1]) ^ (a[0] & a[2]) ^ (a[1] & a[2]);
            let t2 = s0.wrapping_add(maj);
            a[7] = a[6];
            a[6] = a[5];
            a[5] = a[4];
            a[4] = a[3].wrapping_add(t1);
            a[3] = a[2];
            a[2] = a[1];
            a[1] = a[0];
            a[0] = t1.wrapping_add(t2);
        }
        for index in 0..8 {
            h[index] = h[index].wrapping_add(a[index]);
        }
    }
    h.iter().map(|word| format!("{word:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// 需要联网，日常不跑：`cargo test -p launcher-core seeds_font -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn seeds_font_on_demand() {
        let home = std::env::var("USERPROFILE").expect("USERPROFILE");
        let _ = fs::remove_dir_all(PathBuf::from(&home).join(".cleanroom/cache/kirino"));
        let first = ensure_font_cached().unwrap();
        assert!(first.is_some(), "第一次应该真的下载");
        let dir = PathBuf::from(&home).join(".cleanroom/cache/kirino/fonts/source_han_sans");
        let blob = dir.join(FONT_NAME);
        assert_eq!(fs::metadata(&blob).unwrap().len(), FONT_SIZE);
        let meta = fs::read_to_string(dir.join(format!("{FONT_NAME}.meta"))).unwrap();
        assert!(meta.contains("sha256="), "{meta}");
        assert!(fs::read_dir(dir.join(format!("{FONT_NAME}.versions")))
            .unwrap()
            .count()
            == 1);
        assert!(ensure_font_cached().unwrap().is_none(), "第二次应该直接命中");
    }

    #[test]
    fn cleanroom_jar_is_detected() {
        let args = vec![
            "-cp".to_owned(),
            "C:\\x\\libraries\\com\\cleanroommc\\cleanroom\\0.7.14-alpha\\cleanroom-0.7.14-alpha-universal.jar"
                .to_owned(),
        ];
        assert!(looks_like_cleanroom(&args));
        assert!(!looks_like_cleanroom(&["-cp".to_owned(), "x\\1.20.1.jar".to_owned()]));
    }
}
