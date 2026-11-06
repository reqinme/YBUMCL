//! 第三方认证服务器（authlib-injector）：服务器元数据、注入参数、yggdrasil 登录。
//!
//! 走法和 SJMCL / HMCL 一样：先向服务器根地址要一份元数据（`meta` + `skinDomains`，
//! `meta.signaturePublickey` 可选），登录拿 yggdrasil 的 accessToken，
//! 启动时把 `-javaagent:authlib-injector.jar=<服务器地址>` 加上，再把元数据用
//! base64 塞进 `-Dauthlibinjector.yggdrasil.prefetched`，游戏端就不会再去拉一遍。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::auth;

/// authlib-injector 的文件名（放在启动器数据目录下，和 SJMCL 一致）。
pub const JAR_NAME: &str = "authlib-injector.jar";
/// 官方版本清单：`{ version, download_url }`。
const LATEST: &str = "https://authlib-injector.yushi.moe/artifact/latest.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Server {
    /// 接口地址（不是用户填的原始地址：已经按 `X-Authlib-Injector-API-Location` 归一过）。
    pub url: String,
    /// 服务器名字，取元数据里的 `meta.serverName`，没有就用域名。
    pub name: String,
    /// OIDC 的 client id（服务器支持设备码登录时才用得上，暂时只存不用）。
    pub client_id: Option<String>,
    /// 元数据原文（注入时 base64 一份给游戏）。
    pub metadata: Option<String>,
    pub timestamp: u64,
}

/// 一次 yggdrasil 登录拿到的一个角色。
#[derive(Debug, Clone)]
pub struct Profile {
    pub name: String,
    pub uuid: String,
    pub token: String,
    pub avatar: Option<String>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn http() -> &'static reqwest::blocking::Client {
    static CLIENT: std::sync::OnceLock<reqwest::blocking::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("YBUMCL/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new())
    })
}

/// 用户填的地址可能没有协议、带尾部斜杠，统一成 `https://host/path`。
pub fn normalize_url(input: &str) -> Result<String, String> {
    let text = input.trim();
    if text.is_empty() {
        return Err("认证服务器地址是空的。".into());
    }
    let with_scheme = if text.starts_with("http://") || text.starts_with("https://") {
        text.to_owned()
    } else {
        format!("https://{text}")
    };
    let url = reqwest::Url::parse(&with_scheme)
        .map_err(|error| format!("这不是一个网址：{error}"))?;
    if url.host_str().is_none() {
        return Err("这个网址没有主机名。".into());
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// 读服务器的元数据。按 authlib-injector 的约定，根地址可能用
/// `X-Authlib-Injector-API-Location` 把头指到真正的接口地址。
pub fn fetch_server(input: &str) -> Result<Server, String> {
    let root = normalize_url(input)?;
    let response = http()
        .get(&root)
        .send()
        .map_err(|error| format!("连不上这个认证服务器：{error}"))?;
    if !response.status().is_success() {
        return Err(format!("认证服务器返回 HTTP {}。", response.status().as_u16()));
    }
    let location = response
        .headers()
        .get("X-Authlib-Injector-API-Location")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let base = match location {
        Some(location) => {
            let joined = reqwest::Url::parse(&root)
                .and_then(|root| root.join(&location))
                .map_err(|error| format!("服务器给的接口地址不对：{error}"))?;
            joined.as_str().trim_end_matches('/').to_owned()
        }
        None => root.clone(),
    };
    // 元数据可能是根地址自己，也可能是指过去的接口地址，两边都试一次。
    let metadata = read_metadata(&base)
        .or_else(|| {
            if base == root {
                None
            } else {
                read_metadata(&root)
            }
        })
        .ok_or_else(|| "这个服务器没有返回 authlib-injector 元数据，可能不是认证服务器。".to_string())?;
    let name = metadata
        .get("meta")
        .and_then(|meta| meta.get("serverName"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            reqwest::Url::parse(&base)
                .ok()
                .and_then(|url| url.host_str().map(str::to_owned))
        })
        .unwrap_or_else(|| base.clone());
    Ok(Server {
        url: base,
        name,
        client_id: None,
        metadata: Some(metadata.to_string()),
        timestamp: now(),
    })
}

fn read_metadata(url: &str) -> Option<serde_json::Value> {
    let response = http().get(url).send().ok()?;
    if !response.status().is_success() {
        return None;
    }
    let value: serde_json::Value = response.json().ok()?;
    // 元数据长这样：{ "meta": { "serverName": … }, "skinDomains": […] }。
    // `signaturePublickey` 是可选的（LittleSkin 就没给），不能拿它当判据。
    let looks_like = value.get("meta").is_some() || value.get("skinDomains").is_some();
    looks_like.then_some(value)
}

/// 启动参数：agent + 预取元数据。游戏端拿到这几行就不再访问网络。
pub fn injection_args(server: &Server, jar: &Path) -> Result<Vec<String>, String> {
    let metadata = server
        .metadata
        .as_deref()
        .ok_or_else(|| "这个认证服务器还没有元数据，先重新添加一次。".to_string())?;
    let prefetched = base64::engine::general_purpose::STANDARD.encode(metadata.as_bytes());
    Ok(vec![
        format!("-javaagent:{}={}", jar.display(), server.url),
        "-Dauthlibinjector.side=client".into(),
        format!("-Dauthlibinjector.yggdrasil.prefetched={prefetched}"),
    ])
}

pub fn jar_path(dir: &Path) -> PathBuf {
    dir.join(JAR_NAME)
}

/// 读 jar 里 `META-INF/MANIFEST.MF` 的版本；文件不在或读不动就是 None。
pub fn jar_version(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut entry = archive.by_name("META-INF/MANIFEST.MF").ok()?;
    let mut text = String::new();
    std::io::Read::read_to_string(&mut entry, &mut text).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("Implementation-Version:"))
        .map(|value| value.trim().to_owned())
}

/// 保证数据目录里有一份能用的 authlib-injector.jar：有就复用，没有就下最新的。
pub fn ensure_jar(dir: &Path) -> Result<PathBuf, String> {
    let path = jar_path(dir);
    if jar_version(&path).is_some() {
        return Ok(path);
    }
    let meta: serde_json::Value = http()
        .get(LATEST)
        .send()
        .and_then(|response| response.json())
        .map_err(|error| format!("拿不到 authlib-injector 的版本信息：{error}"))?;
    let url = meta
        .get("download_url")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "authlib-injector 的版本信息里没有下载地址。".to_string())?;
    let bytes = http()
        .get(url)
        .send()
        .and_then(|response| response.bytes())
        .map_err(|error| format!("下载 authlib-injector 失败：{error}"))?;
    fs::create_dir_all(dir).map_err(|error| format!("建不了数据目录：{error}"))?;
    let temp = path.with_extension("jar.tmp");
    fs::write(&temp, &bytes).map_err(|error| format!("写不进 authlib-injector.jar：{error}"))?;
    fs::rename(&temp, &path).map_err(|error| format!("authlib-injector.jar 换名失败：{error}"))?;
    if jar_version(&path).is_none() {
        return Err("下载回来的 authlib-injector.jar 打不开，可能下坏了。".into());
    }
    Ok(path)
}

/// yggdrasil 密码登录；返回这个账号下的所有角色（有些服务器是「账号 → 多个角色」）。
pub fn login(server: &Server, username: &str, password: &str) -> Result<Vec<Profile>, String> {
    let body = serde_json::json!({
        "username": username,
        "password": password,
        "agent": { "name": "Minecraft", "version": 1 },
        "requestUser": false,
    });
    let (status, text) = post(&format!("{}/authserver/authenticate", server.url), &body)?;
    if status == 403 || status == 401 {
        return Err("账号或密码不对。".into());
    }
    if status != 200 {
        return Err(format!("登录失败（HTTP {status}）：{}", yggdrasil_error(&text)));
    }
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| format!("登录返回看不懂：{error}"))?;
    let token = value
        .get("accessToken")
        .and_then(|item| item.as_str())
        .ok_or_else(|| "登录返回里没有 accessToken。".to_string())?
        .to_owned();
    let mut ids: Vec<(String, String)> = Vec::new();
    if let Some(id) = value
        .get("selectedProfile")
        .and_then(|profile| profile.get("id"))
        .and_then(|item| item.as_str())
    {
        let name = value["selectedProfile"]["name"].as_str().unwrap_or_default();
        ids.push((id.to_owned(), name.to_owned()));
    } else if let Some(profiles) = value.get("availableProfiles").and_then(|item| item.as_array()) {
        for profile in profiles {
            if let (Some(id), Some(name)) = (
                profile.get("id").and_then(|item| item.as_str()),
                profile.get("name").and_then(|item| item.as_str()),
            ) {
                ids.push((id.to_owned(), name.to_owned()));
            }
        }
    }
    if ids.is_empty() {
        return Err("这个账号在服务器上还没有角色。".into());
    }
    let mut out = Vec::new();
    for (id, name) in ids {
        let (uuid, display, avatar) = profile(server, &id).unwrap_or((id.clone(), name.clone(), None));
        out.push(Profile {
            name: display,
            uuid,
            token: token.clone(),
            avatar,
        });
    }
    Ok(out)
}

/// `{server}/sessionserver/session/minecraft/profile/<uuid>`：名字和皮肤。
fn profile(server: &Server, id: &str) -> Option<(String, String, Option<String>)> {
    let url = format!("{}/sessionserver/session/minecraft/profile/{}", server.url, id);
    let value: serde_json::Value = http().get(url).send().ok()?.json().ok()?;
    let uuid = value
        .get("id")
        .and_then(|item| item.as_str())
        .unwrap_or(id)
        .to_owned();
    let name = value
        .get("name")
        .and_then(|item| item.as_str())
        .unwrap_or_default()
        .to_owned();
    let avatar = value
        .get("properties")
        .and_then(|item| item.as_array())
        .and_then(|properties| {
            properties
                .iter()
                .find(|property| property.get("name").and_then(|n| n.as_str()) == Some("textures"))
        })
        .and_then(|property| property.get("value").and_then(|item| item.as_str()))
        .and_then(texture_url)
        .and_then(|url| auth::head_avatar(&url).ok().flatten());
    Some((uuid, name, avatar))
}

/// textures 属性：base64 的 `{ "textures": { "SKIN": { "url": … } } }`。
fn texture_url(encoded: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value
        .get("textures")?
        .get("SKIN")?
        .get("url")?
        .as_str()
        .map(str::to_owned)
}

/// yggdrasil 的刷新：accessToken 过期时换一个新的。
pub fn refresh(server: &Server, token: &str) -> Result<String, String> {
    let body = serde_json::json!({ "accessToken": token });
    let (status, text) = post(&format!("{}/authserver/refresh", server.url), &body)?;
    if status != 200 {
        return Err(format!("刷新登录失败（HTTP {status}）：{}", yggdrasil_error(&text)));
    }
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| format!("刷新返回看不懂：{error}"))?;
    value
        .get("accessToken")
        .and_then(|item| item.as_str())
        .map(str::to_owned)
        .ok_or_else(|| "刷新返回里没有 accessToken。".to_string())
}

fn post(url: &str, body: &serde_json::Value) -> Result<(u16, String), String> {
    let response = http()
        .post(url)
        .json(body)
        .send()
        .map_err(|error| format!("连不上认证服务器：{error}"))?;
    let status = response.status().as_u16();
    let text = response.text().unwrap_or_default();
    Ok((status, text))
}

fn yggdrasil_error(text: &str) -> String {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|value| {
            value
                .get("errorMessage")
                .and_then(|item| item.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| text.chars().take(160).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_normalized() {
        assert_eq!(normalize_url(" example.com/api/ ").unwrap(), "https://example.com/api");
        assert_eq!(
            normalize_url("http://127.0.0.1:8080/").unwrap(),
            "http://127.0.0.1:8080"
        );
        assert!(normalize_url("").is_err());
    }

    #[test]
    fn textures_value_yields_skin_url() {
        let inner = r#"{"textures":{"SKIN":{"url":"https://example.com/skin.png"}}}"#;
        let encoded = base64::engine::general_purpose::STANDARD.encode(inner);
        assert_eq!(
            texture_url(&encoded).as_deref(),
            Some("https://example.com/skin.png")
        );
        assert!(texture_url("not-base64").is_none());
    }

    #[test]
    fn injection_uses_server_url_and_prefetched_metadata() {
        let server = Server {
            url: "https://example.com/api".into(),
            name: "Example".into(),
            client_id: None,
            metadata: Some(r#"{"meta":{"serverName":"Example"}}"#.into()),
            timestamp: 0,
        };
        let args = injection_args(&server, Path::new("C:/x/authlib-injector.jar")).unwrap();
        assert_eq!(args[0], "-javaagent:C:/x/authlib-injector.jar=https://example.com/api");
        assert_eq!(args[1], "-Dauthlibinjector.side=client");
        assert!(args[2].starts_with("-Dauthlibinjector.yggdrasil.prefetched="));
    }

    #[test]
    fn injection_without_metadata_fails() {
        let server = Server {
            url: "https://example.com/api".into(),
            ..Default::default()
        };
        assert!(injection_args(&server, Path::new("x.jar")).is_err());
    }
}
