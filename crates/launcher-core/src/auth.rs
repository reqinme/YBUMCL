//! 账户：离线和微软（设备代码流）档案，落在数据目录的 `accounts.json`。
//!
//! 微软这条链和 PCL / SJMCL 一样：设备码拿 OAuth 令牌 → Xbox Live → XSTS →
//! Minecraft 访问令牌 → profile。刷新令牌用 DPAPI 加密后再写盘（只有当前
//! Windows 用户解得开），访问令牌只放在内存里，进程退出即失效。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const ACCOUNTS_FILE: &str = "accounts.json";
pub const SCHEMA_VERSION: u32 = 1;

/// 启动器自己注册的 Azure 应用（仅限个人 Microsoft 账户，允许公共客户端流）。
pub const CLIENT_ID: &str = "0190d27f-2112-45ba-8ae1-4ebba66ca60c";
const AUTHORITY: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0";
const XBL_AUTH: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_AUTH: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_API: &str = "https://api.minecraftservices.com";
const SCOPE: &str = "XboxLive.signin offline_access";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountKind {
    #[default]
    Offline,
    Microsoft,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Account {
    /// 本地唯一 id（离线是 `offline-<uuid>`，微软是 `ms-<uuid>`）。
    pub id: String,
    pub name: String,
    /// 档案 UUID，小写无连字符。
    pub uuid: String,
    pub kind: AccountKind,
    /// 头像（`data:image/png;base64,…`），离线账户没有。
    pub avatar: Option<String>,
    /// 刷新令牌，DPAPI 保护后的 base64；离线账户为 None。不往界面送。
    #[serde(skip_serializing)]
    pub refresh: Option<String>,
    /// 访问令牌只在内存里用，不落盘。
    #[serde(skip)]
    pub access: Option<String>,
    /// 访问令牌过期时间（epoch 秒）。
    #[serde(default)]
    pub access_expires: u64,
}

impl Account {
    pub fn subtitle(&self) -> &'static str {
        match self.kind {
            AccountKind::Offline => "离线账户",
            AccountKind::Microsoft => "微软账户",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Store {
    pub schema_version: u32,
    /// 当前选中的账户 id。
    pub current: Option<String>,
    pub players: Vec<Account>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            current: None,
            players: Vec::new(),
        }
    }
}

fn accounts_path(dir: &Path) -> PathBuf {
    dir.join(ACCOUNTS_FILE)
}

impl Store {
    pub fn load(dir: &Path) -> Self {
        let path = accounts_path(dir);
        let Ok(text) = fs::read_to_string(&path) else {
            return Self::default();
        };
        let mut store: Store = serde_json::from_str(&text).unwrap_or_default();
        store.schema_version = SCHEMA_VERSION;
        if store.current.is_none() {
            store.current = store.players.first().map(|player| player.id.clone());
        }
        store
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let path = accounts_path(dir);
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        // 先写临时文件再换名，避免写一半崩掉把账户表弄坏。
        let temp = path.with_extension("json.tmp");
        fs::create_dir_all(dir)?;
        fs::write(&temp, text)?;
        fs::rename(&temp, &path)
    }

    pub fn current(&self) -> Option<&Account> {
        let id = self.current.as_ref()?;
        self.players.iter().find(|player| &player.id == id)
    }

    pub fn current_mut(&mut self) -> Option<&mut Account> {
        let id = self.current.clone()?;
        self.players.iter_mut().find(|player| player.id == id)
    }

    pub fn select(&mut self, id: &str) -> Result<(), String> {
        if !self.players.iter().any(|player| player.id == id) {
            return Err("找不到这个账户，可能已经被删除了。".into());
        }
        self.current = Some(id.to_owned());
        Ok(())
    }

    /// 离线账户用「玩家名」当身份：UUID 由名字按 Minecraft 的老规矩算出来。
    pub fn add_offline(&mut self, name: &str) -> Result<String, String> {
        let name = normalize_name(name)?;
        if self
            .players
            .iter()
            .any(|player| player.kind == AccountKind::Offline && player.name.eq_ignore_ascii_case(&name))
        {
            return Err(format!("已经有一个叫 {name} 的离线账户了。"));
        }
        let uuid = offline_uuid(&name);
        let id = format!("offline-{uuid}");
        self.players.push(Account {
            id: id.clone(),
            name,
            uuid,
            kind: AccountKind::Offline,
            ..Account::default()
        });
        self.current = Some(id.clone());
        Ok(id)
    }

    pub fn rename(&mut self, id: &str, name: &str) -> Result<(), String> {
        let name = normalize_name(name)?;
        let kind = self
            .players
            .iter()
            .find(|player| player.id == id)
            .map(|player| player.kind)
            .ok_or_else(|| "找不到这个账户。".to_string())?;
        if kind == AccountKind::Microsoft {
            return Err("微软账户的名字由微软那边决定，不能在启动器里改。".into());
        }
        if self
            .players
            .iter()
            .any(|player| player.id != id && player.name.eq_ignore_ascii_case(&name))
        {
            return Err(format!("已经有一个叫 {name} 的账户了。"));
        }
        if let Some(player) = self.players.iter_mut().find(|player| player.id == id) {
            // id 是界面和启动请求拿来认人的，改名只换 UUID 和显示名，id 不动。
            player.uuid = offline_uuid(&name);
            player.name = name;
        }
        Ok(())
    }

    pub fn remove(&mut self, id: &str) {
        self.players.retain(|player| player.id != id);
        if self.current.as_deref() == Some(id) {
            self.current = self.players.first().map(|player| player.id.clone());
        }
    }

    /// 把微软登录拿到的档案并进来：同一 UUID 视为同一个人，覆盖令牌与头像。
    pub fn absorb(&mut self, mut account: Account) -> String {
        if let Some(existing) = self
            .players
            .iter_mut()
            .find(|player| player.kind == AccountKind::Microsoft && player.uuid == account.uuid)
        {
            existing.name = account.name;
            existing.refresh = account.refresh.take();
            existing.access = account.access.take();
            existing.access_expires = account.access_expires;
            existing.avatar = account.avatar.take();
            self.current = Some(existing.id.clone());
            return existing.id.clone();
        }
        let id = format!("ms-{}", account.uuid);
        account.id = id.clone();
        self.players.push(account);
        self.current = Some(id.clone());
        id
    }
}

/// 离线名字：3–16 个字符，只允许英文、数字和下划线（中文名进不了大多数服务器）。
pub fn normalize_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.chars().count() < 3 || name.chars().count() > 16 {
        return Err("玩家名要 3–16 个字符。".into());
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err("玩家名只能用英文、数字和下划线。".into());
    }
    Ok(name.to_owned())
}

/// 离线 UUID：`md5("OfflinePlayer:<name>")` 再按 Java 的规矩打上版本位。
pub fn offline_uuid(name: &str) -> String {
    let mut bytes = md5::compute(format!("OfflinePlayer:{name}")).0;
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|span| span.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
    pub message: String,
}

/// 一次轮询的结果：还没确认、确认了、或者失败。
#[derive(Debug)]
pub enum Poll {
    Pending,
    Done(Box<Account>),
    Failed(String),
}

fn client() -> &'static reqwest::blocking::Client {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("YBUMCL/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new())
    })
}

fn post_form(url: &str, form: &[(&str, &str)]) -> Result<(u16, String), String> {
    let response = client()
        .post(url)
        .form(form)
        .send()
        .map_err(|error| format!("连不上微软登录服务：{error}"))?;
    let status = response.status().as_u16();
    let text = response.text().unwrap_or_default();
    Ok((status, text))
}

fn post_json(url: &str, body: serde_json::Value) -> Result<(u16, String), String> {
    let response = client()
        .post(url)
        .json(&body)
        .send()
        .map_err(|error| format!("连不上 Xbox Live：{error}"))?;
    let status = response.status().as_u16();
    let text = response.text().unwrap_or_default();
    Ok((status, text))
}

/// 开始设备代码登录：界面把 `user_code` 和 `verification_uri` 显示给用户。
pub fn start_device_code() -> Result<DeviceCode, String> {
    let (status, text) = post_form(
        &format!("{AUTHORITY}/devicecode"),
        &[("client_id", CLIENT_ID), ("scope", SCOPE)],
    )?;
    if status != 200 {
        return Err(format!("微软没有给出设备码（HTTP {status}）：{}", http_error(&text)));
    }
    let body: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("设备码返回看不懂：{error}"))?;
    Ok(DeviceCode {
        device_code: string_field(&body, "device_code"),
        user_code: string_field(&body, "user_code"),
        verification_uri: string_field(&body, "verification_uri"),
        interval: body.get("interval").and_then(|value| value.as_u64()).unwrap_or(5),
        expires_in: body.get("expires_in").and_then(|value| value.as_u64()).unwrap_or(900),
        message: string_field(&body, "message"),
    })
}

/// 轮询一次：用户还没在浏览器里确认时返回 `Pending`（属于正常情况）。
pub fn poll_device_code(device_code: &str) -> Result<Poll, String> {
    let (status, text) = post_form(
        &format!("{AUTHORITY}/token"),
        &[
            ("client_id", CLIENT_ID),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", device_code),
        ],
    )?;
    let body: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    if status == 200 {
        let access = string_field(&body, "access_token");
        let refresh = string_field(&body, "refresh_token");
        if access.is_empty() {
            return Ok(Poll::Failed("微软返回了空令牌，再试一次。".into()));
        }
        return finish_microsoft(&access, non_empty(refresh));
    }
    Ok(match body.get("error").and_then(|value| value.as_str()) {
        Some("authorization_pending") => Poll::Pending,
        Some("slow_down") => Poll::Pending,
        Some("expired_token") => Poll::Failed("这次登录超时了，重新点一次「微软账户」。".into()),
        Some("authorization_declined") => Poll::Failed("浏览器里拒绝了授权。".into()),
        Some(_) => Poll::Failed(format!("微软登录失败：{}", http_error(&text))),
        None => Poll::Failed(format!("微软登录失败（HTTP {status}）：{}", http_error(&text))),
    })
}

/// 用刷新令牌续期。成功时把新的访问令牌写回 `account`。
pub fn refresh(account: &mut Account) -> Result<(), String> {
    let encrypted = account
        .refresh
        .as_deref()
        .ok_or_else(|| "这个微软账户没有保存的登录状态，重新登录一次。".to_string())?;
    let token = unprotect(encrypted)
        .ok_or_else(|| "保存的登录状态解不开（换了 Windows 用户或被清理过），重新登录一次。".to_string())?;
    let (status, text) = post_form(
        &format!("{AUTHORITY}/token"),
        &[
            ("client_id", CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", &token),
            ("scope", SCOPE),
        ],
    )?;
    if status != 200 {
        return Err(format!(
            "登录状态过期了，重新登录一次（{}）。",
            http_error(&text)
        ));
    }
    let body: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("刷新返回看不懂：{error}"))?;
    let access = string_field(&body, "access_token");
    let new_refresh = non_empty(string_field(&body, "refresh_token")).or(Some(token.clone()));
    match finish_microsoft(&access, new_refresh)? {
        Poll::Done(fresh) => {
            account.access = fresh.access;
            account.access_expires = fresh.access_expires;
            account.refresh = fresh.refresh;
            if let Some(avatar) = fresh.avatar {
                account.avatar = Some(avatar);
            }
            account.name = fresh.name;
            Ok(())
        }
        Poll::Failed(error) => Err(error),
        Poll::Pending => Err("刷新登录状态时微软没有给出结果，请重试。".into()),
    }
}

/// 访问令牌还有效就什么都不做，过期了就用刷新令牌续上。
pub fn ensure_fresh(account: &mut Account) -> Result<(), String> {
    if account.kind != AccountKind::Microsoft {
        return Ok(());
    }
    if account.access.as_deref().is_some_and(|token| !token.is_empty())
        && account.access_expires > now_secs() + 60
    {
        return Ok(());
    }
    refresh(account)
}

/// 从微软访问令牌一路换到 Minecraft 档案（Xbox Live → XSTS → 档案 → 皮肤）。
fn finish_microsoft(ms_access: &str, refresh_token: Option<String>) -> Result<Poll, String> {
    let xbl = match xbox_live(ms_access) {
        Ok(value) => value,
        Err(error) => return Ok(Poll::Failed(error)),
    };
    let xsts = match xsts(&xbl.0) {
        Ok(value) => value,
        Err(error) => return Ok(Poll::Failed(error)),
    };
    let _ = xbl.1;
    let (mc_access, expires_in) = match minecraft_login(&xsts.0, &xsts.1) {
        Ok(value) => value,
        Err(error) => return Ok(Poll::Failed(error)),
    };
    if let Err(error) = check_entitlement(&mc_access) {
        return Ok(Poll::Failed(error));
    }
    let profile = match minecraft_profile(&mc_access) {
        Ok(value) => value,
        Err(error) => return Ok(Poll::Failed(error)),
    };
    let avatar = profile
        .skin
        .as_deref()
        .and_then(|url| head_avatar(url).ok().flatten());
    Ok(Poll::Done(Box::new(Account {
        id: String::new(),
        name: profile.name,
        uuid: profile.id.to_lowercase(),
        kind: AccountKind::Microsoft,
        avatar,
        refresh: refresh_token.as_deref().and_then(protect),
        access: Some(mc_access),
        access_expires: now_secs() + expires_in.max(60),
    })))
}

/// `d=` 前缀对应 Xbox Live 的 RPS 登录方式。
fn xbox_live(ms_access: &str) -> Result<(String, String), String> {
    let body = serde_json::json!({
        "Properties": {
            "AuthMethod": "RPS",
            "SiteName": "user.auth.xboxlive.com",
            "RpsTicket": format!("d={ms_access}")
        },
        "RelyingParty": "http://auth.xboxlive.com",
        "TokenType": "JWT"
    });
    let (status, text) = post_json(XBL_AUTH, body)?;
    if status != 200 {
        return Err(format!("Xbox Live 不认这次登录（HTTP {status}）：{}", http_error(&text)));
    }
    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let token = string_field(&value, "Token");
    let uhs = value
        .pointer("/DisplayClaims/xui/0/uhs")
        .and_then(|item| item.as_str())
        .unwrap_or_default()
        .to_owned();
    if token.is_empty() || uhs.is_empty() {
        return Err("Xbox Live 没有返回令牌。".into());
    }
    Ok((token, uhs))
}

fn xsts(xbl_token: &str) -> Result<(String, String), String> {
    let body = serde_json::json!({
        "Properties": {
            "SandboxId": "RETAIL",
            "UserTokens": [xbl_token]
        },
        "RelyingParty": "rp://api.minecraftservices.com/",
        "TokenType": "JWT"
    });
    let (status, text) = post_json(XSTS_AUTH, body)?;
    if status == 401 {
        return Err(xsts_error(&text));
    }
    if status != 200 {
        return Err(format!("Xbox 安全令牌服务报错（HTTP {status}）：{}", http_error(&text)));
    }
    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let token = string_field(&value, "Token");
    let uhs = value
        .pointer("/DisplayClaims/xui/0/uhs")
        .and_then(|item| item.as_str())
        .unwrap_or_default()
        .to_owned();
    if token.is_empty() {
        return Err("Xbox 安全令牌服务没有返回令牌。".into());
    }
    Ok((token, uhs))
}

/// XSTS 的 401 带一个 `XErr` 码，是「这个账号登不进 Minecraft」的具体原因。
fn xsts_error(text: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    let code = value.get("XErr").and_then(|item| item.as_u64()).unwrap_or(0);
    let hint = match code {
        2148916233 => "这个微软账号还没有 Xbox 档案，先去 xbox.com 建一个（免费的）。",
        2148916235 => "这个微软账号所在的国家/地区不提供 Xbox Live。",
        2148916236 | 2148916237 => "这个微软账号需要在 xbox.com 完成成人验证。",
        2148916238 => "这是未成年账号，需要一个成人账号把它加进 Microsoft 家庭组。",
        2148916227 => "这个微软账号被 Xbox Live 封禁了。",
        _ => "微软账号过不了 Xbox 安全令牌校验。",
    };
    if code == 0 {
        hint.into()
    } else {
        format!("{hint}（XErr {code}）")
    }
}

fn minecraft_login(xsts_token: &str, uhs: &str) -> Result<(String, u64), String> {
    let body = serde_json::json!({
        "identityToken": format!("XBL3.0 x={uhs};{xsts_token}")
    });
    let (status, text) = post_json(&format!("{MC_API}/authentication/login_with_xbox"), body)?;
    if status != 200 {
        return Err(mc_login_error(status, &text));
    }
    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let token = string_field(&value, "access_token");
    if token.is_empty() {
        return Err("Minecraft 服务没有返回访问令牌。".into());
    }
    let expires = value
        .get("expires_in")
        .and_then(|item| item.as_u64())
        .unwrap_or(86_400);
    Ok((token, expires))
}

/// Minecraft 接口的报错翻成人话。403 Invalid app registration 是审核问题：
/// 微软这边已经放行（XSTS 发下来了），拦下来的是 Mojang 的白名单。
fn mc_login_error(status: u16, text: &str) -> String {
    if status == 403 && text.contains("Invalid app registration") {
        return "微软已经登录成功，但 Minecraft 接口不认这个 Azure 应用（403 Invalid app registration）。应用要先向 Mojang 申请接入（https://aka.ms/AppRegInfo），批准前只能用离线账户。".into();
    }
    format!("Minecraft 服务不认这次登录（HTTP {status}）：{}", http_error(text))
}

/// 没有买过 Minecraft 的账号拿不到档案，早一点说清楚，别让用户对着错误码发呆。
fn check_entitlement(mc_access: &str) -> Result<(), String> {
    let response = client()
        .get(format!("{MC_API}/entitlements/mcstore"))
        .bearer_auth(mc_access)
        .send()
        .map_err(|error| format!("查不到这个账号有没有买过 Minecraft：{error}"))?;
    let status = response.status().as_u16();
    if status != 200 {
        return Ok(());
    }
    let value: serde_json::Value = response.json().unwrap_or_default();
    let empty = value
        .get("items")
        .and_then(|items| items.as_array())
        .is_none_or(|items| items.is_empty());
    if empty {
        return Err("这个微软账号没有买过 Minecraft，只能登录离线账户。".into());
    }
    Ok(())
}

struct Profile {
    id: String,
    name: String,
    skin: Option<String>,
}

fn minecraft_profile(mc_access: &str) -> Result<Profile, String> {
    let response = client()
        .get(format!("{MC_API}/minecraft/profile"))
        .bearer_auth(mc_access)
        .send()
        .map_err(|error| format!("读不到 Minecraft 档案：{error}"))?;
    let status = response.status().as_u16();
    let text = response.text().unwrap_or_default();
    if status != 200 {
        return Err(format!("读不到 Minecraft 档案（HTTP {status}）：{}", http_error(&text)));
    }
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("档案返回看不懂：{error}"))?;
    let id = string_field(&value, "id");
    let name = string_field(&value, "name");
    if id.is_empty() || name.is_empty() {
        return Err("Minecraft 档案缺少 id 或名字。".into());
    }
    let skin = value
        .pointer("/skins/0/url")
        .and_then(|item| item.as_str())
        .map(str::to_owned);
    Ok(Profile { id, name, skin })
}

/// 把皮肤裁成头像：取头部正面 8×8，再叠上帽子层，放大成 64×64。
fn head_avatar(url: &str) -> Result<Option<String>, String> {
    let bytes = client()
        .get(url)
        .send()
        .and_then(|response| response.bytes())
        .map_err(|error| format!("下载皮肤失败：{error}"))?;
    let image = image::load_from_memory(&bytes).map_err(|error| format!("皮肤文件看不懂：{error}"))?;
    let head = match head_image(&image.to_rgba8()) {
        Some(head) => head,
        None => return Ok(None),
    };
    let mut png = std::io::Cursor::new(Vec::new());
    head.write_to(&mut png, image::ImageFormat::Png)
        .map_err(|error| format!("头像编码失败：{error}"))?;
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(png.into_inner());
    Ok(Some(format!("data:image/png;base64,{encoded}")))
}

/// 从皮肤里裁出 64×64 的头像：头部正面 8×8 叠上帽子层，再按最近邻放大。
fn head_image(skin: &image::RgbaImage) -> Option<image::RgbaImage> {
    // 新版 64×64；老皮肤是 64×32，头部位置一样。
    if skin.width() < 64 || skin.height() < 32 {
        return None;
    }
    let mut head = image::RgbaImage::new(8, 8);
    for y in 0..8 {
        for x in 0..8 {
            let base = *skin.get_pixel(8 + x, 8 + y);
            let hat = *skin.get_pixel(40 + x, 8 + y);
            head.put_pixel(x, y, overlay(base, hat));
        }
    }
    Some(image::imageops::resize(
        &head,
        64,
        64,
        image::imageops::FilterType::Nearest,
    ))
}

/// 帽子层压在基础皮肤上（透明度按帽子自己的 alpha 混合）。
fn overlay(base: image::Rgba<u8>, hat: image::Rgba<u8>) -> image::Rgba<u8> {
    if hat[3] == 0 {
        return base;
    }
    let alpha = hat[3] as f32 / 255.0;
    let mix = |bottom: u8, top: u8| {
        ((top as f32 * alpha) + (bottom as f32 * (1.0 - alpha))).round() as u8
    };
    image::Rgba([
        mix(base[0], hat[0]),
        mix(base[1], hat[1]),
        mix(base[2], hat[2]),
        255,
    ])
}

fn string_field(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|item| item.as_str())
        .unwrap_or_default()
        .to_owned()
}

fn non_empty(value: String) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

/// 微软的报错里最有用的就是 `error_description`，别的都是追踪 id。
fn http_error(text: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    let description = string_field(&value, "error_description");
    if let Some(description) = description.split("Trace ID").next().map(str::trim) {
        if !description.is_empty() {
            return description.to_owned();
        }
    }
    let code = string_field(&value, "error");
    if !code.is_empty() {
        return code;
    }
    text.chars().take(200).collect()
}

#[cfg(windows)]
mod dpapi {
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    use windows_sys::Win32::Foundation::LocalFree;

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        }
    }

    pub fn protect(plain: &str) -> Option<Vec<u8>> {
        let input = blob(plain.as_bytes());
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return None;
        }
        let bytes = unsafe {
            std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
        };
        unsafe { LocalFree(output.pbData as *mut core::ffi::c_void) };
        Some(bytes)
    }

    pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
        let input = blob(data);
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return None;
        }
        let bytes = unsafe {
            std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
        };
        unsafe { LocalFree(output.pbData as *mut core::ffi::c_void) };
        Some(bytes)
    }
}

/// 刷新令牌写盘前用 DPAPI 包一层；解不开就当没有（让用户重新登录）。
fn protect(plain: &str) -> Option<String> {
    use base64::Engine;
    #[cfg(windows)]
    {
        let bytes = dpapi::protect(plain)?;
        return Some(format!(
            "dpapi:{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ));
    }
    #[cfg(not(windows))]
    {
        let _ = plain;
        None
    }
}

fn unprotect(stored: &str) -> Option<String> {
    use base64::Engine;
    let encoded = stored.strip_prefix("dpapi:").unwrap_or(stored);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    #[cfg(windows)]
    {
        let plain = dpapi::unprotect(&bytes)?;
        return String::from_utf8(plain).ok();
    }
    #[cfg(not(windows))]
    {
        let _ = bytes;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn temp_dir(tag: &str) -> PathBuf {
        // 这台机器上 %TEMP% 在测试进程里不可写，用仓库的 target 目录做临时区。
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("auth-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn offline_uuid_is_stable_and_versioned() {
        let uuid = offline_uuid("Steve");
        assert_eq!(uuid.len(), 32);
        assert_eq!(offline_uuid("Steve"), uuid);
        assert_ne!(offline_uuid("Alex"), uuid);
        // 第 13 位是版本号 3，第 17 位是 RFC 4122 的变体位。
        assert_eq!(&uuid[12..13], "3");
        assert!(matches!(&uuid[16..17], "8" | "9" | "a" | "b"));
    }

    #[test]
    fn names_follow_minecraft_rules() {
        assert!(normalize_name("Steve_1").is_ok());
        assert!(normalize_name("ab").is_err());
        assert!(normalize_name("bad name").is_err());
        assert!(normalize_name("中文名").is_err());
        assert!(normalize_name("abcdefghijklmnopq").is_err());
    }

    #[test]
    fn store_round_trips_and_keeps_current() {
        let dir = temp_dir("store");
        let mut store = Store::default();
        let steve = store.add_offline("Steve").unwrap();
        let alex = store.add_offline("Alex").unwrap();
        assert_eq!(store.current.as_deref(), Some(alex.as_str()));
        store.select(&steve).unwrap();
        store.save(&dir).unwrap();

        let loaded = Store::load(&dir);
        assert_eq!(loaded.players.len(), 2);
        assert_eq!(loaded.current.as_deref(), Some(steve.as_str()));
        assert_eq!(loaded.current().unwrap().name, "Steve");

        let mut loaded = loaded;
        loaded.rename(&steve, "Steve_2").unwrap();
        assert_eq!(loaded.current().unwrap().name, "Steve_2");
        loaded.remove(&steve);
        assert_eq!(loaded.current.as_deref(), Some(alex.as_str()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_offline_names_are_rejected() {
        let mut store = Store::default();
        store.add_offline("Steve").unwrap();
        assert!(store.add_offline("steve").is_err());
    }

    #[test]
    fn microsoft_account_replaces_same_uuid() {
        let mut store = Store::default();
        let first = store.absorb(Account {
            name: "A".into(),
            uuid: "0123456789abcdef0123456789abcdef".into(),
            kind: AccountKind::Microsoft,
            ..Account::default()
        });
        let second = store.absorb(Account {
            name: "B".into(),
            uuid: "0123456789abcdef0123456789abcdef".into(),
            kind: AccountKind::Microsoft,
            ..Account::default()
        });
        assert_eq!(first, second);
        assert_eq!(store.players.len(), 1);
        assert_eq!(store.players[0].name, "B");
        assert_eq!(store.current.as_deref(), Some(first.as_str()));
    }

    #[test]
    fn xsts_errors_are_translated() {
        assert!(xsts_error(r#"{"XErr":2148916233}"#).contains("Xbox 档案"));
        assert!(xsts_error(r#"{"XErr":2148916238}"#).contains("未成年"));
        assert!(xsts_error(r#"{"XErr":2148916227}"#).contains("封禁"));
        assert!(xsts_error("{}").contains("Xbox 安全令牌"));
    }

    #[test]
    fn head_avatar_blends_hat_over_face() {
        let mut skin = image::RgbaImage::from_pixel(64, 64, Rgba([0, 0, 0, 0]));
        for y in 8..16 {
            for x in 8..16 {
                skin.put_pixel(x, y, Rgba([255, 0, 0, 255]));
            }
        }
        // 帽子层在皮肤右侧的 (40, 8)，半透明蓝，只盖住上半部分。
        for y in 8..12 {
            for x in 40..48 {
                skin.put_pixel(x, y, Rgba([0, 0, 255, 128]));
            }
        }
        let head = head_image(&skin).unwrap();
        assert_eq!(head.dimensions(), (64, 64));
        let top = head.get_pixel(2, 2);
        assert!(top[2] > 100 && top[0] > 100, "帽子层要跟脸色混在一起");
        assert_eq!(*head.get_pixel(2, 60), Rgba([255, 0, 0, 255]));
    }

    #[test]
    fn small_skin_has_no_avatar() {
        let skin = image::RgbaImage::new(32, 32);
        assert!(head_image(&skin).is_none());
    }

    #[test]
    #[cfg(windows)]
    fn refresh_tokens_survive_dpapi_round_trip() {
        let wrapped = protect("secret-token").unwrap();
        assert!(wrapped.starts_with("dpapi:"));
        assert_eq!(unprotect(&wrapped).as_deref(), Some("secret-token"));
        assert!(unprotect("dpapi:bm90LWEtdG9rZW4=").is_none());
    }

    #[test]
    fn app_registration_error_says_what_to_do() {
        let body = r#"{"path":"/authentication/login_with_xbox","errorMessage":"Invalid app registration, see https://aka.ms/AppRegInfo for more information"}"#;
        let message = mc_login_error(403, body);
        assert!(message.contains("Mojang"), "{message}");
        assert!(message.contains("离线账户"), "{message}");
        let other = mc_login_error(500, "boom");
        assert!(other.starts_with("Minecraft 服务不认这次登录（HTTP 500）"), "{other}");
    }
}
