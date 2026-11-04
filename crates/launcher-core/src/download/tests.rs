use super::*;
use serde_json::json;
use std::{collections::BTreeMap, io::Cursor, sync::atomic::AtomicUsize};

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha1::digest(bytes))
}
struct Fixture {
    root: PathBuf,
    data: BTreeMap<String, Vec<u8>>,
    calls: AtomicUsize,
    /// 接下来多少次 open 直接报网络错误（用来验证重试）。
    fail_next: AtomicUsize,
}
impl Source for Fixture {
    fn open(&self, url: &str) -> Result<Box<dyn Read>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self
            .fail_next
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return Err(DownloadError::Network("fixture offline".into()));
        }
        Ok(Box::new(Cursor::new(
            self.data
                .get(url)
                .ok_or_else(|| DownloadError::Network(format!("fixture missing {url}")))?
                .clone(),
        )))
    }
    fn open_from(&self, url: &str, offset: u64) -> Result<(Box<dyn Read>, bool)> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let body = self
            .data
            .get(url)
            .ok_or_else(|| DownloadError::Network(format!("fixture missing {url}")))?;
        let start = (offset as usize).min(body.len());
        Ok((Box::new(Cursor::new(body[start..].to_vec())), offset > 0))
    }
}
/// 每个 Fixture 用独立临时目录（`std::env::temp_dir()` 在沙箱下不一定可写，见 scripts/test-core.mjs）。
static TEMP_ID: AtomicUsize = AtomicUsize::new(0);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "ybumcl-download-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let mut f = Self {
            root,
            data: BTreeMap::new(),
            calls: AtomicUsize::new(0),
            fail_next: AtomicUsize::new(0),
        };
        let client = f.artifact("https://piston-data.mojang.com/client.jar", b"client");
        let mut lib = f.artifact("https://libraries.minecraft.net/a/b.jar", b"library");
        lib["path"] = json!("a/b.jar");
        let mut native = f.artifact("https://libraries.minecraft.net/a/native.jar", b"native");
        native["path"] = json!("a/native.jar");
        let mut log = f.artifact("https://piston-data.mojang.com/log.xml", b"log");
        log["id"] = json!("test.xml");
        let asset = b"asset";
        let asset_hash = hash(asset);
        f.data.insert(
            format!(
                "https://resources.download.minecraft.net/{}/{}",
                &asset_hash[..2],
                asset_hash
            ),
            asset.to_vec(),
        );
        let index_bytes = serde_json::to_vec(&json!({"virtual":true,"map_to_resources":true,"objects":{"sounds/test.ogg":{"hash":asset_hash,"size":asset.len()}}})).unwrap();
        let mut index = f.artifact("https://piston-meta.mojang.com/index.json", &index_bytes);
        index["id"] = json!("test");
        let metadata = json!({"id":"test","mainClass":"net.minecraft.client.main.Main","downloads":{"client":client},"assetIndex":index,"logging":{"client":{"file":log}},"libraries":[{"name":"a:b:1","downloads":{"artifact":lib,"classifiers":{"natives-windows":native}},"natives":{"windows":"natives-windows"}},{"name":"skip:linux:1","rules":[{"action":"allow","os":{"name":"linux"}}],"downloads":{"artifact":{"path":"../bad","url":"http://evil.test"}}}]});
        f.set_metadata(metadata);
        f
    }
    fn artifact(&mut self, url: &str, bytes: &[u8]) -> Value {
        self.data.insert(url.into(), bytes.to_vec());
        json!({"url":url,"sha1":hash(bytes),"size":bytes.len()})
    }
    fn metadata(&self) -> Value {
        serde_json::from_slice(&self.data["https://piston-meta.mojang.com/test.json"]).unwrap()
    }
    fn set_metadata(&mut self, v: Value) {
        let bytes = serde_json::to_vec(&v).unwrap();
        let sha1 = hash(&bytes);
        self.data
            .insert("https://piston-meta.mojang.com/test.json".into(), bytes);
        self.data.insert(MANIFEST.into(), serde_json::to_vec(&json!({"latest":{"release":"test","snapshot":"test"},"versions":[{"id":"test","type":"release","url":"https://piston-meta.mojang.com/test.json","sha1":sha1,"time":"now","releaseTime":"now"}]})).unwrap());
    }
    fn request(&self) -> InstallRequest {
        InstallRequest {
            game_dir: self.root.to_string_lossy().into(),
            version_id: "test".into(),
            fabric_loader: None,
            quilt_loader: None,
            strategy: DownloadStrategy::default(),
            threads: None,
        }
    }
    fn fabric(&mut self) -> Value {
        let url =
            "https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.16.0/fabric-loader-0.16.0.jar";
        self.data.insert(url.into(), b"fabric-loader".to_vec());
        self.data
            .insert(format!("{url}.sha1"), hash(b"fabric-loader").into_bytes());
        let profile = json!({
            "id":"fabric-loader-0.16.0-test", "inheritsFrom":"test",
            "mainClass":"net.fabricmc.loader.impl.launch.knot.KnotClient",
            "arguments":{"jvm":[],"game":[]},
            "libraries":[{"name":"net.fabricmc:fabric-loader:0.16.0","url":"https://maven.fabricmc.net/"}]
        });
        self.set_fabric_profile(profile.clone());
        profile
    }
    fn set_fabric_profile(&mut self, profile: Value) {
        self.data.insert(
            format!("{FABRIC_META}/test/0.16.0/profile/json"),
            serde_json::to_vec(&profile).unwrap(),
        );
    }
    fn fabric_request(&self) -> InstallRequest {
        InstallRequest {
            fabric_loader: Some("0.16.0".into()),
            quilt_loader: None,
            ..self.request()
        }
    }
    fn install_fabric(&self) -> Result<InstallSummary> {
        install_with(
            self,
            self.fabric_request(),
            &CancellationToken::default(),
            |_| {},
        )
    }
    fn install(&self) -> Result<InstallSummary> {
        install_with(self, self.request(), &CancellationToken::default(), |_| {})
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn large_artifact_is_fetched_with_parallel_ranges() {
    let mut f = Fixture::new();
    let size = (CHUNK_THRESHOLD + 1024 * 1024) as usize;
    let body: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    let url = "https://piston-data.mojang.com/big.jar";
    let mut artifact = f.artifact(url, &body);
    artifact["path"] = json!("big.jar");
    let a = Artifact::new(
        "big.jar".into(),
        &artifact,
        Some(url.to_string()),
        FILE_LIMIT,
    )
    .unwrap();
    let root = f.root.clone();
    let part = root.join("big.jar.part");
    let ranges = AtomicUsize::new(0);
    let cancel = CancellationToken::default();
    let source = CountingSource {
        fixture: &f,
        ranged: &ranges,
    };
    let fetched = fetch_chunked(&source, &a, &part, &cancel, &|_| {}).unwrap();
    assert!(fetched, "chunked path should engage for large artifacts");
    // 第一段从 0 开始不计入 Range 请求。
    assert!(ranges.load(Ordering::Relaxed) >= CHUNK_MIN_PARTS - 1);
    assert_eq!(fs::read(&part).unwrap(), body);
}

#[test]
fn chunked_fetch_falls_back_when_server_ignores_ranges() {
    let f = Fixture::new();
    let artifact = json!({"url":"https://piston-data.mojang.com/big.jar","sha1":hash(&vec![0u8; CHUNK_THRESHOLD as usize + 8]),"size":CHUNK_THRESHOLD as usize + 8});
    let a = Artifact::new(
        "big.jar".into(),
        &artifact,
        Some("https://piston-data.mojang.com/big.jar".into()),
        FILE_LIMIT,
    )
    .unwrap();
    let part = f.root.join("big.jar.part");
    let cancel = CancellationToken::default();
    let source = NoRangeSource { fixture: &f };
    assert!(!fetch_chunked(&source, &a, &part, &cancel, &|_| {}).unwrap());
    assert!(!part.exists());
}

struct CountingSource<'a> {
    fixture: &'a Fixture,
    ranged: &'a AtomicUsize,
}
impl Source for CountingSource<'_> {
    fn open(&self, url: &str) -> Result<Box<dyn Read>> {
        self.fixture.open(url)
    }
    fn open_from(&self, url: &str, offset: u64) -> Result<(Box<dyn Read>, bool)> {
        if offset > 0 {
            self.ranged.fetch_add(1, Ordering::Relaxed);
        }
        self.fixture.open_from(url, offset)
    }
}
struct NoRangeSource<'a> {
    fixture: &'a Fixture,
}
impl Source for NoRangeSource<'_> {
    fn open(&self, url: &str) -> Result<Box<dyn Read>> {
        self.fixture.open(url)
    }
    fn open_from(&self, url: &str, offset: u64) -> Result<(Box<dyn Read>, bool)> {
        let (reader, _) = self.fixture.open_from(url, offset)?;
        Ok((reader, false))
    }
}
#[test]
fn parallel_install_publishes_every_artifact_atomically() {
    let f = Fixture::new();
    let mut events = vec![];
    let summary = install_with_cache(
        &f,
        f.request(),
        &CancellationToken::default(),
        |p| events.push(p),
        None,
        4,
    )
    .unwrap();
    assert_eq!(summary.downloaded_files, 9);
    assert_eq!(summary.skipped_files, 0);
    let last = events.last().unwrap();
    assert_eq!(last.stage, "complete");
    assert_eq!(last.total_files, last.completed_files);
    assert_eq!(last.total_files, 9);
    assert_eq!(last.downloaded_bytes, summary.downloaded_bytes);
    assert_eq!(
        fs::read(f.root.join("versions/test/test.jar")).unwrap(),
        b"client"
    );
    assert!(f.root.join("libraries/a/native.jar").exists());
    assert!(f.root.join("assets/log_configs/test.xml").exists());
    assert!(fs::read_dir(f.root.join("versions/test"))
        .unwrap()
        .all(|p| !p.unwrap().file_name().to_string_lossy().ends_with(".part")));
    let again = install_with_cache(
        &f,
        f.request(),
        &CancellationToken::default(),
        |_| {},
        None,
        4,
    )
    .unwrap();
    assert_eq!(again.skipped_files, 9);
    assert_eq!(again.downloaded_files, 0);
}

#[test]
fn installs_windows_files_and_reuses_verified_files() {
    let f = Fixture::new();
    let mut events = vec![];
    let result = install_with(&f, f.request(), &CancellationToken::default(), |p| {
        events.push(p)
    })
    .unwrap();
    assert_eq!(result.downloaded_files, 9);
    assert_eq!(
        fs::read(f.root.join("versions/test/test.jar")).unwrap(),
        b"client"
    );
    assert_eq!(
        fs::read(f.root.join("resources/sounds/test.ogg")).unwrap(),
        b"asset"
    );
    assert_eq!(
        fs::read(f.root.join("assets/virtual/test/sounds/test.ogg")).unwrap(),
        b"asset"
    );
    assert!(f.root.join("libraries/a/native.jar").exists());
    assert!(f.root.join("assets/log_configs/test.xml").exists());
    assert_eq!(events.last().unwrap().stage, "complete");
    assert_eq!(
        events.last().unwrap().total_files,
        events.last().unwrap().completed_files
    );
    f.calls.store(0, Ordering::Relaxed);
    let reused = f.install().unwrap();
    assert_eq!(reused.skipped_files, 9);
    assert_eq!(reused.downloaded_files, 0);
    assert_eq!(f.calls.load(Ordering::Relaxed), 1); // only manifest is fetched again
    fs::write(f.root.join("versions/test/test.jar"), b"tampered").unwrap();
    assert_eq!(f.install().unwrap().downloaded_files, 1);
}
#[test]
fn tampered_payload_never_replaces_existing_file_and_cleans_temp() {
    let mut f = Fixture::new();
    f.install().unwrap();
    let target = f.root.join("versions/test/test.jar");
    fs::write(&target, b"old invalid").unwrap();
    f.data.insert(
        "https://piston-data.mojang.com/client.jar".into(),
        b"wrong!".to_vec(),
    );
    assert!(matches!(f.install(), Err(DownloadError::Integrity(_))));
    assert_eq!(fs::read(&target).unwrap(), b"old invalid");
    assert!(fs::read_dir(target.parent().unwrap()).unwrap().all(|p| !p
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".part")));
}
#[test]
fn rejects_tampered_metadata_before_parsing_or_fetching_dependencies() {
    let mut f = Fixture::new();
    f.data.insert(
        "https://piston-meta.mojang.com/test.json".into(),
        b"{}".to_vec(),
    );
    assert!(matches!(f.install(), Err(DownloadError::Integrity(_))));
    assert!(!f.root.join("versions/test/test.json").exists());
    assert_eq!(f.calls.load(Ordering::Relaxed), 2);
}
#[test]
fn rejects_paths_and_untrusted_urls_before_payload_download() {
    for path in [
        "../escape.jar",
        "C:/escape.jar",
        "a/NUL",
        "a:stream",
        "a/../bad",
        "a\\..\\bad",
    ] {
        let mut f = Fixture::new();
        let mut meta = f.metadata();
        meta["libraries"][0]["downloads"]["artifact"]["path"] = json!(path);
        f.set_metadata(meta);
        assert!(f.install().is_err(), "{path}");
        assert!(!f.root.join("versions/test/test.jar").exists());
    }
    for url in [
        "http://libraries.minecraft.net/a",
        "https://libraries.minecraft.net.evil.test/a",
        "https://evil.test/a",
        "https://user@libraries.minecraft.net/a",
        "https://libraries.minecraft.net:444/a",
        "file:///a",
    ] {
        assert!(trusted_url(url).is_err(), "{url}");
    }
    let mut f = Fixture::new();
    let mut meta = f.metadata();
    meta["downloads"]["client"]["url"] = json!("https://evil.test/download");
    f.set_metadata(meta);
    assert!(f.install().is_err());
    assert!(!f.root.join("versions/test/test.jar").exists());
}
#[test]
fn cancellation_stops_install_and_emits_terminal_event() {
    let f = Fixture::new();
    let token = CancellationToken::default();
    let mut last = String::new();
    // Serial install so "cancel on first downloading event" is deterministic.
    let result = install_with_cache(
        &f,
        f.request(),
        &token,
        |p| {
            last = p.stage.clone();
            if p.stage == "downloading" {
                token.cancel();
            }
        },
        None,
        1,
    );
    assert!(matches!(result, Err(DownloadError::Cancelled)));
    assert_eq!(last, "cancelled");
    assert!(!f.root.join("versions/test/test.jar").exists());
    assert!(!f.root.join("versions/test/test.json").exists());
    assert!(crate::scan_versions(&f.root)
        .unwrap()
        .iter()
        .all(|v| !v.supported));
    assert!(f.root.join("versions/test/.ybumcl-version.json").exists());
    assert!(fs::read_dir(f.root.join("versions/test"))
        .unwrap()
        .all(|p| !p.unwrap().file_name().to_string_lossy().ends_with(".part")));
    f.install().unwrap();
    assert!(f.root.join("versions/test/test.json").exists());
}
#[test]
fn manifest_omits_unsafe_historical_ids_without_losing_valid_versions() {
    let mut f = Fixture::new();
    let mut data: Value = serde_json::from_slice(&f.data[MANIFEST]).unwrap();
    let mut historical = data["versions"][0].clone();
    historical["id"] = json!("3D Shareware v1.34");
    data["versions"]
        .as_array_mut()
        .unwrap()
        .push(historical.clone());
    historical["id"] = json!("../escape");
    data["versions"].as_array_mut().unwrap().push(historical);
    f.data
        .insert(MANIFEST.into(), serde_json::to_vec(&data).unwrap());
    let catalog = manifest(&f, &CancellationToken::default()).unwrap();
    // 带空格的官方版本名（3D Shareware v1.34）现在允许保留，只挡目录穿越这类危险 id。
    let ids: Vec<&str> = catalog.versions.iter().map(|v| v.id.as_str()).collect();
    assert!(ids.contains(&"test"));
    assert!(ids.contains(&"3D Shareware v1.34"));
    assert!(!ids.iter().any(|id| id.contains("..")));
}
#[test]
fn fabric_catalog_validates_versions_and_supports_empty_catalog() {
    let mut f = Fixture::new();
    let url = format!("{FABRIC_META}/test");
    f.data.insert(
        url.clone(),
        br#"[{"loader":{"version":"0.16.0","stable":true}}]"#.to_vec(),
    );
    assert_eq!(
        fabric_loaders(&f, "test", &CancellationToken::default()).unwrap(),
        vec![FabricLoaderVersion {
            version: "0.16.0".into(),
            stable: true
        }]
    );
    f.data.insert(url.clone(), b"[]".to_vec());
    assert!(fabric_loaders(&f, "test", &CancellationToken::default())
        .unwrap()
        .is_empty());
    f.data.insert(
        url,
        br#"[{"loader":{"version":"../bad","stable":false}}]"#.to_vec(),
    );
    assert!(fabric_loaders(&f, "test", &CancellationToken::default()).is_err());
    assert!(fabric_loaders(&f, "../bad", &CancellationToken::default()).is_err());
    let old: InstallRequest =
        serde_json::from_value(json!({"gameDir":"test","versionId":"test"})).unwrap();
    assert!(old.fabric_loader.is_none());
}
#[test]
fn fabric_installs_inherited_profile_once_and_reuses_only_verified_libraries() {
    let mut f = Fixture::new();
    f.fabric();
    let mut events = vec![];
    let summary = install_with(&f, f.fabric_request(), &CancellationToken::default(), |p| {
        events.push(p)
    })
    .unwrap();
    assert_eq!(summary.version_id, "fabric-loader-0.16.0-test");
    assert_eq!(summary.downloaded_files, 11);
    assert!(f.root.join("versions/test/test.json").exists());
    assert!(f
        .root
        .join("versions/fabric-loader-0.16.0-test/fabric-loader-0.16.0-test.json")
        .exists());
    assert!(!f.root.join("versions/test/.ybumcl-fabric.json").exists());
    assert_eq!(events.iter().filter(|p| p.stage == "complete").count(), 1);
    let last = events.last().unwrap();
    assert_eq!(last.stage, "complete");
    assert_eq!(last.total_files, last.completed_files);
    assert_eq!(last.downloaded_bytes, summary.downloaded_bytes);
    assert_eq!(last.version_id, summary.version_id);
    f.calls.store(0, Ordering::Relaxed);
    let reused = f.install_fabric().unwrap();
    assert_eq!(reused.skipped_files, 10);
    assert_eq!(reused.downloaded_files, 1); // profile fetched fresh, checksum revalidated
    assert_eq!(f.calls.load(Ordering::Relaxed), 3); // manifest, profile, sidecar; no JAR payloads
    fs::write(
        f.root
            .join("libraries/net/fabricmc/fabric-loader/0.16.0/fabric-loader-0.16.0.jar"),
        b"tampered",
    )
    .unwrap();
    assert_eq!(f.install_fabric().unwrap().downloaded_files, 2);
}
#[test]
fn fabric_rejects_unsafe_profile_and_maven_urls_before_jar_download() {
    for (key, value) in [
        ("id", json!("../escape")),
        ("id", json!("test")),
        ("inheritsFrom", json!("another")),
        ("mainClass", json!("evil.Main")),
    ] {
        let mut f = Fixture::new();
        let mut profile = f.fabric();
        profile[key] = value;
        f.set_fabric_profile(profile);
        assert!(f.install_fabric().is_err());
        assert!(!f
            .root
            .join("versions/fabric-loader-0.16.0-test/fabric-loader-0.16.0-test.json")
            .exists());
        assert!(!f.root.join("libraries/net/fabricmc").exists());
    }
    for url in [
        "https://evil.test/",
        "https://piston-data.mojang.com/",
        "https://maven.fabricmc.net.evil.test/",
        "http://maven.fabricmc.net/",
        "https://user@maven.fabricmc.net/",
        "https://maven.fabricmc.net/?q=x",
    ] {
        let mut f = Fixture::new();
        let mut profile = f.fabric();
        profile["libraries"][0]["url"] = json!(url);
        f.set_fabric_profile(profile);
        assert!(f.install_fabric().is_err(), "{url}");
        assert!(!f.root.join("libraries/net/fabricmc").exists());
    }
}
#[test]
fn fabric_cancellation_and_integrity_failure_leave_profile_unpublished() {
    let mut f = Fixture::new();
    f.fabric();
    let token = CancellationToken::default();
    let mut events = vec![];
    let result = install_with(&f, f.fabric_request(), &token, |p| {
        if p.stage == "fabric-libraries" {
            token.cancel();
        }
        events.push(p);
    });
    assert!(matches!(result, Err(DownloadError::Cancelled)));
    assert_eq!(events.last().unwrap().stage, "cancelled");
    assert!(!events.iter().any(|p| p.stage == "complete"));
    let target = f
        .root
        .join("versions/fabric-loader-0.16.0-test/fabric-loader-0.16.0-test.json");
    assert!(!target.exists());
    let jar =
        "https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.16.0/fabric-loader-0.16.0.jar";
    f.data.insert(jar.into(), b"bad".to_vec());
    assert!(matches!(
        f.install_fabric(),
        Err(DownloadError::Integrity(_))
    ));
    assert!(!target.exists());
    f.data.insert(jar.into(), b"fabric-loader".to_vec());
    f.data
        .insert(format!("{jar}.sha1"), b"invalid checksum".to_vec());
    assert!(matches!(f.install_fabric(), Err(DownloadError::Invalid(_))));
    assert!(!target.exists());
    f.data
        .insert(format!("{jar}.sha1"), hash(b"fabric-loader").into_bytes());
    f.install_fabric().unwrap();
    let previous = fs::read(&target).unwrap();
    fs::write(
        f.root
            .join("libraries/net/fabricmc/fabric-loader/0.16.0/fabric-loader-0.16.0.jar"),
        b"tampered",
    )
    .unwrap();
    f.data.remove(jar);
    assert!(matches!(f.install_fabric(), Err(DownloadError::Network(_))));
    assert_eq!(fs::read(&target).unwrap(), previous);
}
#[test]
fn fabric_partial_library_failure_keeps_profile_staged_and_reuses_completed_jar() {
    let mut f = Fixture::new();
    let mut profile = f.fabric();
    profile["libraries"].as_array_mut().unwrap().push(json!({
        "name":"org.example:helper:1", "url":"https://libraries.minecraft.net/", "sha1":hash(b"helper")
    }));
    f.set_fabric_profile(profile);
    let helper = "https://libraries.minecraft.net/org/example/helper/1/helper-1.jar";
    let target = f
        .root
        .join("versions/fabric-loader-0.16.0-test/fabric-loader-0.16.0-test.json");
    assert!(matches!(f.install_fabric(), Err(DownloadError::Network(_))));
    assert!(!target.exists());
    assert!(f
        .root
        .join("libraries/net/fabricmc/fabric-loader/0.16.0/fabric-loader-0.16.0.jar")
        .exists());
    f.data.insert(helper.into(), b"helper".to_vec());
    let completed = f.install_fabric().unwrap();
    assert_eq!(completed.skipped_files, 10);
    assert_eq!(completed.downloaded_files, 2);
    assert!(target.exists());
}
#[test]
fn fabric_rejects_maven_coordinate_traversal_and_oversized_checksums() {
    for coordinate in [
        "../evil:lib:1",
        "a:b:../1",
        "a:b%2fescape:1",
        "a:b?query:1",
        "a:b:1:../../native",
    ] {
        let mut f = Fixture::new();
        let mut profile = f.fabric();
        profile["libraries"][0]["name"] = json!(coordinate);
        f.set_fabric_profile(profile);
        assert!(f.install_fabric().is_err(), "{coordinate}");
        assert!(!f.root.join("libraries/net/fabricmc").exists());
    }
    let mut f = Fixture::new();
    f.fabric();
    f.data.insert("https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.16.0/fabric-loader-0.16.0.jar.sha1".into(), vec![b'a'; 1025]);
    assert!(matches!(f.install_fabric(), Err(DownloadError::Invalid(_))));
    assert!(!f.root.join("libraries/net/fabricmc").exists());
}
#[test]
fn verified_cache_copies_only_requested_artifacts_without_network_or_aliasing() {
    let cache = Fixture::new();
    cache.install().unwrap();
    fs::write(cache.root.join("unreferenced.jar"), b"do not import").unwrap();
    let mut destination = Fixture::new();
    // A manifest remains necessary to authenticate the cached metadata. Every
    // other URL is deliberately absent: success proves no payload was fetched.
    destination.data.retain(|url, _| url == MANIFEST);
    let result = install_with_cache(
        &destination,
        destination.request(),
        &CancellationToken::default(),
        |_| {},
        Some(&cache.root),
        1,
    )
    .unwrap();
    assert_eq!(result.skipped_files, 9);
    assert_eq!(result.downloaded_files, 0);
    assert_eq!(result.downloaded_bytes, 0);
    assert_eq!(destination.calls.load(Ordering::Relaxed), 1);
    assert!(!destination.root.join("unreferenced.jar").exists());
    assert!(destination.root.join("versions/test/test.json").exists());
    // Writes in either tree cannot mutate the other (in particular no hardlinks).
    fs::write(
        destination.root.join("versions/test/test.jar"),
        b"destination edit",
    )
    .unwrap();
    assert_eq!(
        fs::read(cache.root.join("versions/test/test.jar")).unwrap(),
        b"client"
    );
    fs::write(cache.root.join("libraries/a/b.jar"), b"source edit").unwrap();
    assert_eq!(
        fs::read(destination.root.join("libraries/a/b.jar")).unwrap(),
        b"library"
    );
}

#[test]
fn corrupt_cache_falls_back_to_network_and_is_never_repaired_in_place() {
    let cache = Fixture::new();
    cache.install().unwrap();
    // Same length exercises hash checks, not just the expected size check.
    fs::write(cache.root.join("versions/test/test.jar"), b"broken").unwrap();
    let destination = Fixture::new();
    let result = install_with_cache(
        &destination,
        destination.request(),
        &CancellationToken::default(),
        |_| {},
        Some(&cache.root),
        1,
    )
    .unwrap();
    assert_eq!(result.skipped_files, 8);
    assert_eq!(result.downloaded_files, 1);
    assert_eq!(result.downloaded_bytes, 6);
    assert_eq!(destination.calls.load(Ordering::Relaxed), 2);
    assert_eq!(
        fs::read(destination.root.join("versions/test/test.jar")).unwrap(),
        b"client"
    );
    assert_eq!(
        fs::read(cache.root.join("versions/test/test.jar")).unwrap(),
        b"broken"
    );
}

#[cfg(any(windows, unix))]
#[test]
fn cache_linked_directories_fall_back_to_network() {
    let cache = Fixture::new();
    cache.install().unwrap();
    let linked = Fixture::new();
    #[cfg(unix)]
    std::os::unix::fs::symlink(cache.root.join("libraries"), linked.root.join("libraries"))
        .unwrap();
    #[cfg(windows)]
    {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(linked.root.join("libraries"))
            .arg(cache.root.join("libraries"))
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "{}",
            String::from_utf8_lossy(&status.stderr)
        );
    }
    let destination = Fixture::new();
    let result = install_with_cache(
        &destination,
        destination.request(),
        &CancellationToken::default(),
        |_| {},
        Some(&linked.root),
        1,
    )
    .unwrap();
    assert_eq!(result.skipped_files, 0);
    assert_eq!(result.downloaded_files, 9);
    // Removing only the link protects the fixture cache from recursive cleanup.
    fs::remove_dir(linked.root.join("libraries")).unwrap_or_else(|_| {
        fs::remove_file(linked.root.join("libraries")).unwrap();
    });
    assert_eq!(
        fs::read(cache.root.join("libraries/a/b.jar")).unwrap(),
        b"library"
    );
}

#[cfg(unix)]
#[test]
fn rejects_symlink_escape() {
    let f = Fixture::new();
    let outside = Fixture::new();
    std::os::unix::fs::symlink(&outside.root, f.root.join("versions")).unwrap();
    assert!(f.install().is_err());
    assert!(!outside.root.join("test/test.json").exists());
}

#[test]
fn network_blip_is_retried_until_the_file_arrives() {
    let f = Fixture::new();
    f.fail_next.store(2, Ordering::Relaxed);
    f.install().unwrap();
    assert_eq!(
        fs::read(f.root.join("versions/test/test.jar")).unwrap(),
        b"client"
    );
}

#[test]
fn partial_file_resumes_without_refetching_finished_bytes() {
    let f = Fixture::new();
    let part = f.root.join("versions/test/test.jar.part");
    fs::create_dir_all(part.parent().unwrap()).unwrap();
    fs::write(&part, b"cli").unwrap();
    f.install().unwrap();
    assert_eq!(
        fs::read(f.root.join("versions/test/test.jar")).unwrap(),
        b"client"
    );
    assert!(!part.exists());
}

#[test]
fn stale_part_larger_than_payload_is_discarded() {
    let f = Fixture::new();
    let part = f.root.join("versions/test/test.jar.part");
    fs::create_dir_all(part.parent().unwrap()).unwrap();
    fs::write(&part, vec![0u8; 4096]).unwrap();
    f.install().unwrap();
    assert_eq!(
        fs::read(f.root.join("versions/test/test.jar")).unwrap(),
        b"client"
    );
}

fn central_jar() -> Url {
    Url::parse("https://repo.maven.apache.org/maven2/org/ow2/asm/asm/9.10.1/asm-9.10.1.jar")
        .unwrap()
}

#[test]
fn central_coordinates_offer_an_aliyun_mirror_beside_the_official_route() {
    let routes = route_candidates(&central_jar());
    assert_eq!(routes[0].0, Route::Aliyun);
    assert_eq!(
        routes[0].1.as_str(),
        "https://maven.aliyun.com/repository/public/org/ow2/asm/asm/9.10.1/asm-9.10.1.jar"
    );
    assert_eq!(routes.last().unwrap().0, Route::Official);
}

#[test]
fn checksums_and_unknown_hosts_stay_on_the_official_route() {
    let checksum = Url::parse(
        "https://repo.maven.apache.org/maven2/org/ow2/asm/asm/9.10.1/asm-9.10.1.jar.sha1",
    )
    .unwrap();
    assert_eq!(route_candidates(&checksum).len(), 1);
    assert_eq!(route_candidates(&checksum)[0].0, Route::Official);
    let manifest =
        Url::parse("https://piston-meta.mojang.com/mc/game/version_manifest_v2.json").unwrap();
    assert_eq!(route_candidates(&manifest).len(), 1);
}

#[test]
fn minecraft_libraries_keep_the_bmclapi_route_ahead_of_official() {
    let url =
        Url::parse("https://libraries.minecraft.net/com/mojang/authlib/1.5.25/authlib-1.5.25.jar")
            .unwrap();
    let routes = route_candidates(&url);
    assert_eq!(routes[0].0, Route::Bmclapi);
    assert_eq!(
        routes[0].1.as_str(),
        "https://bmclapi2.bangbang93.com/maven/com/mojang/authlib/1.5.25/authlib-1.5.25.jar"
    );
    assert_eq!(routes.last().unwrap().0, Route::Official);
}

#[test]
fn measured_speed_decides_which_route_is_tried_first() {
    let http = Http::new(DownloadStrategy::Race).unwrap();
    http.rank.lock().unwrap().insert(
        "central",
        Ranked {
            speeds: vec![(Route::Official, 9_000_000.0), (Route::Aliyun, 1_000.0)],
            at: Instant::now(),
        },
    );
    let ordered = http.candidates(central_jar().as_str()).unwrap();
    assert_eq!(
        ordered[0].host_str(),
        Some("repo.maven.apache.org"),
        "官方实测更快时应排在前面"
    );
}

#[test]
fn a_slow_route_is_demoted_so_the_next_attempt_uses_another() {
    let http = Http::new(DownloadStrategy::Race).unwrap();
    http.rank.lock().unwrap().insert(
        "central",
        Ranked {
            speeds: vec![(Route::Aliyun, 8_000_000.0), (Route::Official, 100_000.0)],
            at: Instant::now(),
        },
    );
    assert_eq!(
        http.candidates(central_jar().as_str()).unwrap()[0].host_str(),
        Some("maven.aliyun.com")
    );
    http.note_slow(central_jar().as_str());
    assert_eq!(
        http.candidates(central_jar().as_str()).unwrap()[0].host_str(),
        Some("repo.maven.apache.org")
    );
}

#[test]
fn version_client_jars_get_a_bmclapi_route_keyed_by_the_version_id() {
    let jar = Url::parse(
        "https://piston-data.mojang.com/v1/objects/901a4f8fcb6b0f0e0d0a5cbb1b1c4c6f4c0f5ad8/client.jar",
    )
    .unwrap();
    // 没有版本号时拼不出镜像地址：/v1/objects/<sha>/client.jar 里的 sha 不是版本号。
    assert!(Http::new(DownloadStrategy::Race).unwrap().version_mirror(&jar).is_none());
    let http = Http::new(DownloadStrategy::Race).unwrap().with_version("1.20.5");
    assert_eq!(
        http.version_mirror(&jar).unwrap().as_str(),
        "https://bmclapi2.bangbang93.com/version/1.20.5/client"
    );
    let server = Url::parse(
        "https://piston-data.mojang.com/v1/objects/901a4f8fcb6b0f0e0d0a5cbb1b1c4c6f4c0f5ad8/server.jar",
    )
    .unwrap();
    assert_eq!(
        http.version_mirror(&server).unwrap().as_str(),
        "https://bmclapi2.bangbang93.com/version/1.20.5/server"
    );
    // 其它主机、其它文件都不该冒出镜像地址。
    assert!(http
        .version_mirror(
            &Url::parse("https://piston-meta.mojang.com/mc/game/version_manifest_v2.json").unwrap()
        )
        .is_none());
    let library = Url::parse("https://libraries.minecraft.net/com/mojang/authlib/1.5.25/authlib-1.5.25.jar")
        .unwrap();
    assert!(http.version_mirror(&library).is_none());
}

#[test]
fn mirror_only_keeps_the_version_mirror_and_refuses_the_official_jar() {
    let jar = "https://piston-data.mojang.com/v1/objects/901a4f8fcb6b0f0e0d0a5cbb1b1c4c6f4c0f5ad8/client.jar";
    let http = Http::new(DownloadStrategy::MirrorOnly)
        .unwrap()
        .with_version("1.20.5");
    let routed = http.routed_candidates(jar).unwrap();
    assert_eq!(
        routed.iter().map(|(route, _)| *route).collect::<Vec<_>>(),
        vec![Route::Bmclapi],
        "只有镜像源时官方 jar 也不该被留下"
    );
    assert_eq!(
        routed[0].1.as_str(),
        "https://bmclapi2.bangbang93.com/version/1.20.5/client"
    );
    let official = Http::new(DownloadStrategy::OfficialOnly)
        .unwrap()
        .with_version("1.20.5");
    let routed = official.routed_candidates(jar).unwrap();
    assert_eq!(
        routed.iter().map(|(route, _)| *route).collect::<Vec<_>>(),
        vec![Route::Official],
        "只有官方源时不该出现镜像"
    );
}

#[test]
fn cleanroom_libraries_prefer_the_nexus_mirror() {
    let url = "https://maven.cleanroommc.com/com/cleanroommc/cleanmix/0.9.0/cleanmix-0.9.0.jar";
    let routes = route_candidates(&Url::parse(url).unwrap());
    let hosts: Vec<_> = routes
        .iter()
        .map(|(route, url)| (*route, url.host_str().unwrap_or_default()))
        .collect();
    assert_eq!(
        hosts,
        vec![
            (Route::Nexus, "nexus.jsdu.cn"),
            (Route::Official, "maven.cleanroommc.com"),
        ]
    );
    // nexus 是扁平的 /repository/ 布局，官方路径直接接在后面，别无脑加 /releases。
    assert_eq!(
        routes[0].1.as_str(),
        "https://nexus.jsdu.cn/repository/com/cleanroommc/cleanmix/0.9.0/cleanmix-0.9.0.jar"
    );
    assert_eq!(family(&routes[0].1), "cleanroom");
    assert_eq!(family(&routes[1].1), "cleanroom");
}

#[test]
fn cleanroom_checksums_stay_on_the_official_host() {
    let url =
        "https://maven.cleanroommc.com/com/cleanroommc/cleanmix/0.9.0/cleanmix-0.9.0.jar.sha1";
    let routes = route_candidates(&Url::parse(url).unwrap());
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].0, Route::Official);
}

fn strategy_http(strategy: DownloadStrategy) -> Http {
    Http::new(strategy).unwrap()
}

fn library_jar() -> &'static str {
    "https://libraries.minecraft.net/com/mojang/brigadier/1.0.18/brigadier-1.0.18.jar"
}

#[test]
fn official_only_strategy_drops_every_mirror_route() {
    let http = strategy_http(DownloadStrategy::OfficialOnly);
    let routes = http.routed_candidates(library_jar()).unwrap();
    assert_eq!(routes.len(), 1, "只走官方时不该留镜像：{routes:?}");
    assert_eq!(routes[0].0, Route::Official);
}

#[test]
fn mirror_only_strategy_drops_the_official_route() {
    let http = strategy_http(DownloadStrategy::MirrorOnly);
    let routes = http.routed_candidates(library_jar()).unwrap();
    assert!(!routes.is_empty());
    assert!(
        routes.iter().all(|(route, _)| *route != Route::Official),
        "只走镜像时不该留官方：{routes:?}"
    );
    assert!(routes
        .iter()
        .any(|(route, _)| *route == Route::Bmclapi));
}

#[test]
fn mirror_only_strategy_falls_back_when_a_file_has_no_mirror() {
    let http = strategy_http(DownloadStrategy::MirrorOnly);
    // 原版清单没有镜像路由：即便选了「仅镜像」也只能走官方，而不是报错。
    let url = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
    let routes = http.routed_candidates(url).unwrap();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].0, Route::Official);
    assert_eq!(routes[0].1.as_str(), url);
}

#[test]
fn first_place_strategies_only_reorder_the_same_candidates() {
    let mirror_first = strategy_http(DownloadStrategy::MirrorFirst)
        .routed_candidates(library_jar())
        .unwrap();
    let official_first = strategy_http(DownloadStrategy::OfficialFirst)
        .routed_candidates(library_jar())
        .unwrap();
    assert_eq!(mirror_first.first().unwrap().0, Route::Bmclapi);
    assert_eq!(official_first.first().unwrap().0, Route::Official);
    assert_eq!(mirror_first.len(), official_first.len());
}
