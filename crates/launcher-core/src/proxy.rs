//! 下载走不走代理。
//!
//! 三档：`system` 跟随系统设置（Windows 的「Internet 选项」/ 环境变量），
//! `direct` 一律直连，`manual` 用手填的地址。启动器里所有 reqwest 客户端
//! 都在建之前过一遍 [`apply`]，保证元数据和文件下载用同一条链路。

use std::sync::Mutex;

use reqwest::blocking::ClientBuilder;

#[derive(Debug, Clone, Default)]
struct Choice {
    /// `None` = 跟随系统。
    manual: Option<String>,
    direct: bool,
}

static CHOICE: Mutex<Choice> = Mutex::new(Choice {
    manual: None,
    direct: false,
});

/// 按设置里的两个字段配置：`mode` 取 `system` / `direct` / `manual`。
pub fn configure(mode: &str, url: &str) {
    let url = url.trim();
    let mut choice = CHOICE.lock().unwrap_or_else(|poison| poison.into_inner());
    choice.direct = mode == "direct";
    choice.manual = if mode == "manual" && !url.is_empty() {
        Some(url.to_owned())
    } else {
        None
    };
}

/// 给即将创建的客户端套上代理设置。
pub fn apply(builder: ClientBuilder) -> ClientBuilder {
    let choice = CHOICE
        .lock()
        .map(|choice| choice.clone())
        .unwrap_or_default();
    if choice.direct {
        return builder.no_proxy();
    }
    match choice.manual.as_deref() {
        Some(url) => match reqwest::Proxy::all(url) {
            Ok(proxy) => builder.proxy(proxy),
            // 手填的地址解析不了就退回系统设置，总比整条链路报错强。
            Err(_) => builder,
        },
        None => builder,
    }
}
