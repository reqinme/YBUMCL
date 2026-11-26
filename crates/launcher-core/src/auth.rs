//! 账户：离线和微软（设备代码流）档案，落在数据目录的 `accounts.json`。
//!
//! 微软这条链和 PCL / SJMCL 一样：设备码拿 OAuth 令牌 → Xbox Live → XSTS →
//! Minecraft 访问令牌 → profile。刷新令牌用 DPAPI 加密后再写盘（只有当前
//! Windows 用户解得开），访问令牌只放在内存里，进程退出即失效。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const ACCOUNTS_FILE: &str = "accounts.json";
pub const SCHEMA_VERSION: u32 = 2;

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
    /// 第三方认证服务器（authlib-injector / yggdrasil）。
    Thirdparty,
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
    /// 刷新令牌（微软）或 yggdrasil 访问令牌（第三方），DPAPI 保护后的 base64；
    /// 离线账户为 None。会写进 accounts.json，界面用 PlayerView 过滤掉。
    #[serde(default)]
    pub refresh: Option<String>,
    /// 访问令牌只在内存里用，不落盘。
    #[serde(skip)]
    pub access: Option<String>,
    /// 访问令牌过期时间（epoch 秒）。
    #[serde(default)]
    pub access_expires: u64,
    /// 第三方账户所属的认证服务器地址（离线 / 微软账户为 None）。
    #[serde(default)]
    pub server: Option<String>,
    /// 第三方外链登录换来的 OIDC 刷新令牌（DPAPI 保护）。密码登录的账户没有。
    #[serde(default)]
    pub oauth: Option<String>,
    /// 整张皮肤（`data:image/png;base64,…`），账号页拿来画全身预览。
    #[serde(default)]
    pub skin: Option<String>,
    /// 披风（同格式），没有就是 None。
    #[serde(default)]
    pub cape: Option<String>,
    /// 皮肤模型：`slim`（细手臂）或 `classic`。
    #[serde(default)]
    pub skin_model: Option<String>,
    /// 选的是哪一款官方默认皮肤（`steve`/`alex`/…）；自定义皮肤为 None。
    #[serde(default)]
    pub skin_official: Option<String>,
    /// 选的是哪一张内置披风（`minecon2011`/`migrator`/…）；自选文件为 None。
    #[serde(default)]
    pub cape_official: Option<String>,
}

impl Account {
    pub fn subtitle(&self) -> &'static str {
        match self.kind {
            AccountKind::Offline => "离线账户",
            AccountKind::Microsoft => "微软账户",
            AccountKind::Thirdparty => "第三方账户",
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
    /// 添加过的第三方认证服务器（用不到的不会有，删掉账户也不会自动删服务器）。
    #[serde(default)]
    pub auth_servers: Vec<crate::authlib::Server>,
    /// 用户主动删掉的默认服务器：记下来，下次启动别再自动塞回去。
    #[serde(default)]
    pub dismissed_servers: Vec<String>,
    /// 每个「来源」档自己的顺序：`all` / `offline` / `microsoft` / 第三方服务器 URL。
    ///
    /// 各档互不影响：在某一档里拖动只改这一档，切回「全部来源」看到的还是
    /// `players` 本来的顺序。没出现在列表里的账户排在末尾。
    #[serde(default)]
    pub orders: BTreeMap<String, Vec<String>>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            current: None,
            players: Vec::new(),
            auth_servers: Vec::new(),
            dismissed_servers: Vec::new(),
            orders: BTreeMap::new(),
        }
    }
}

fn accounts_path(dir: &Path) -> PathBuf {
    dir.join(ACCOUNTS_FILE)
}

/// 内置的第三方认证服务器：装上就有，不用用户自己填地址。
const DEFAULT_SERVERS: &[(&str, &str)] = &[(
    "https://skin.ybumua.homes/api/yggdrasil",
    "YBUMC用户中心",
)];

/// 早先写进账户表里的旧名字，读到时顺手改掉。
const RENAMED_SERVERS: &[(&str, &str)] = &[("YBU用户中心", "YBUMC用户中心")];

impl Store {
    /// 把内置服务器补进列表（用户删过的跳过）。
    fn seed_default_servers(&mut self) {
        for (url, name) in DEFAULT_SERVERS {
            if self.dismissed_servers.iter().any(|gone| gone == url) {
                continue;
            }
            if let Some(existing) = self.auth_servers.iter_mut().find(|server| server.url == *url) {
                if let Some((_, new_name)) = RENAMED_SERVERS.iter().find(|(old, _)| old == &existing.name) {
                    existing.name = (*new_name).to_owned();
                }
                continue;
            }
            self.auth_servers.push(crate::authlib::Server {
                url: (*url).to_owned(),
                name: (*name).to_owned(),
                client_id: None,
                metadata: None,
                timestamp: 0,
            });
        }
    }

    pub fn load(dir: &Path) -> Self {
        let path = accounts_path(dir);
        let Ok(text) = fs::read_to_string(&path) else {
            let mut store = Self::default();
            store.seed_default_servers();
            return store;
        };
        let mut store: Store = serde_json::from_str(&text).unwrap_or_default();
        // 旧版本没有 current 字段：只在迁移时补选第一个，补完写回一次。
        // 迁移之后再出现 `current: None` 就是用户主动取消选择（未登录），必须原样保留。
        let legacy = store.schema_version < SCHEMA_VERSION;
        store.schema_version = SCHEMA_VERSION;
        if legacy && store.current.is_none() {
            store.current = store.players.first().map(|player| player.id.clone());
            let _ = store.save(dir);
        }
        store.seed_default_servers();
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

    /// 取消选中：账户都还在，只是没有「当前账户」，启动器回到未登录状态。
    pub fn deselect(&mut self) {
        self.current = None;
    }

    /// 第三方账户：同一个服务器下按 UUID 合并（换过一次皮肤也只是同一个人）。
    pub fn add_third_party(&mut self, mut account: Account) -> String {
        let server = account.server.clone().unwrap_or_default();
        if let Some(existing) = self.players.iter_mut().find(|player| {
            player.kind == AccountKind::Thirdparty
                && player.uuid == account.uuid
                && player.server.as_deref() == Some(server.as_str())
        }) {
            existing.name = account.name;
            existing.avatar = account.avatar.take();
            existing.access = account.access.take();
            existing.refresh = account.refresh.take();
            existing.access_expires = account.access_expires;
            self.current = Some(existing.id.clone());
            return existing.id.clone();
        }
        let id = format!("tp-{}", account.uuid);
        account.id = id.clone();
        self.players.push(account);
        self.current = Some(id.clone());
        id
    }

    /// 认证服务器：同地址的更新名字与元数据，不重复添加。
    pub fn add_server(&mut self, server: crate::authlib::Server) {
        if let Some(existing) = self
            .auth_servers
            .iter_mut()
            .find(|saved| saved.url == server.url)
        {
            *existing = server;
        } else {
            self.auth_servers.push(server);
        }
    }

    /// 删服务器时把它名下的账户一起删掉（留着也登不进去）。
    pub fn remove_server(&mut self, url: &str) {
        if DEFAULT_SERVERS.iter().any(|(builtin, _)| builtin == &url) {
            self.dismissed_servers.push(url.to_owned());
        }
        self.auth_servers.retain(|server| server.url != url);
        let removed: Vec<String> = self
            .players
            .iter()
            .filter(|player| player.server.as_deref() == Some(url))
            .map(|player| player.id.clone())
            .collect();
        self.players
            .retain(|player| player.server.as_deref() != Some(url));
        if self.current.as_ref().is_some_and(|id| removed.contains(id)) {
            self.current = self.players.first().map(|player| player.id.clone());
        }
    }

    /// 离线账户用「玩家名」当身份：UUID 默认由名字按 Minecraft 的老规矩算出来，
    /// 也可以在「高级」里手填一个（服务器要求固定 UUID 时用）。
    pub fn add_offline(&mut self, name: &str, uuid: Option<&str>) -> Result<String, String> {
        let name = normalize_name(name)?;
        if self
            .players
            .iter()
            .any(|player| player.kind == AccountKind::Offline && player.name.eq_ignore_ascii_case(&name))
        {
            return Err(format!("已经有一个叫 {name} 的离线账户了。"));
        }
        let uuid = match uuid.map(str::trim).filter(|text| !text.is_empty()) {
            Some(text) => parse_uuid(text)?,
            None => offline_uuid(&name),
        };
        let mut id = format!("offline-{uuid}");
        let mut index = 2;
        while self.players.iter().any(|player| player.id == id) {
            id = format!("offline-{uuid}-{index}");
            index += 1;
        }
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

    /// 换掉离线账户的皮肤（`None` 表示去掉，回到首字母方块）。
    pub fn set_offline_look(
        &mut self,
        id: &str,
        skin: Option<String>,
        skin_model: Option<String>,
        skin_official: Option<String>,
        cape: Option<String>,
        cape_official: Option<String>,
    ) -> Result<(), String> {
        let player = self
            .players
            .iter_mut()
            .find(|player| player.id == id)
            .ok_or_else(|| "找不到这个账户。".to_string())?;
        if player.kind != AccountKind::Offline {
            return Err("只有离线账户可以在这里换皮肤。".into());
        }
        player.skin = skin;
        player.cape = cape;
        player.skin_official = skin_official;
        player.cape_official = cape_official;
        player.skin_model = skin_model.map(|model| {
            if model.eq_ignore_ascii_case("slim") {
                "slim".to_string()
            } else {
                "classic".to_string()
            }
        });
        Ok(())
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
            // 只换显示名：UUID 建号时就定了，改名后存档与服务器白名单仍认得是同一人。
            player.name = name;
        }
        Ok(())
    }

    pub fn remove(&mut self, id: &str) {
        self.players.retain(|player| player.id != id);
        // 删的是当前账户就回到未登录，不自动选下一个。
        if self.current.as_deref() == Some(id) {
            self.current = None;
        }
        // 每个来源档的顺序里也要抹掉这个 id：离线账户 id 是按名字算的，
        // 同名账户重新加回来时 id 相同，残留会把它「复活」到老位置或画出两张卡。
        for ids in self.orders.values_mut() {
            ids.retain(|kept| kept != id);
        }
    }

    /// 按档保存顺序。`all` 直接改 `players` 的数组顺序，其它档只存自己的列表。
    pub fn reorder(&mut self, filter: &str, ids: &[String]) -> Result<(), String> {
        let mut seen = std::collections::BTreeSet::new();
        for id in ids {
            if !seen.insert(id.clone()) {
                return Err("顺序里有重复的账户，刷新后再试。".into());
            }
            if !self.players.iter().any(|player| &player.id == id) {
                return Err(format!("找不到账户 {id}，刷新后再试。"));
            }
        }
        if filter == "all" {
            if ids.len() != self.players.len() {
                return Err("账户顺序和账户数量对不上，刷新后再试。".into());
            }
            let mut ordered = Vec::with_capacity(ids.len());
            for id in ids {
                ordered.push(self.players.iter().find(|player| &player.id == id).cloned().unwrap());
            }
            self.players = ordered;
            return Ok(());
        }
        self.orders.insert(filter.to_owned(), ids.to_vec());
        Ok(())
    }

    /// 某个来源档的顺序：先按存过的顺序，没出现过的账户按账户表顺序接在后面。
    pub fn order_for(&self, filter: &str) -> Vec<String> {
        if filter == "all" {
            return self.players.iter().map(|player| player.id.clone()).collect();
        }
        let saved = self.orders.get(filter);
        let mut out: Vec<String> = Vec::new();
        if let Some(ids) = saved {
            for id in ids {
                if self.players.iter().any(|player| &player.id == id) && !out.contains(id) {
                    out.push(id.clone());
                }
            }
        }
        for player in &self.players {
            let visible = match filter {
                "offline" | "microsoft" => {
                    matches!(
                        (filter, player.kind),
                        ("offline", AccountKind::Offline) | ("microsoft", AccountKind::Microsoft)
                    )
                }
                other => player.server.as_deref() == Some(other),
            };
            if visible && !out.contains(&player.id) {
                out.push(player.id.clone());
            }
        }
        out
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

/// 离线名字：1–16 个字符，不能带空格或控制字符；中文和符号可以用，但多数服务器不认。
pub fn normalize_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 16 {
        return Err("玩家名要 1–16 个字符。".into());
    }
    if name.chars().any(|ch| ch.is_whitespace() || ch.is_control()) {
        return Err("玩家名里不能有空格或控制字符。".into());
    }
    Ok(name.to_owned())
}

/// 手填的 UUID：接受 32 位裸十六进制或 36 位带横线的写法，统一成小写带横线。
fn parse_uuid(text: &str) -> Result<String, String> {
    let clean: String = text.chars().filter(|ch| *ch != '-').collect();
    if clean.len() != 32 || !clean.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err("UUID 要是 32 位十六进制（可带横线）。".into());
    }
    let lower = clean.to_ascii_lowercase();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &lower[0..8],
        &lower[8..12],
        &lower[12..16],
        &lower[16..20],
        &lower[20..32]
    ))
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
        crate::proxy::apply(reqwest::blocking::Client::builder())
            .timeout(Duration::from_secs(30))
            .user_agent(crate::user_agent())
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
    // `verification_uri_complete` 里已经带了 user_code，用户点开就能直接确认。
    let verification = {
        let complete = string_field(&body, "verification_uri_complete");
        if complete.is_empty() {
            string_field(&body, "verification_uri")
        } else {
            complete
        }
    };
    Ok(DeviceCode {
        device_code: string_field(&body, "device_code"),
        user_code: string_field(&body, "user_code"),
        verification_uri: verification,
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
    // 皮肤整张存下来（账号页画全身预览），头像顺带从同一份字节里裁。
    let (avatar, skin, cape) = match profile.skin.as_deref() {
        Some(url) => match fetch_image(url) {
            Ok(png) => {
                let cape = profile
                    .cape
                    .as_deref()
                    .and_then(|url| fetch_image(url).ok())
                    .map(|png| data_url(&png));
                (head_avatar_png(&png), Some(data_url(&png)), cape)
            }
            Err(_) => (None, None, None),
        },
        None => (None, None, None),
    };
    Ok(Poll::Done(Box::new(Account {
        id: String::new(),
        name: profile.name,
        uuid: profile.id.to_lowercase(),
        kind: AccountKind::Microsoft,
        avatar,
        refresh: refresh_token.as_deref().and_then(protect),
        access: Some(mc_access),
        access_expires: now_secs() + expires_in.max(60),
        server: None,
        oauth: None,
        skin,
        cape,
        skin_model: profile.skin_model,
        skin_official: None,
        cape_official: None,
    })))
}

/// 第三方账户：把 yggdrasil 的访问令牌记进内存，同时 DPAPI 保护后存进 `refresh`，
/// 这样重启启动器还能续上（yggdrasil 的 refresh 只认 accessToken）。
pub fn set_session(account: &mut Account, token: &str) {
    account.access = Some(token.to_owned());
    account.refresh = protect(token);
    // 各服务器给的有效期不一样，按 12 小时保守估计；过期了下次启动会走一次 refresh。
    account.access_expires = now_secs() + 12 * 3600;
}

/// 取第三方账户还能用的访问令牌；过期或缺失时用存下来的令牌调 `/authserver/refresh`。
pub fn session(account: &mut Account, server: &crate::authlib::Server) -> Result<String, String> {
    if let Some(token) = account.access.clone().filter(|token| !token.is_empty()) {
        if account.access_expires == 0 || account.access_expires > now_secs() + 60 {
            return Ok(token);
        }
    }
    // 外链登录的账户用 OIDC 的 refresh_token 换票，密码登录的走 yggdrasil 的 refresh。
    if let Some(refresh_token) = account.oauth.as_deref().and_then(unprotect) {
        let (token, next) = crate::authlib::refresh_oauth(server, &refresh_token)?;
        if let Some(next) = next {
            account.oauth = protect(&next);
        }
        set_session(account, &token);
        return Ok(token);
    }
    let saved = account
        .refresh
        .as_deref()
        .and_then(unprotect)
        .ok_or_else(|| "这个账户的登录状态没了，重新输入一次密码。".to_string())?;
    // 存下来的令牌很多本来就是长期有效的：先问一次 validate，过了就不用换，
    // 省掉一次 refresh（有些服务器的 refresh 会顺手作废旧令牌）。
    if crate::authlib::validate(server, &saved) {
        set_session(account, &saved);
        return Ok(saved);
    }
    let token = crate::authlib::refresh(server, &saved)?;
    set_session(account, &token);
    Ok(token)
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
        2148916227 => "这个微软账号被 Xbox Live 封禁了。",
        2148916229 => "这个微软账号受家长控制限制，去 account.microsoft.com/family 改一下权限。",
        2148916233 => "这个微软账号还没有 Xbox 档案，先去 xbox.com 建一个（免费的）。",
        2148916234 => "这个微软账号还没同意 Xbox 的服务条款，去 xbox.com 同意一次。",
        2148916235 => "这个微软账号所在的国家/地区不提供 Xbox Live。",
        2148916236 | 2148916238 | 2148916239 => {
            "这是未成年账号，需要一个成人账号把它加进 Microsoft 家庭组并授权。"
        }
        2148916237 => "这个微软账号受家长控制限制，需要家长在 family.microsoft.com 授权。",
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
    #[allow(dead_code)]
    skin_model: Option<String>,
    #[allow(dead_code)]
    cape: Option<String>,
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
    // 只用 state=ACTIVE 的那套；没激活的皮肤是「买了但没穿」。
    let active = |key: &str| -> Option<(String, Option<String>)> {
        value
            .get(key)
            .and_then(|items| items.as_array())
            .and_then(|items| {
                items.iter().find(|item| {
                    item.get("state").and_then(|state| state.as_str()) == Some("ACTIVE")
                })
            })
            .and_then(|item| item.get("url").and_then(|url| url.as_str()))
            .map(|url| {
                (
                    url.to_owned(),
                    value
                        .get(key)
                        .and_then(|items| items.as_array())
                        .and_then(|items| {
                            items.iter().find(|item| {
                                item.get("state").and_then(|state| state.as_str())
                                    == Some("ACTIVE")
                            })
                        })
                        .and_then(|item| item.pointer("/metadata/model"))
                        .and_then(|model| model.as_str())
                        .map(str::to_owned),
                )
            })
    };
    let skin = active("skins");
    let cape = active("capes").map(|(url, _)| url);
    Ok(Profile {
        id,
        name,
        skin: skin.as_ref().map(|(url, _)| url.clone()),
        skin_model: skin.and_then(|(_, model)| model),
        cape,
    })
}

/// 下载一张皮肤/披风图，返回 PNG 字节（统一重编码，省得前端分辨 gif/webp）。
pub(crate) fn fetch_image(url: &str) -> Result<Vec<u8>, String> {
    let bytes = client()
        .get(url)
        .send()
        .and_then(|response| response.bytes())
        .map_err(|error| format!("下载皮肤失败：{error}"))?;
    let image = image::load_from_memory(&bytes).map_err(|error| format!("皮肤文件看不懂：{error}"))?;
    let mut png = std::io::Cursor::new(Vec::new());
    image
        .to_rgba8()
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|error| format!("皮肤编码失败：{error}"))?;
    Ok(png.into_inner())
}

/// 本地皮肤文件 → data URL。只收 Minecraft 皮肤表的那三种尺寸。
pub fn local_skin(path: &str) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("读不到这个文件：{error}"))?;
    let image = image::load_from_memory(&bytes).map_err(|error| format!("这看起来不是图片：{error}"))?;
    let (width, height) = (image.width(), image.height());
    if !((width == 64 && (height == 32 || height == 64)) || (width == 128 && height == 128)) {
        return Err("皮肤图要 64×32、64×64 或 128×128。".into());
    }
    let mut png = std::io::Cursor::new(Vec::new());
    image
        .to_rgba8()
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|error| format!("皮肤编码失败：{error}"))?;
    Ok(data_url(&png.into_inner()))
}

/// 读一张披风贴图：原版披风是 64×32，个别老资源是 22×17，两种都收。
pub fn local_cape(path: &str) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("读不到这个文件：{error}"))?;
    let image = image::load_from_memory(&bytes).map_err(|error| format!("这看起来不是图片：{error}"))?;
    let (width, height) = (image.width(), image.height());
    if !((width == 64 && height == 32) || (width == 22 && height == 17) || (width == 64 && height == 64)) {
        return Err("披风图要是 64×32（老资源 22×17 也行）。".into());
    }
    let mut png = std::io::Cursor::new(Vec::new());
    image
        .to_rgba8()
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|error| format!("披风编码失败：{error}"))?;
    Ok(data_url(&png.into_inner()))
}

/// 把名字塞进 URL 路径：中文得转成 UTF-8 百分号编码。
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// LittleSkin 按用户名取皮肤：返回 (dataUrl, `slim` 或 `classic`)。
pub fn littleskin_skin(name: &str) -> Result<(String, String), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("先填 LittleSkin 的用户名。".into());
    }
    let url = format!("https://littleskin.cn/csl/{}.json", escape(name));
    let text = client()
        .get(&url)
        .send()
        .and_then(|response| response.text())
        .map_err(|error| format!("查不到这个皮肤：{error}"))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| "LittleSkin 返回的东西看不懂。".to_string())?;
    let skins = value["skins"].as_object();
    let (hash, model) = match skins {
        Some(skins) => {
            if let Some(slim) = skins.get("slim").and_then(|item| item.as_str()) {
                (slim.to_owned(), "slim")
            } else if let Some(fallback) = skins.get("default").and_then(|item| item.as_str()) {
                (fallback.to_owned(), "classic")
            } else if let Some(any) = skins.values().find_map(|item| item.as_str()) {
                (any.to_owned(), "classic")
            } else {
                return Err("这个玩家在 LittleSkin 上没有皮肤。".into());
            }
        }
        None => return Err("这个玩家在 LittleSkin 上没有皮肤。".into()),
    };
    let png = fetch_image(&format!("https://littleskin.cn/csl/textures/{hash}"))?;
    Ok((data_url(&png), model.to_string()))
}

/// PNG 字节 → `data:image/png;base64,…`，账号页和启动参数都直接吃这个字符串。
pub(crate) fn data_url(png: &[u8]) -> String {
    use base64::Engine;
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    )
}

/// 把皮肤裁成头像：取头部正面 8×8，再叠上帽子层，放大成 64×64。
pub(crate) fn head_avatar(url: &str) -> Result<Option<String>, String> {
    Ok(head_avatar_png(&fetch_image(url)?))
}

pub(crate) fn head_avatar_png(png: &[u8]) -> Option<String> {
    let image = image::load_from_memory(png).ok()?;
    let head = head_image(&image.to_rgba8())?;
    let mut encoded = std::io::Cursor::new(Vec::new());
    head.write_to(&mut encoded, image::ImageFormat::Png).ok()?;
    Some(data_url(encoded.get_ref()))
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
pub fn protect(plain: &str) -> Option<String> {
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

pub fn unprotect(stored: &str) -> Option<String> {
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
        // 短名、中文、符号都放行；空格、控制字符和超长才拒绝。
        assert!(normalize_name("Steve_1").is_ok());
        assert!(normalize_name("ab").is_ok());
        assert!(normalize_name("中文名").is_ok());
        assert!(normalize_name("bad name").is_err());
        assert!(normalize_name("bad	name").is_err());
        assert!(normalize_name("abcdefghijklmnopq").is_err());
    }

    #[test]
    fn custom_uuid_is_normalized_and_kept_on_rename() {
        assert_eq!(
            parse_uuid("0123456789ABCDEF0123456789ABCDEF").unwrap(),
            "01234567-89ab-cdef-0123-456789abcdef"
        );
        assert!(parse_uuid("hello").is_err());
        let mut store = Store::default();
        let id = store
            .add_offline("Steve", Some("01234567-89ab-cdef-0123-456789abcdef"))
            .unwrap();
        store.rename(&id, "Alex").unwrap();
        let player = store.players.iter().find(|item| item.id == id).unwrap();
        assert_eq!(player.name, "Alex");
        assert_eq!(player.uuid, "01234567-89ab-cdef-0123-456789abcdef");
    }

    #[test]
    fn offline_skin_can_be_set_and_cleared() {
        let mut store = Store::default();
        let id = store.add_offline("Steve", None).unwrap();
        store
            .set_offline_look(
                &id,
                Some("data:image/png;base64,AA==".into()),
                Some("slim".into()),
                Some("alex".into()),
                Some("data:image/png;base64,BB==".into()),
                Some("migrator".into()),
            )
            .unwrap();
        let player = store.players.iter().find(|item| item.id == id).unwrap();
        assert_eq!(player.skin_model.as_deref(), Some("slim"));
        assert_eq!(player.skin_official.as_deref(), Some("alex"));
        assert_eq!(player.cape_official.as_deref(), Some("migrator"));
        store.set_offline_look(&id, None, None, None, None, None).unwrap();
        let player = store.players.iter().find(|item| item.id == id).unwrap();
        assert!(player.skin.is_none());
        assert!(player.cape.is_none());
    }

    #[test]
    fn store_round_trips_and_keeps_current() {
        let dir = temp_dir("store");
        let mut store = Store::default();
        let steve = store.add_offline("Steve", None).unwrap();
        let alex = store.add_offline("Alex", None).unwrap();
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
        // 删掉当前账户 = 回到未登录，不自动选下一个。
        assert_eq!(loaded.current.as_deref(), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_offline_names_are_rejected() {
        let mut store = Store::default();
        store.add_offline("Steve", None).unwrap();
        assert!(store.add_offline("steve", None).is_err());
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
