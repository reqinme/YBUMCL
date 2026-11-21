//! 离线账户的本地 Yggdrasil 服务。
//!
//! 离线账户本身没有皮肤，但 authlib-injector 会把游戏对「会话服务器」的请求
//! 转到注入时指定的地址。这里在 `127.0.0.1` 起一个小服务，按 Yggdrasil 协议
//! 交出用户的皮肤（profile 里带签名的 textures 属性），单人/局域网里就能看到
//! 自己的皮肤——HMCL 的离线账户也是这么做的。
//!
//! 只服务一个账户、只在启动游戏期间存在，不对外监听。

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::{EncodePublicKey, LineEnding};
use rsa::signature::{SignatureEncoding, Signer};
use rsa::{RsaPrivateKey, RsaPublicKey};
use sha1::Sha1;

/// 起好之后一直留到进程结束：游戏在跑的时候服务必须在。
static SERVERS: OnceLock<Mutex<Vec<LocalYggdrasil>>> = OnceLock::new();

pub struct LocalYggdrasil {
    /// 注入时传给 authlib-injector 的地址。
    pub url: String,
    /// 元数据原文（注入时作为 `-Dauthlibinjector.yggdrasil.prefetched`）。
    pub metadata: String,
}

struct Context {
    name: String,
    uuid: String,
    png: Vec<u8>,
    cape: Option<Vec<u8>>,
    model: String,
    base: String,
    key: RsaPrivateKey,
    public_key: String,
    metadata: String,
}

/// 起一个本地服务并把它挂在静态表里（进程结束自动释放）。
pub fn start(
    name: &str,
    uuid: &str,
    png: Vec<u8>,
    model: &str,
    cape: Option<Vec<u8>>,
) -> Result<LocalYggdrasil, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("本地皮肤服务起不来：{e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("本地皮肤服务起不来：{e}"))?
        .port();
    let base = format!("http://127.0.0.1:{port}");
    let key = RsaPrivateKey::new(&mut rand::thread_rng(), 2048)
        .map_err(|e| format!("生成皮肤签名密钥失败：{e}"))?;
    let public_key = RsaPublicKey::from(&key)
        .to_public_key_pem(LineEnding::LF)
        .map_err(|e| format!("导出公钥失败：{e}"))?;
    let metadata = build_metadata(&base, &public_key, &key)?;

    let context = Arc::new(Context {
        name: name.to_owned(),
        uuid: uuid.replace('-', "").to_ascii_lowercase(),
        png,
        cape,
        model: if model.eq_ignore_ascii_case("slim") {
            "slim".into()
        } else {
            "classic".into()
        },
        base: base.clone(),
        key,
        public_key,
        metadata: metadata.clone(),
    });

    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let context = Arc::clone(&context);
            std::thread::spawn(move || {
                let _ = serve(stream, &context);
            });
        }
    });

    let server = LocalYggdrasil {
        url: base,
        metadata,
    };
    keep(&server);
    Ok(server)
}

fn keep(server: &LocalYggdrasil) {
    if let Ok(mut list) = SERVERS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
    {
        list.push(LocalYggdrasil {
            url: server.url.clone(),
            metadata: server.metadata.clone(),
        });
    }
}

fn serve(mut stream: TcpStream, context: &Context) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }
    let mut parts = request_line.split_whitespace();
    let _method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    let path = target.split('?').next().unwrap_or("/");

    // 请求头读掉即可，我们不需要里面的内容。
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
    }

    match path {
        "/" => respond_json(&mut stream, 200, &context.metadata),
        "/textures/skin" => respond_bytes(&mut stream, 200, "image/png", &context.png),
        "/textures/cape" => {
            let cape = context.cape.as_deref().unwrap_or(&[]);
            respond_bytes(&mut stream, 200, "image/png", cape)
        }
        "/sessionserver/session/minecraft/join" => respond_bytes(&mut stream, 204, "text/plain", &[]),
        p if p.starts_with("/sessionserver/session/minecraft/profile/")
            || p == "/sessionserver/session/minecraft/hasJoined" =>
        {
            let profile = build_profile(context)?;
            respond_json(&mut stream, 200, &profile)
        }
        _ => respond_json(&mut stream, 404, r#"{"error":"Not Found"}"#),
    }
}

fn build_metadata(_base: &str, public_key: &str, key: &RsaPrivateKey) -> Result<String, String> {
    let body = serde_json::json!({
        "meta": {
            "serverName": "YBUMCL",
            "implementationName": "YBUMCL",
            "implementationVersion": env!("CARGO_PKG_VERSION"),
        },
        "skinDomains": ["127.0.0.1", "localhost"],
        "signaturePublickey": public_key,
    });
    let text = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    let signature = sign(key, text.as_bytes())?;
    let mut value: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    value["signature"] = serde_json::Value::String(signature);
    serde_json::to_string(&value).map_err(|e| e.to_string())
}

fn build_profile(context: &Context) -> std::io::Result<String> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or(0);
    let mut textures = serde_json::json!({
        "timestamp": timestamp,
        "profileId": context.uuid,
        "profileName": context.name,
        "textures": {
            "SKIN": {
                "url": format!("{}/textures/skin", context.base),
                "metadata": { "model": context.model },
            }
        }
    });
    // 有披风就一并交出去，游戏会自己贴到背后。
    if context.cape.is_some() {
        textures["textures"]["CAPE"] =
            serde_json::json!({ "url": format!("{}/textures/cape", context.base) });
    }
    let value = serde_json::to_string(&textures).map_err(|e| std::io::Error::other(e.to_string()))?;
    let signature = sign(&context.key, value.as_bytes())
        .map_err(|e| std::io::Error::other(e))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(value.as_bytes());
    let body = serde_json::json!({
        "id": context.uuid,
        "name": context.name,
        "properties": [{ "name": "textures", "value": encoded, "signature": signature }],
    });
    let _ = &context.public_key;
    serde_json::to_string(&body).map_err(|e| std::io::Error::other(e.to_string()))
}

fn sign(key: &RsaPrivateKey, data: &[u8]) -> Result<String, String> {
    let signing = SigningKey::<Sha1>::new(key.clone());
    let signature = signing.sign(data);
    Ok(base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()))
}

fn respond_json(stream: &mut TcpStream, code: u16, body: &str) -> std::io::Result<()> {
    respond_bytes(stream, code, "application/json; charset=utf-8", body.as_bytes())
}

fn respond_bytes(
    stream: &mut TcpStream,
    code: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let reason = match code {
        200 => "OK",
        204 => "No Content",
        _ => "Not Found",
    };
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 起一个服务，按游戏端会走的那几条路由取一遍，确认皮肤 URL 与签名都在。
    #[test]
    fn serves_metadata_and_profile() {
        let server = start(
            "Steve",
            "0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f",
            vec![1, 2, 3],
            "slim",
            Some(vec![4, 5, 6]),
        )
        .expect("服务应该能起来");
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();

        let meta: serde_json::Value = client
            .get(format!("{}/", server.url))
            .send()
            .unwrap()
            .json()
            .unwrap();
        assert_eq!(meta["meta"]["serverName"], "YBUMCL");
        assert!(meta["signaturePublickey"].as_str().unwrap().contains("BEGIN PUBLIC KEY"));

        let profile: serde_json::Value = client
            .get(format!(
                "{}/sessionserver/session/minecraft/profile/0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f",
                server.url
            ))
            .send()
            .unwrap()
            .json()
            .unwrap();
        assert_eq!(profile["name"], "Steve");
        let encoded = profile["properties"][0]["value"].as_str().unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let textures: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert!(textures["textures"]["SKIN"]["url"]
            .as_str()
            .unwrap()
            .ends_with("/textures/skin"));
        assert_eq!(textures["textures"]["SKIN"]["metadata"]["model"], "slim");
        assert!(profile["properties"][0]["signature"].as_str().unwrap().len() > 100);

        assert_eq!(
            textures["textures"]["CAPE"]["url"].as_str().unwrap(),
            format!("{}/textures/cape", server.url)
        );

        let skin = client
            .get(format!("{}/textures/skin", server.url))
            .send()
            .unwrap();
        assert_eq!(skin.bytes().unwrap().as_ref(), &[1, 2, 3]);

        let cape = client
            .get(format!("{}/textures/cape", server.url))
            .send()
            .unwrap();
        assert_eq!(cape.bytes().unwrap().as_ref(), &[4, 5, 6]);
    }
}
