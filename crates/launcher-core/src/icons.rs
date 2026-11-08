//! 图标可用性判断。
//!
//! CurseForge 上有不少老 mod（Discontinued / Deprecated / Archived）的 logo 是一张
//! 纯白或纯透明的图，直接显示就是一块白方块。这里把字节解出来量一下亮度跨度，
//! 单色图当作「没有图标」，界面就会退回首字母占位。

/// 亮度跨度低于这个值就当整张图一个颜色。
const FLAT_SPAN: u32 = 16;

/// 返回 (mime, 是否空白图)。
pub fn inspect(bytes: &[u8]) -> Option<(&'static str, bool)> {
    let decoded = image::load_from_memory(bytes).ok()?;
    let (mime, format) = match image::guess_format(bytes).ok()? {
        image::ImageFormat::Jpeg => ("jpeg", "jpeg"),
        image::ImageFormat::Png => ("png", "png"),
        image::ImageFormat::WebP => ("webp", "webp"),
        image::ImageFormat::Gif => ("gif", "gif"),
        other => {
            let _ = other;
            ("png", "png")
        }
    };
    let _ = format;
    let small = decoded.thumbnail(32, 32).to_rgba8();
    let mut min = 255_u32;
    let mut max = 0_u32;
    let mut opaque = 0_u32;
    for pixel in small.pixels() {
        let [r, g, b, a] = pixel.0;
        if a < 24 {
            continue;
        }
        opaque += 1;
        let value = (r as u32 + g as u32 + b as u32) / 3;
        min = min.min(value);
        max = max.max(value);
    }
    if opaque == 0 {
        return Some((mime, true));
    }
    Some((mime, max - min < FLAT_SPAN))
}

/// 网络图标在磁盘缓存里的文件名：URL 的 SHA1。
///
/// 直接拿 URL 当文件名会撞上长度限制和 `?`、`:` 这些非法字符；同一个 URL
/// 永远得到同一个名字，换图（CDN 路径里带指纹）自然就是新文件。
pub fn url_key(url: &str) -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(url.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 简单的 base64，避免为了一个图标再加依赖。
pub fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}
