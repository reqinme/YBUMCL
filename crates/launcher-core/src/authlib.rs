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
    /// 整张皮肤（`data:image/png;base64,…`），账号页画全身预览用。
    pub skin: Option<String>,
    /// 披风，没有就是 None。
    pub cape: Option<String>,
    /// `slim` / `classic`。
    pub skin_model: Option<String>,
    /// 外链登录换来的 OIDC 刷新令牌；密码登录为空。
    pub refresh_token: Option<String>,
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

/// 保证数据目录里有一份能用的 authlib-injector.jar：版本对就复用，旧了才重下。
pub fn ensure_jar(dir: &Path) -> Result<PathBuf, String> {
    let path = jar_path(dir);
    let local = jar_version(&path);
    let (version, url) = match latest_release() {
        Ok(release) => release,
        // 拿不到版本清单时，本地有 jar 就先用着（离线也能启动）。
        Err(error) => {
            return match local {
                Some(_) => Ok(path),
                None => Err(error),
            }
        }
    };
    if local.as_deref() == Some(version.as_str()) {
        return Ok(path);
    }
    let bytes = http()
        .get(&url)
        .send()
        .and_then(|response| response.bytes())
        .map_err(|error| format!("下载 authlib-injector 失败：{error}"))?;
    fs::create_dir_all(dir).map_err(|error| format!("建不了数据目录：{error}"))?;
    let temp = path.with_extension("jar.tmp");
    fs::write(&temp, &bytes).map_err(|error| format!("写不进 authlib-injector.jar：{error}"))?;
    fs::rename(&temp, &path).map_err(|error| format!("authlib-injector.jar 换名失败：{error}"))?;
    if jar_version(&path).as_deref() != Some(version.as_str()) {
        return Err("下载回来的 authlib-injector.jar 版本不对，可能下坏了。".into());
    }
    Ok(path)
}

/// 官方清单拿不到时退回 GitHub 的 release（国内访问 GitHub API 常常比 yushi.moe 稳）。
fn latest_release() -> Result<(String, String), String> {
    let direct = http()
        .get(LATEST)
        .send()
        .and_then(|response| response.json::<serde_json::Value>());
    if let Ok(value) = direct {
        let version = value
            .get("version")
            .and_then(|item| item.as_str())
            .unwrap_or_default()
            .to_owned();
        if let Some(url) = value.get("download_url").and_then(|item| item.as_str()) {
            if !version.is_empty() {
                return Ok((version, url.to_owned()));
            }
        }
    }
    let value: serde_json::Value = http()
        .get("https://api.github.com/repos/yushijinhun/authlib-injector/releases/latest")
        .send()
        .and_then(|response| response.json())
        .map_err(|error| format!("拿不到 authlib-injector 的版本信息：{error}"))?;
    let version = value
        .get("tag_name")
        .and_then(|item| item.as_str())
        .unwrap_or_default()
        .trim_start_matches('v')
        .to_owned();
    let url = value
        .get("assets")
        .and_then(|item| item.as_array())
        .and_then(|assets| {
            assets.iter().find(|asset| {
                asset
                    .get("name")
                    .and_then(|name| name.as_str())
                    .is_some_and(|name| name.ends_with(".jar"))
            })
        })
        .and_then(|asset| asset.get("browser_download_url"))
        .and_then(|item| item.as_str())
        .ok_or_else(|| "authlib-injector 的版本信息里没有下载地址。".to_string())?;
    Ok((version, url.to_owned()))
}

/// yggdrasil 的 `validate`：令牌还有效就返回 true，避免每次启动都 refresh。
pub fn validate(server: &Server, token: &str) -> bool {
    let body = serde_json::json!({ "accessToken": token });
    matches!(
        post(&format!("{}/authserver/validate", server.url), &body),
        Ok((200, _))
    )
}

/// 第三方服务器的外链登录（OIDC 设备码）。只有开了 `feature.openid_configuration_url`
/// 的服务器支持；拿不到就直接报错，让用户回去用密码登录。
#[derive(Debug, Clone)]
pub struct DeviceLogin {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
    pub client_id: String,
    token_endpoint: String,
    jwks_uri: Option<String>,
}

/// 一次设备码轮询的结果。
#[derive(Debug)]
pub enum DevicePoll {
    /// 用户还没在浏览器里确认。
    Pending,
    /// 服务器要求放慢轮询。
    SlowDown,
    /// 用户点了拒绝。
    Denied,
    /// 设备码过期。
    Expired,
    Ready(Box<Profile>),
}

fn openid_config(server: &Server) -> Result<serde_json::Value, String> {
    let metadata = server
        .metadata
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .ok_or_else(|| "这个服务器没有给出元数据，用不了外链登录。".to_string())?;
    let endpoint = metadata
        .pointer("/meta/feature.openid_configuration_url")
        .and_then(|item| item.as_str())
        .ok_or_else(|| "这个服务器不支持外链登录，用密码登录吧。".to_string())?;
    http()
        .get(endpoint)
        .send()
        .and_then(|response| response.json())
        .map_err(|error| format!("读不到这个服务器的登录配置：{error}"))
}

/// 内置的 client id 表：公益服的 OAuth 应用都是各服自己报备的，按域名对。
fn builtin_client_id(server: &Server) -> Option<String> {
    let host = server
        .url
        .split("//")
        .nth(1)?
        .split('/')
        .next()?
        .to_ascii_lowercase();
    let table = [
        ("skin.mc.sjtu.cn", "6"),
        ("skin.mualliance.ltd", "27"),
        ("littleskin.cn", "1014"),
        ("oauth.ustb.world", "6"),
        ("skin.jsumc.fun", "2"),
        ("user.suesmc.ltd", "4"),
    ];
    table
        .iter()
        .find(|(domain, _)| host == *domain || host.ends_with(&format!(".{domain}")))
        .map(|(_, id)| (*id).to_owned())
}

/// 发起设备码登录：返回用户要打开/输入的东西。
pub fn start_device_login(server: &Server) -> Result<DeviceLogin, String> {
    let config = openid_config(server)?;
    let device_endpoint = config
        .get("device_authorization_endpoint")
        .and_then(|item| item.as_str())
        .ok_or_else(|| "这个服务器的登录配置里没有设备码接口。".to_string())?;
    let token_endpoint = config
        .get("token_endpoint")
        .and_then(|item| item.as_str())
        .ok_or_else(|| "这个服务器的登录配置里没有换票接口。".to_string())?
        .to_owned();
    let client_id = server
        .client_id
        .clone()
        .filter(|id| !id.trim().is_empty())
        .or_else(|| {
            config
                .get("shared_client_id")
                .and_then(|item| item.as_str())
                .map(str::to_owned)
        })
        .or_else(|| builtin_client_id(server))
        .ok_or_else(|| "不知道该用哪个 client id，这个服务器没法外链登录。".to_string())?;
    let response = http()
        .post(device_endpoint)
        .form(&[
            ("client_id", client_id.as_str()),
            (
                "scope",
                "openid offline_access Yggdrasil.PlayerProfiles.Select Yggdrasil.Server.Join",
            ),
        ])
        .send()
        .map_err(|error| format!("发不起设备码请求：{error}"))?;
    let status = response.status().as_u16();
    let text = response.text().unwrap_or_default();
    if status != 200 {
        return Err(format!("服务器拒绝了外链登录（HTTP {status}）。"));
    }
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| format!("设备码返回看不懂：{error}"))?;
    let verification = value
        .get("verification_uri_complete")
        .and_then(|item| item.as_str())
        .or_else(|| value.get("verification_uri").and_then(|item| item.as_str()))
        .unwrap_or_default()
        .to_owned();
    Ok(DeviceLogin {
        device_code: value
            .get("device_code")
            .and_then(|item| item.as_str())
            .unwrap_or_default()
            .to_owned(),
        user_code: value
            .get("user_code")
            .and_then(|item| item.as_str())
            .unwrap_or_default()
            .to_owned(),
        verification_uri: verification,
        interval: value.get("interval").and_then(|item| item.as_u64()).unwrap_or(5),
        expires_in: value
            .get("expires_in")
            .and_then(|item| item.as_u64())
            .unwrap_or(600),
        client_id,
        token_endpoint,
        jwks_uri: config
            .get("jwks_uri")
            .and_then(|item| item.as_str())
            .map(str::to_owned),
    })
}

/// 轮询设备码；用户确认后返回角色（含皮肤）。
pub fn poll_device_login(server: &Server, login: &DeviceLogin) -> Result<DevicePoll, String> {
    let response = http()
        .post(&login.token_endpoint)
        .form(&[
            ("client_id", login.client_id.as_str()),
            ("device_code", login.device_code.as_str()),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .map_err(|error| format!("轮询登录状态失败：{error}"))?;
    let status = response.status().as_u16();
    let text = response.text().unwrap_or_default();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    if status != 200 {
        return Ok(match value.get("error").and_then(|item| item.as_str()) {
            Some("authorization_pending") => DevicePoll::Pending,
            Some("slow_down") => DevicePoll::SlowDown,
            Some("access_denied") => DevicePoll::Denied,
            Some("expired_token") => DevicePoll::Expired,
            Some(other) => return Err(format!("登录失败：{other}")),
            None => return Err(format!("登录失败（HTTP {status}）。")),
        });
    }
    let access = value
        .get("access_token")
        .and_then(|item| item.as_str())
        .ok_or_else(|| "登录返回里没有 access_token。".to_string())?
        .to_owned();
    let refresh_token = value
        .get("refresh_token")
        .and_then(|item| item.as_str())
        .map(str::to_owned);
    let id = value
        .get("id_token")
        .and_then(|item| item.as_str())
        .map(|token| verify_id_token(token, login))
        .transpose()?
        .and_then(|claims| {
            claims
                .pointer("/selectedProfile/id")
                .and_then(|item| item.as_str())
                .map(str::to_owned)
        })
        .ok_or_else(|| "登录成功但没有返回角色 id。".to_string())?;
    let look = profile(server, &id).unwrap_or_else(|| Look {
        uuid: id.clone(),
        name: id.clone(),
        ..Look::default()
    });
    Ok(DevicePoll::Ready(Box::new(Profile {
        name: look.name,
        uuid: look.uuid,
        token: access,
        avatar: look.avatar,
        skin: look.skin,
        cape: look.cape,
        skin_model: look.skin_model,
        refresh_token,
    })))
}

/// 外链登录的账户过期后用 OIDC 的 refresh_token 换票；有的服务器会顺带换一个新的
/// refresh_token，那就一起返回，由调用方写回账号。
pub fn refresh_oauth(server: &Server, refresh_token: &str) -> Result<(String, Option<String>), String> {
    let config = openid_config(server)?;
    let endpoint = config
        .get("token_endpoint")
        .and_then(|item| item.as_str())
        .ok_or_else(|| "这个服务器的登录配置里没有换票接口。".to_string())?;
    let client_id = server
        .client_id
        .clone()
        .filter(|id| !id.trim().is_empty())
        .or_else(|| {
            config
                .get("shared_client_id")
                .and_then(|item| item.as_str())
                .map(str::to_owned)
        })
        .or_else(|| builtin_client_id(server))
        .ok_or_else(|| "不知道该用哪个 client id，重新登录一次吧。".to_string())?;
    let response = http()
        .post(endpoint)
        .form(&[
            ("client_id", client_id.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .send()
        .map_err(|error| format!("刷新登录状态失败：{error}"))?;
    let status = response.status().as_u16();
    let value: serde_json::Value = response
        .json()
        .map_err(|error| format!("刷新登录状态失败：{error}"))?;
    if status != 200 {
        let reason = value
            .get("error")
            .and_then(|item| item.as_str())
            .unwrap_or("unknown");
        return Err(format!("登录已过期（{reason}），重新登录一次。"));
    }
    let access = value
        .get("access_token")
        .and_then(|item| item.as_str())
        .ok_or_else(|| "刷新返回里没有 access_token。".to_string())?
        .to_owned();
    let next = value
        .get("refresh_token")
        .and_then(|item| item.as_str())
        .map(str::to_owned);
    Ok((access, next))
}

/// 用服务器的 JWKS 验 id_token 的 RS256 签名，顺便核对 aud。
fn verify_id_token(token: &str, login: &DeviceLogin) -> Result<serde_json::Value, String> {
    use rsa::signature::Verifier;
    let mut parts = token.split('.');
    let (Some(head), Some(payload), Some(signature)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err("id_token 格式不对。".into());
    };
    let header: serde_json::Value = serde_json::from_slice(&b64url(head)?)
        .map_err(|error| format!("id_token 头看不懂：{error}"))?;
    let claims: serde_json::Value = serde_json::from_slice(&b64url(payload)?)
        .map_err(|error| format!("id_token 内容看不懂：{error}"))?;
    if claims.get("aud").and_then(|item| item.as_str()) != Some(login.client_id.as_str()) {
        return Err("id_token 不是发给这个启动器的。".into());
    }
    let Some(jwks_uri) = login.jwks_uri.as_deref() else {
        return Ok(claims);
    };
    let jwks: serde_json::Value = http()
        .get(jwks_uri)
        .send()
        .and_then(|response| response.json())
        .map_err(|error| format!("读不到服务器的公钥：{error}"))?;
    let kid = header.get("kid").and_then(|item| item.as_str());
    let key = jwks
        .get("keys")
        .and_then(|item| item.as_array())
        .and_then(|keys| {
            keys.iter().find(|key| {
                kid.is_none_or(|kid| key.get("kid").and_then(|k| k.as_str()) == Some(kid))
            })
        })
        .ok_or_else(|| "服务器的公钥里没有对应的那把。".to_string())?;
    let modulus = key
        .get("n")
        .and_then(|item| item.as_str())
        .ok_or_else(|| "公钥缺少 n。".to_string())?;
    let exponent = key
        .get("e")
        .and_then(|item| item.as_str())
        .ok_or_else(|| "公钥缺少 e。".to_string())?;
    let public = rsa::RsaPublicKey::new(
        rsa::BigUint::from_bytes_be(&b64url(modulus)?),
        rsa::BigUint::from_bytes_be(&b64url(exponent)?),
    )
    .map_err(|error| format!("公钥读不出来：{error}"))?;
    let verifying = rsa::pkcs1v15::VerifyingKey::<sha2::Sha256>::new(public);
    let signature = rsa::pkcs1v15::Signature::try_from(b64url(signature)?.as_slice())
        .map_err(|_| "id_token 签名格式不对。".to_string())?;
    verifying
        .verify(format!("{head}.{payload}").as_bytes(), &signature)
        .map_err(|_| "id_token 签名不对，可能被改过。".to_string())?;
    Ok(claims)
}

fn b64url(input: &str) -> Result<Vec<u8>, String> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    URL_SAFE_NO_PAD
        .decode(input)
        .map_err(|error| format!("base64 解码失败：{error}"))
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
        let look = profile(server, &id).unwrap_or_else(|| Look {
            uuid: id.clone(),
            name: name.clone(),
            ..Look::default()
        });
        out.push(Profile {
            name: look.name,
            uuid: look.uuid,
            token: token.clone(),
            avatar: look.avatar,
            skin: look.skin,
            cape: look.cape,
            skin_model: look.skin_model,
            refresh_token: None,
        });
    }
    Ok(out)
}

/// `{server}/sessionserver/session/minecraft/profile/<uuid>`：名字和皮肤。
/// 档案 + 皮肤纹理，`profile()` 一次拿全，省得再请求一遍。
#[derive(Default)]
struct Look {
    uuid: String,
    name: String,
    avatar: Option<String>,
    skin: Option<String>,
    cape: Option<String>,
    skin_model: Option<String>,
}

fn profile(server: &Server, id: &str) -> Option<Look> {
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
    let textures = value
        .get("properties")
        .and_then(|item| item.as_array())
        .and_then(|properties| {
            properties
                .iter()
                .find(|property| property.get("name").and_then(|n| n.as_str()) == Some("textures"))
        })
        .and_then(|property| property.get("value").and_then(|item| item.as_str()))
        .and_then(decoded_textures);
    let skin_url = textures
        .as_ref()
        .and_then(|value| value.pointer("/textures/SKIN/url"))
        .and_then(|item| item.as_str());
    let skin_model = textures
        .as_ref()
        .and_then(|value| value.pointer("/textures/SKIN/metadata/model"))
        .and_then(|item| item.as_str())
        .map(str::to_owned);
    let cape_url = textures
        .as_ref()
        .and_then(|value| value.pointer("/textures/CAPE/url"))
        .and_then(|item| item.as_str());
    let skin_png = skin_url.and_then(|url| auth::fetch_image(url).ok());
    let avatar = skin_png.as_deref().and_then(auth::head_avatar_png);
    let skin = skin_png.as_deref().map(auth::data_url);
    let cape = cape_url
        .and_then(|url| auth::fetch_image(url).ok())
        .map(|png| auth::data_url(&png));
    Some(Look {
        uuid,
        name,
        avatar,
        skin,
        cape,
        skin_model,
    })
}

/// textures 属性：base64 的 `{ "textures": { "SKIN": { "url": … } } }`。
fn decoded_textures(encoded: &str) -> Option<serde_json::Value> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// textures 属性里的 SKIN 地址（测试与旧调用点用，正式解析走 `decoded_textures`）。
fn texture_url(encoded: &str) -> Option<String> {
    decoded_textures(encoded)?
        .pointer("/textures/SKIN/url")?
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
