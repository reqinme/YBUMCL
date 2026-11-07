use super::*;
use serde_json::json;
use std::{
    io::Write,
    sync::atomic::{AtomicU64, Ordering},
};
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "ybumcl-core-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let this = Self { root };
        this.file("versions/test/test.jar", b"fixture");
        this.file("java.exe", b"fixture");
        this.file("assets/indexes/test.json", br#"{"objects":{}}"#);
        this.metadata(json!({"id":"test","type":"release","mainClass":"net.minecraft.client.main.Main","libraries":[],"assetIndex":{"id":"test"},"downloads":{"client":{"url":"https://piston-data.mojang.com/v1/objects/fixture/client.jar"}},"javaVersion":{"majorVersion":17},"arguments":{"jvm":["-Djava.library.path=${natives_directory}","-cp","${classpath}"],"game":["--username","${auth_player_name}","--accessToken","${auth_access_token}","--uuid","${auth_uuid}","--gameDir","${game_directory}"]}}));
        this
    }
    fn file(&self, path: &str, bytes: &[u8]) {
        let p = self.root.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }
    fn metadata(&self, v: Value) {
        self.file("versions/test/test.json", &serde_json::to_vec(&v).unwrap());
    }
    fn version(&self, id: &str, v: Value) {
        self.file(
            &format!("versions/{id}/{id}.json"),
            &serde_json::to_vec(&v).unwrap(),
        );
    }
    fn value(&self) -> Value {
        read_json(&self.root.join("versions/test/test.json")).unwrap()
    }
    fn request(&self) -> LaunchRequest {
        LaunchRequest {
            game_dir: self.root.to_string_lossy().into(),
            game_directory: None,
            version_id: "test".into(),
            java_path: self.root.join("java.exe").to_string_lossy().into(),
            username: "Player_1".into(),
            max_memory_mb: 2048,
            access_token: None,
            uuid: None,
            ..LaunchRequest::default()
        }
    }
    fn plan(&self) -> Result<LaunchPlan> {
        prepare(self.request(), |_| Ok(21))
    }
    fn zip(&self, name: &str, entry: &str, bytes: &[u8]) {
        let path = self.root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        zip.start_file(entry, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(bytes).unwrap();
        zip.finish().unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn log4j_flag_is_always_added_and_direct_memory_is_dropped() {
    let f = Fixture::new();
    let mut v = f.value();
    v["arguments"]["jvm"] = json!([
        "-XX:MaxDirectMemorySize=256M",
        "-Djava.library.path=${natives_directory}",
        "-cp",
        "${classpath}"
    ]);
    f.metadata(v);
    let plan = f.plan().unwrap();
    assert!(plan
        .args
        .iter()
        .any(|arg| arg == "-Dlog4j2.formatMsgNoLookups=true"));
    assert!(plan
        .args
        .iter()
        .all(|arg| arg != "-XX:MaxDirectMemorySize=256M"));
}
#[test]
fn encoding_flags_follow_the_selected_java() {
    let f = Fixture::new();
    let mut v = f.value();
    v["javaVersion"] = json!({"majorVersion": 8});
    f.metadata(v);
    let eight = prepare(f.request(), |_| Ok(8)).unwrap();
    assert!(!eight
        .args
        .iter()
        .any(|arg| arg.starts_with("-Dstdout.encoding=")));
    assert!(!eight
        .args
        .iter()
        .any(|arg| arg.starts_with("-Dfile.encoding=")));

    let seventeen = prepare(f.request(), |_| Ok(17)).unwrap();
    assert!(seventeen
        .args
        .iter()
        .any(|arg| arg == "-Dstdout.encoding=UTF-8"));
    assert!(!seventeen
        .args
        .iter()
        .any(|arg| arg.starts_with("-Dfile.encoding=")));

    let twenty_one = prepare(f.request(), |_| Ok(21)).unwrap();
    assert!(twenty_one
        .args
        .iter()
        .any(|arg| arg == "-Dfile.encoding=COMPAT"));
}

#[cfg(windows)]
#[test]
fn java_launch_paths_strip_only_verbatim_prefix() {
    assert_eq!(
        java_launch_path(Path::new(r"\\?\C:\Games\Minecraft")),
        r"C:\Games\Minecraft"
    );
    assert_eq!(
        java_launch_path(Path::new(r"\\?\UNC\server\share\Minecraft")),
        r"\\server\share\Minecraft"
    );
    let f = Fixture::new();
    let mut v = f.value();
    v["arguments"]["game"] = json!([
        "--gameDir",
        "${game_directory}",
        "--assetsDir",
        "${assets_root}",
        "${game_assets}"
    ]);
    v["logging"] =
        json!({"client":{"file":{"id":"test.xml"},"argument":"-Dlog4j.configurationFile=${path}"}});
    f.file("assets/log_configs/test.xml", b"fixture");
    f.metadata(v);
    let plan = f.plan().unwrap();
    assert!(!plan.java_path.contains(r"\\?\"));
    assert!(!plan.working_directory.contains(r"\\?\"));
    assert!(
        plan.args.iter().all(|arg| !arg.contains(r"\\?\")),
        "{:?}",
        plan.args
    );
}
#[test]
fn windows_rules_obey_order_and_features() {
    assert!(rules_allow(None).unwrap());
    assert!(!rules_allow(Some(&json!([]))).unwrap());
    assert!(rules_allow(Some(
        &json!([{"action":"allow"},{"action":"disallow","os":{"name":"linux"}}])
    ))
    .unwrap());
    assert!(!rules_allow(Some(
        &json!([{"action":"allow"},{"action":"disallow","os":{"name":"windows"}}])
    ))
    .unwrap());
    assert!(!rules_allow(Some(
        &json!([{"action":"allow","features":{"is_demo_user":true}}])
    ))
    .unwrap());
    assert!(rules_allow(Some(
        &json!([{"action":"allow","features":{"is_demo_user":false}}])
    ))
    .unwrap());
    assert!(!rules_allow(Some(
        &json!([{"action":"allow","os":{"name":"windows","arch":"unrecognized"}}])
    ))
    .unwrap());
    assert!(rules_allow(Some(&json!([{"action":"oops"}]))).is_err());
    assert!(rules_allow(Some(&json!([{"action":"allow","os":{"version":"["}}]))).is_err());
}
#[test]
fn legacy_quotes_preserve_grouping_and_windows_paths() {
    assert_eq!(
        split_legacy(r#"--name "Hello World" --dir 'C:\My Games' --empty "" --quote "a\"b""#)
            .unwrap(),
        [
            "--name",
            "Hello World",
            "--dir",
            r"C:\My Games",
            "--empty",
            "",
            "--quote",
            "a\"b"
        ]
    );
    assert!(split_legacy("'oops").is_err());
}
#[test]
fn offline_uuid_matches_java_known_vector() {
    assert_eq!(offline_uuid("Notch"), "b50ad385829d3141a2167e7d7539ba7f");
}
#[test]
fn plan_expands_tokens_and_serializes_camel_case() {
    let f = Fixture::new();
    let plan = f.plan().unwrap();
    assert!(plan.args.contains(&"-Xmx2048M".into()));
    assert!(plan.args.contains(&"Player_1".into()));
    assert!(plan.args.contains(&java_launch_path(
        &f.root
            .join("versions/test/test.jar")
            .canonicalize()
            .unwrap()
    )));
    assert!(!plan.args.iter().any(|a| a.contains("${")));
    assert_eq!(plan.warnings.len(), 1);
    let serialized = serde_json::to_value(&plan).unwrap();
    assert!(serialized.get("workingDirectory").is_some());
    assert!(serialized.get("javaPath").is_some());
}
#[test]
fn isolated_launch_uses_runtime_directory_and_shared_installation() {
    let f = Fixture::new();
    f.file("libraries/org/example/shared/1/shared-1.jar", b"shared");
    let mut v = f.value();
    v["libraries"] = json!([{"name":"org.example:shared:1"}]);
    v["arguments"]["game"] = json!([
        "--gameDir",
        "${game_directory}",
        "--assetsDir",
        "${assets_root}",
        "--legacyAssets",
        "${game_assets}"
    ]);
    f.metadata(v);
    let mut request = f.request();
    request.game_directory = Some("instances/profile-1".into());
    let plan = prepare(request, |_| Ok(21)).unwrap();
    let root = f.root.canonicalize().unwrap();
    let runtime = root.join("instances/profile-1");
    assert!(runtime.is_dir());
    assert_eq!(plan.working_directory, java_launch_path(&runtime));
    for (flag, expected) in [
        ("--gameDir", runtime),
        ("--assetsDir", root.join("assets")),
        ("--legacyAssets", root.join("assets")),
    ] {
        let position = plan.args.iter().position(|a| a == flag).unwrap();
        assert_eq!(plan.args[position + 1], java_launch_path(&expected));
    }
    let classpath_position = plan.args.iter().position(|a| a == "-cp").unwrap();
    let classpath = &plan.args[classpath_position + 1];
    assert!(classpath.contains(&java_launch_path(
        &root.join("libraries/org/example/shared/1/shared-1.jar")
    )));
    assert!(classpath.contains(&java_launch_path(&root.join("versions/test/test.jar"))));
    assert!(!classpath.contains("instances"));
}

#[test]
fn instance_launch_options_land_in_the_plan() {
    let f = Fixture::new();
    let mut request = f.request();
    request.max_memory_mb = 4096;
    request.initial_memory_mb = 2048;
    // 用户自己填的 JVM 参数：整段替换版本自带的那些，但启动器该补的 -cp 还得在。
    request.jvm_args = vec!["-Dtest.head=1".into(), "-cp".into(), "evil".into()];
    request.game_args = vec!["--width".into(), "1280".into()];
    let plan = prepare(request, |_| Ok(21)).unwrap();
    assert!(plan.args.iter().any(|a| a == "-Dtest.head=1"));
    assert!(!plan.args.iter().any(|a| a == "evil"));
    assert_eq!(plan.args.iter().filter(|a| *a == "-cp").count(), 1);
    assert!(plan.args.iter().any(|a| a == "-Xmx4096M"));
    assert!(plan.args.iter().any(|a| a == "-Xms2048M"));
    let width = plan.args.iter().position(|a| a == "--width").unwrap();
    assert_eq!(plan.args[width + 1], "1280");
}

#[test]
fn old_launch_requests_default_to_shared_root() {
    let f = Fixture::new();
    let mut value = serde_json::to_value(f.request()).unwrap();
    value.as_object_mut().unwrap().remove("gameDirectory");
    let request: LaunchRequest = serde_json::from_value(value.clone()).unwrap();
    assert!(request.game_directory.is_none());
    let plan = prepare(request, |_| Ok(21)).unwrap();
    assert_eq!(
        plan.working_directory,
        java_launch_path(&f.root.canonicalize().unwrap())
    );
    assert!(!f.root.join("instances").exists());
    value["gameDirectory"] = json!("instances/profile-1");
    let request: LaunchRequest = serde_json::from_value(value).unwrap();
    assert_eq!(
        request.game_directory.as_deref(),
        Some("instances/profile-1")
    );
}

#[test]
fn isolated_runtime_rejects_arbitrary_or_unsafe_targets() {
    let f = Fixture::new();
    let root = f.root.canonicalize().unwrap();
    for target in [
        "",
        ".",
        "..",
        "instances",
        "instances/",
        "instances/../versions",
        "instances/..",
        "instances/CON",
        "instances/profile.",
        "instances/profile ",
        "instances/a:stream",
        "instances/profile/child",
        "instances//profile",
        "versions",
        "versions/",
        "versions/..",
        "versions/../instances",
        "versions/CON",
        "/instances/test",
        "C:/instances/test",
        "../instances/test",
    ] {
        let mut request = f.request();
        request.game_directory = Some(target.into());
        assert!(
            matches!(
                prepare(request, |_| Ok(21)),
                Err(CoreError::InvalidInput(_))
            ),
            "{target}"
        );
    }
    assert!(runtime_directory(
        &root,
        Some(&f.root.join("instances/test").to_string_lossy())
    )
    .is_err());
    // 版本即实例：`versions/<safe-id>` 同样是受管路径（版本自带的存档/模组目录）。
    let version_runtime = runtime_directory(&root, Some("versions/test")).unwrap();
    assert!(version_runtime.ends_with(std::path::Path::new("versions").join("test")));
    assert!(version_runtime.is_dir());
    assert!(!root.join("instances").exists());
    f.file("instances/file", b"not a directory");
    assert!(runtime_directory(&root, Some("instances/file")).is_err());
    // 解析结果统一是「去 verbatim 前缀」的普通路径。
    assert_eq!(
        runtime_directory(&root, Some(r"instances\valid-id")).unwrap(),
        crate::plain(root.join("instances/valid-id"))
    );
}

#[test]
fn isolated_legacy_resources_are_materialized_without_sharing_writes() {
    let f = Fixture::new();
    let hash = "0123456789012345678901234567890123456789";
    f.file(&format!("assets/objects/01/{hash}"), b"shared asset");
    f.file("assets/virtual/test/sounds/test.ogg", b"shared asset");
    f.file(
        "assets/indexes/test.json",
        &serde_json::to_vec(&json!({
            "virtual":true, "map_to_resources":true,
            "objects":{"sounds/test.ogg":{"hash":hash}}
        }))
        .unwrap(),
    );
    let mut v = f.value();
    v["arguments"]["game"] = json!([
        "--gameDir",
        "${game_directory}",
        "--assetsDir",
        "${game_assets}"
    ]);
    f.metadata(v);
    let mut request = f.request();
    request.game_directory = Some("instances/legacy".into());
    let plan = prepare(request.clone(), |_| Ok(21)).unwrap();
    let resource = f.root.join("instances/legacy/resources/sounds/test.ogg");
    assert_eq!(fs::read(&resource).unwrap(), b"shared asset");
    assert!(plan.args.contains(&java_launch_path(
        &f.root.canonicalize().unwrap().join("assets/virtual/test")
    )));
    fs::write(&resource, b"local change").unwrap();
    assert!(prepare(request, |_| Ok(21)).is_ok());
    assert_eq!(fs::read(&resource).unwrap(), b"local change");
    assert_eq!(
        fs::read(f.root.join(format!("assets/objects/01/{hash}"))).unwrap(),
        b"shared asset"
    );
    assert!(!f.root.join("resources").exists());
}

#[cfg(any(unix, windows))]
#[test]
fn isolated_runtime_rejects_redirected_instance_directories() {
    let f = Fixture::new();
    let root = f.root.canonicalize().unwrap();
    fs::create_dir(root.join("instances")).unwrap();
    for (name, destination) in [
        ("outside", std::env::temp_dir()),
        ("shared", root.join("assets")),
    ] {
        let link = root.join("instances").join(name);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&destination, &link).unwrap();
        #[cfg(windows)]
        if let Err(error) = std::os::windows::fs::symlink_dir(&destination, &link) {
            // Creating symlinks requires Developer Mode or elevated rights on Windows.
            if error.raw_os_error() == Some(1314) {
                return;
            }
            panic!("{error}");
        }
        assert!(runtime_directory(&root, Some(&format!("instances/{name}"))).is_err());
        fs::remove_dir(&link)
            .or_else(|_| fs::remove_file(&link))
            .unwrap();
    }
    fs::remove_dir(root.join("instances")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("assets"), root.join("instances")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(root.join("assets"), root.join("instances")).unwrap();
    assert!(runtime_directory(&root, Some("instances/should-not-exist")).is_err());
    assert!(!root.join("assets/should-not-exist").exists());
    fs::remove_dir(root.join("instances"))
        .or_else(|_| fs::remove_file(root.join("instances")))
        .unwrap();
}

#[test]
fn missing_dependencies_are_actionable() {
    let f = Fixture::new();
    let mut v = f.value();
    v["libraries"] = json!([{"name":"org.example:missing:1"}]);
    f.metadata(v);
    let error = f.plan().unwrap_err().to_string();
    assert!(error.contains("missing-1.jar"));
    assert!(error.contains("Install or repair"));
}
#[test]
fn rules_skip_non_windows_libraries() {
    let f = Fixture::new();
    let mut v = f.value();
    v["libraries"] = json!([{"name":"org.example:missing:1","rules":[{"action":"allow","os":{"name":"linux"}}]}]);
    f.metadata(v);
    assert!(f.plan().is_ok());
}
#[test]
fn rejects_missing_parent_and_custom_main() {
    let f = Fixture::new();
    let mut v = f.value();
    v["inheritsFrom"] = json!("base");
    f.metadata(v);
    assert!(f.plan().unwrap_err().to_string().contains("base.json"));
    let mut v = f.value();
    v.as_object_mut().unwrap().remove("inheritsFrom");
    v["mainClass"] = json!("com.evil.Main");
    f.metadata(v);
    assert!(f.plan().is_err());
    let scans = scan_versions(&f.root).unwrap();
    assert!(!scans[0].supported);
}
#[test]
fn rejects_traversal_and_windows_special_paths() {
    for path in [
        "../evil",
        "a/../../evil",
        "C:/evil",
        "/evil",
        r"a\..\evil",
        "a:stream",
        "CON.dll",
        "a/NUL",
        "a./b",
    ] {
        assert!(safe_relative(path).is_err(), "{path}");
    }
    let f = Fixture::new();
    let mut request = f.request();
    request.version_id = "../test".into();
    assert!(prepare(request, |_| Ok(21)).is_err());
    let mut v = f.value();
    v["libraries"] = json!([{"name":"a:b:1","downloads":{"artifact":{"path":"../outside.jar"}}}]);
    f.metadata(v);
    assert!(f.plan().is_err());
}
#[test]
fn rejects_invalid_inputs_and_java_version() {
    let f = Fixture::new();
    let mut request = f.request();
    request.username = "bad name".into();
    assert!(prepare(request, |_| Ok(21)).is_err());
    let mut request = f.request();
    request.max_memory_mb = 0;
    assert!(prepare(request, |_| Ok(21)).is_err());
    assert!(prepare(f.request(), |_| Ok(8))
        .unwrap_err()
        .to_string()
        .contains("requires Java 17"));
}
#[test]
fn authenticated_tokens_redacted_and_identity_validated() {
    let f = Fixture::new();
    let mut request = f.request();
    request.access_token = Some("secret".into());
    assert!(prepare(request.clone(), |_| Ok(21)).is_err());
    request.uuid = Some("b50ad385-829d-3141-a216-7e7d7539ba7f".into());
    let plan = prepare(request, |_| Ok(21)).unwrap();
    assert!(plan.args.contains(&"secret".into()));
    assert!(!plan.redacted_args().contains(&"secret".into()));
    assert!(plan.redacted_args().contains(&"[REDACTED]".into()));
}
#[test]
fn natives_extract_safely_and_exclude_metadata() {
    let f = Fixture::new();
    f.zip("libraries/native.jar", "x64/test.dll", b"native");
    let mut v = f.value();
    v["libraries"] = json!([{"name":"a:b:1","natives":{"windows":"natives-windows"},"downloads":{"classifiers":{"natives-windows":{"path":"native.jar"}}}}]);
    f.metadata(v);
    f.plan().unwrap();
    assert_eq!(
        fs::read(f.root.join("versions/test/launcher-natives/x64/test.dll")).unwrap(),
        b"native"
    );
}
#[test]
fn native_zip_traversal_is_rejected() {
    let f = Fixture::new();
    f.zip("libraries/native.jar", "../escape.dll", b"bad");
    let root = fs::canonicalize(&f.root).unwrap();
    assert!(extract_natives(
        &root,
        &root.join("versions/test/natives"),
        &[(root.join("libraries/native.jar"), vec![])]
    )
    .is_err());
    assert!(!root.join("versions/test/escape.dll").exists());
}
#[test]
fn jvm_cannot_override_entrypoint_or_classpath() {
    let f = Fixture::new();
    let mut v = f.value();
    v["arguments"]["jvm"] = json!(["-jar", "evil.jar"]);
    f.metadata(v);
    assert!(f.plan().is_err());
    let mut v = f.value();
    v["arguments"]["jvm"] = json!(["-cp", "evil.jar"]);
    f.metadata(v);
    let plan = f.plan().unwrap();
    assert!(!plan.args.contains(&"evil.jar".into()));
}
#[test]
fn legacy_launchwrapper_profiles_may_carry_their_own_libraries() {
    // LiteLoader / OptiFine 的 profile 就是 launchwrapper + 自己的库，
    // 校验不能把它们当成可疑元数据拒绝掉。
    let f = Fixture::new();
    let mut v = f.value();
    v["mainClass"] = json!("net.minecraft.launchwrapper.Launch");
    v["libraries"] = json!([
        {"name": "com.mumfrey:liteloader:1.12.2-SNAPSHOT"},
        {"name": "net.minecraft:launchwrapper:1.12"}
    ]);
    assert_eq!(unsupported(&v, "1.12.2-LiteLoader"), None);
    let mut impostor = f.value();
    impostor["libraries"] = json!([{"name": "com.mumfrey:liteloader:1.12.2-SNAPSHOT"}]);
    assert!(unsupported(&impostor, "1.12.2").is_some());
}
#[test]
fn loader_system_properties_are_accepted_but_class_loading_is_not() {
    let f = Fixture::new();
    let mut v = f.value();
    v["mainClass"] = json!("cpw.mods.bootstraplauncher.BootstrapLauncher");
    v["arguments"]["jvm"] = json!([
        "-Dfml.pluginLayerLibraries=",
        "-DignoreList=a.jar,b.jar",
        "-DmergeModules=jna-5.10.0.jar",
        "-DlibraryDirectory=${library_directory}",
        "-Djava.library.path=${natives_directory}",
        "-cp",
        "${classpath}"
    ]);
    f.metadata(v);
    let plan = f.plan().unwrap();
    assert!(plan.args.contains(&"-Dfml.pluginLayerLibraries=".into()));
    assert!(plan.args.contains(&"-DmergeModules=jna-5.10.0.jar".into()));
    let mut evil = f.value();
    evil["arguments"]["jvm"] = json!([
        "-Djava.system.class.loader=evil.Loader",
        "-cp",
        "${classpath}"
    ]);
    f.metadata(evil);
    assert!(f.plan().is_err());
}
#[test]
fn verifies_asset_objects_and_legacy_virtual_files() {
    let f = Fixture::new();
    f.file(
        "assets/indexes/test.json",
        br#"{"objects":{"sounds/test.ogg":{"hash":"0123456789012345678901234567890123456789"}}}"#,
    );
    assert!(f
        .plan()
        .unwrap_err()
        .to_string()
        .contains("0123456789012345678901234567890123456789"));
}
#[test]
fn vanilla_26_3_jvm_options_preserve_native_subdirectory() {
    let f = Fixture::new();
    let mut v = f.value();
    v["arguments"]["jvm"] = json!([
        "-XX:HeapDumpPath=MojangTricksIntelDriversForPerformance_javaw.exe_minecraft.exe.heapdump",
        "-XX:StackShadowPages=32",
        "--enable-native-access=ALL-UNNAMED",
        "--add-exports",
        "java.base/jdk.internal.misc=ALL-UNNAMED",
        "-Djava.library.path=${natives_directory}/java",
        "-Djna.tmpdir=${natives_directory}/jna",
        "-Dorg.lwjgl.system.SharedLibraryExtractPath=${natives_directory}/lwjgl",
        "-Dio.netty.native.workdir=${natives_directory}/netty",
        "-Dminecraft.launcher.brand=${launcher_name}",
        "-Dminecraft.launcher.version=${launcher_version}",
        "-cp",
        "${classpath}"
    ]);
    v["arguments"]["default-user-jvm"] = json!([{"value":["-XX:+UseCompactObjectHeaders"]}]);
    f.metadata(v);
    let plan = f.plan().unwrap();
    assert!(plan.args.contains(&"-XX:StackShadowPages=32".into()));
    assert!(plan
        .args
        .contains(&"java.base/jdk.internal.misc=ALL-UNNAMED".into()));
    assert!(plan
        .args
        .iter()
        .any(|a| a.starts_with("-Djava.library.path=") && a.ends_with("/java")));
    assert!(!plan.args.contains(&"-XX:+UseCompactObjectHeaders".into()));
}

#[test]
fn numeric_stack_pages_and_module_exports_are_narrowly_validated() {
    let f = Fixture::new();
    for value in [
        "",
        "0",
        "513",
        "-1",
        "32 -jar evil.jar",
        "999999999999",
        "+32",
    ] {
        let mut v = f.value();
        v["arguments"]["jvm"] = json!([format!("-XX:StackShadowPages={value}")]);
        f.metadata(v);
        assert!(f.plan().is_err(), "{value}");
    }
    for args in [
        json!(["--add-exports"]),
        json!(["--add-exports", "java.base/java.lang=ALL-UNNAMED"]),
        json!(["--enable-native-access=evil"]),
        json!(["-Djava.library.path=${natives_directory}/../outside"]),
    ] {
        let mut v = f.value();
        v["arguments"]["jvm"] = args;
        f.metadata(v);
        assert!(f.plan().is_err());
    }
}

#[test]
fn fabric_profiles_inherit_metadata_arguments_and_ancestor_jar() {
    for main in [
        "net.fabricmc.loader.impl.launch.knot.KnotClient",
        "net.fabricmc.loader.launch.knot.KnotClient",
    ] {
        let f = Fixture::new();
        let mut base = f.value();
        base["id"] = json!("base");
        base["arguments"]["game"] = json!([
            "--username",
            "${auth_player_name}",
            "--version",
            "${version_name}"
        ]);
        base["libraries"] = json!([{"name":"org.example:shared:1"}, {"name":"org.example:shared:1:natives-windows"}]);
        f.version("base", base);
        f.file("versions/base/base.jar", b"ancestor client");
        f.file("libraries/org/example/shared/2/shared-2.jar", b"override");
        f.file(
            "libraries/org/example/shared/1/shared-1-natives-windows.jar",
            b"classifier",
        );
        f.file(
            "libraries/net/fabricmc/fabric-loader/0.16/fabric-loader-0.16.jar",
            b"loader",
        );
        f.version("middle", json!({"id":"middle","inheritsFrom":"base","arguments":{"game":["--middle"],"jvm":["-Dfile.encoding=UTF-8"]}}));
        f.metadata(json!({"id":"test","inheritsFrom":"middle","mainClass":main,"libraries":[{"name":"org.example:shared:2"},{"name":"net.fabricmc:fabric-loader:0.16"}],"arguments":{"game":["--child"],"jvm":["-Djava.net.preferIPv4Stack=true", "-DFabricMcEmu= net.minecraft.client.main.Main "]}}));
        // A selected loader profile does not need its own client JAR.
        fs::remove_file(f.root.join("versions/test/test.jar")).unwrap();
        let plan = f.plan().unwrap();
        let cp = &plan.args[plan.args.iter().position(|arg| arg == "-cp").unwrap() + 1];
        assert!(cp.ends_with("base.jar"), "{cp}");
        assert!(cp.contains("shared-2.jar"));
        assert!(!cp.contains("shared-1.jar"));
        assert!(cp.contains("shared-1-natives-windows.jar"));
        let entry = plan.args.iter().position(|arg| arg == main).unwrap();
        assert_eq!(
            &plan.args[entry + 1..],
            [
                "--username",
                "Player_1",
                "--version",
                "test",
                "--middle",
                "--child"
            ]
        );
        let parent_jvm = plan
            .args
            .iter()
            .position(|arg| arg == "-Dfile.encoding=UTF-8")
            .unwrap();
        let child_jvm = plan
            .args
            .iter()
            .position(|arg| arg == "-Djava.net.preferIPv4Stack=true")
            .unwrap();
        assert!(parent_jvm < child_jvm && child_jvm < entry);
        assert!(plan
            .args
            .contains(&"-DFabricMcEmu= net.minecraft.client.main.Main ".into()));
        assert!(plan
            .args
            .iter()
            .any(|arg| arg.starts_with("-Djava.library.path=")
                && (arg.ends_with("test\\launcher-natives")
                    || arg.ends_with("test/launcher-natives"))));
        let scan = scan_versions(&f.root)
            .unwrap()
            .into_iter()
            .find(|v| v.id == "test")
            .unwrap();
        assert!(scan.supported);
        assert_eq!(scan.main_class.as_deref(), Some(main));
        assert_eq!(scan.kind, "release");
        assert!(prepare(f.request(), |_| Ok(8))
            .unwrap_err()
            .to_string()
            .contains("requires Java 17"));
    }
}

#[test]
fn legacy_parent_arguments_precede_modern_fabric_arguments() {
    let f = Fixture::new();
    let mut base = f.value();
    base["id"] = json!("base");
    base.as_object_mut().unwrap().remove("arguments");
    base["minecraftArguments"] = json!("--username ${auth_player_name} --label 'two words'");
    f.version("base", base);
    f.file("versions/base/base.jar", b"client");
    f.metadata(json!({"id":"test","inheritsFrom":"base","mainClass":"net.fabricmc.loader.launch.knot.KnotClient","arguments":{"game":["--fabric"]}}));
    let plan = f.plan().unwrap();
    assert_eq!(
        &plan.args[plan.args.len() - 5..],
        ["--username", "Player_1", "--label", "two words", "--fabric"]
    );
}

#[test]
fn inheritance_rejects_cycles_depth_unsafe_ids_and_id_mismatch() {
    let f = Fixture::new();
    for parent in [json!("../outside"), json!("CON"), json!(42), Value::Null] {
        f.metadata(json!({"id":"test","inheritsFrom":parent}));
        assert!(matches!(f.plan(), Err(CoreError::Metadata(_))));
    }
    f.metadata(json!({"id":"test","inheritsFrom":"test"}));
    assert!(f.plan().unwrap_err().to_string().contains("cycle"));
    f.version("base", json!({"id":"base","inheritsFrom":"test"}));
    f.metadata(json!({"id":"test","inheritsFrom":"base"}));
    assert!(f.plan().unwrap_err().to_string().contains("cycle"));
    f.version("base", json!({"id":"wrong"}));
    assert!(f.plan().unwrap_err().to_string().contains("does not match"));
    for i in 0..32 {
        f.version(
            &format!("depth{i}"),
            json!({"id":format!("depth{i}"),"inheritsFrom":format!("depth{}", i + 1)}),
        );
    }
    f.metadata(json!({"id":"test","inheritsFrom":"depth0"}));
    assert!(f.plan().unwrap_err().to_string().contains("exceeds 32"));
    let scan = scan_versions(&f.root)
        .unwrap()
        .into_iter()
        .find(|v| v.id == "test")
        .unwrap();
    assert!(!scan.supported);
    assert!(scan.reason.unwrap().contains("exceeds 32"));
}

#[test]
fn inheritance_keeps_loader_and_metadata_validation_narrow() {
    let f = Fixture::new();
    let mut base = f.value();
    base["id"] = json!("base");
    f.version("base", base.clone());
    f.file("versions/base/base.jar", b"client");
    for invalid in [
        json!({"libraries":[{"name":"net.minecraftforge:forge:1"}]}),
        json!({"libraries":[{"name":"org.quiltmc:quilt-loader:1"}]}),
        json!({"jar":"base"}),
        json!({"arguments":{"jvm":"invalid"}}),
        json!({"libraries":{}}),
        json!({"libraries":[{"name":"invalid"}]}),
        json!({"arguments":{"jvm":["-javaagent:evil.jar"]}}),
        json!({"arguments":{"jvm":["-DFabricMcEmu=com.evil.Main"]}}),
        json!({"libraries":[{"name":"a:b:1","downloads":{"artifact":{"path":"../outside.jar"}}}]}),
    ] {
        let mut child = json!({"id":"test","inheritsFrom":"base","mainClass":"net.fabricmc.loader.impl.launch.knot.KnotClient"});
        child
            .as_object_mut()
            .unwrap()
            .extend(invalid.as_object().unwrap().clone());
        f.metadata(child);
        assert!(f.plan().is_err(), "{invalid}");
    }
    // Unsupported ancestor options must not disappear behind a valid child override.
    base["mainClass"] = json!("com.evil.Main");
    f.version("base", base);
    f.metadata(json!({"id":"test","inheritsFrom":"base","mainClass":"net.fabricmc.loader.impl.launch.knot.KnotClient"}));
    assert!(f.plan().unwrap_err().to_string().contains("com.evil.Main"));
}

#[test]
fn modern_forge_bootstrap_profile_is_supported() {
    let f = Fixture::new();
    let mut base = f.value();
    base["id"] = json!("1.21.1");
    f.version("1.21.1", base);
    f.file("versions/1.21.1/1.21.1.jar", b"client");
    f.file(
        "libraries/net/minecraftforge/bootstrap/2.1.8/bootstrap-2.1.8.jar",
        b"bootstrap",
    );
    let mut v = f.value();
    v["inheritsFrom"] = json!("1.21.1");
    v["mainClass"] = json!("net.minecraftforge.bootstrap.ForgeBootstrap");
    v["libraries"] = json!([{"name":"net.minecraftforge:bootstrap:2.1.8"}]);
    v["arguments"]["jvm"] = json!([
        "--add-opens",
        "java.base/java.lang=ALL-UNNAMED",
        "--add-exports",
        "java.base/jdk.internal.misc=ALL-UNNAMED",
        "-DlegacyClassPath=${classpath}",
        "-DlibraryDirectory=${library_directory}",
        "-Djava.net.preferIPv6Addresses=system",
        "-cp",
        "${classpath}"
    ]);
    v["arguments"]["game"] = json!(["--launchTarget", "forge_client"]);
    f.metadata(v);
    let plan = f.plan().unwrap();
    assert!(plan
        .args
        .contains(&"net.minecraftforge.bootstrap.ForgeBootstrap".into()));
    assert!(plan.args.contains(&"--launchTarget".into()));
    assert!(plan
        .args
        .iter()
        .any(|a| a.starts_with("-DlegacyClassPath=")));
    assert!(plan
        .args
        .iter()
        .any(|a| a.starts_with("-DlibraryDirectory=")));
}

#[test]
fn modern_loader_rejects_arbitrary_module_open() {
    let f = Fixture::new();
    let mut base = f.value();
    base["id"] = json!("1.21.1");
    f.version("1.21.1", base);
    f.file("versions/1.21.1/1.21.1.jar", b"client");
    f.file(
        "libraries/net/minecraftforge/bootstrap/2.1.8/bootstrap-2.1.8.jar",
        b"bootstrap",
    );
    let mut v = f.value();
    v["inheritsFrom"] = json!("1.21.1");
    v["mainClass"] = json!("net.minecraftforge.bootstrap.ForgeBootstrap");
    v["arguments"]["jvm"] = json!(["--add-opens", "java.desktop/sun.awt=ALL-UNNAMED"]);
    f.metadata(v);
    assert!(f.plan().is_err());
}

#[test]
fn neoforge_module_paths_are_declared_bounded() {
    let f = Fixture::new();
    let mut base = f.value();
    base["id"] = json!("1.21.1");
    f.version("1.21.1", base);
    f.file("versions/1.21.1/1.21.1.jar", b"client");
    f.file(
        "libraries/cpw/mods/bootstraplauncher/2.0.2/bootstraplauncher-2.0.2.jar",
        b"bootstrap",
    );
    let profile = json!({"id":"test","inheritsFrom":"1.21.1",
        "mainClass":"cpw.mods.bootstraplauncher.BootstrapLauncher",
        "libraries":[{"name":"cpw.mods:bootstraplauncher:2.0.2"}],
        "arguments":{"game":["--launchTarget","neoforgeclient","--fml.mcVersion","1.21.1"],
        "jvm":["-p","${library_directory}/cpw/mods/bootstraplauncher/2.0.2/bootstraplauncher-2.0.2.jar",
            "--add-modules","ALL-MODULE-PATH","--add-opens","java.base/java.util.jar=cpw.mods.securejarhandler",
            "--add-exports","java.base/sun.security.util=cpw.mods.securejarhandler",
            "-DignoreList=bootstraplauncher-2.0.2.jar","-DlibraryDirectory=${library_directory}","-DlegacyClassPath=${classpath}"]}});
    f.metadata(profile.clone());
    let plan = f.plan().unwrap();
    assert!(plan.args.contains(&"neoforgeclient".into()));
    assert!(!plan.args.iter().any(|s| s.contains("${")));
    for args in [
        json!(["-p", "${library_directory}/../evil.jar"]),
        json!(["-p", "C:/evil.jar"]),
        json!(["-p", "${library_directory}/undeclared.jar"]),
        json!(["-p"]),
        json!(["--add-opens", "--add-modules=ALL-MODULE-PATH"]),
        json!(["-DlibraryDirectory=C:/evil"]),
        json!(["-DlegacyClassPath=evil.jar"]),
        json!(["-javaagent:evil.jar"]),
        json!(["--patch-module=java.base=evil.jar"]),
        json!(["-jar", "evil.jar"]),
    ] {
        let mut v = profile.clone();
        v["arguments"]["jvm"] = args.clone();
        f.metadata(v);
        assert!(f.plan().is_err(), "accepted {args}");
    }
    let mut older = profile;
    older["inheritsFrom"] = json!("testbase");
    let base = json!({"id":"testbase","mainClass":"net.minecraft.client.main.Main","libraries":[],"assetIndex":{"id":"test"},"javaVersion":{"majorVersion":17}});
    f.version("testbase", base);
    f.file("versions/testbase/testbase.jar", b"client");
    f.metadata(older);
    let plan = f.plan().unwrap();
    assert!(plan.args.contains(&"neoforgeclient".into()));
}

#[test]
fn modern_array_arguments_apply_rules() {
    assert_eq!(argument_values(&json!(["first",{"rules":[{"action":"allow","os":{"name":"windows"}}],"value":["--one","two words"]},{"rules":[{"action":"allow","features":{"has_custom_resolution":true}}],"value":"skip"}])).unwrap(),["first","--one","two words"]);
}

// ------------------------------------------------------------------ Java 需求

#[test]
fn vanilla_java_baseline_follows_the_game_version() {
    use crate::java::{requirement, JavaRequirement};
    let plain = json!({"id":"x","mainClass":"net.minecraft.client.main.Main","libraries":[]});
    assert_eq!(requirement("1.12.2", &plain).range.min, 8);
    assert_eq!(requirement("1.17.1", &plain).range.min, 16);
    assert_eq!(requirement("1.18.2", &plain).range.min, 17);
    assert_eq!(requirement("1.21.1", &plain).range.min, 21);
    assert_eq!(requirement("1.20.4", &plain).range.min, 17);
    assert_eq!(requirement("26.3", &plain).range.min, 21);
    assert!(requirement("1.21.1", &plain).accepts(25));
    assert!(!requirement("1.21.1", &plain).accepts(17));

    let declared = json!({"id":"x","mainClass":"net.minecraft.client.main.Main","libraries":[],"javaVersion":{"majorVersion":25}});
    let JavaRequirement { range, .. } = requirement("26.3", &declared);
    assert_eq!(range.min, 25);
    assert!(!JavaRequirement {
        range,
        majors: None
    }
    .accepts(21));
}

#[test]
fn legacy_forge_caps_the_java_version() {
    use crate::java::{requirement_with, LoaderPresence};
    let forge = |game: &str, version: &'static str| {
        requirement_with(
            game,
            None,
            &LoaderPresence {
                forge: Some(version),
                ..Default::default()
            },
        )
    };
    let old = forge("1.7.10", "10.13.4.1614");
    assert!(old.range.contains(7));
    // 1.7.10 的 Forge 版本也能跑在 Java 8 上（安装器自己常写 javaVersion 8）。
    assert!(old.range.contains(8));
    assert!(!old.range.contains(11));
    let twelve = forge("1.12.2", "14.23.5.2860");
    assert!(twelve.range.contains(8));
    assert!(!twelve.range.contains(11));
    let thirteen = forge("1.13.2", "25.0.219");
    assert!(thirteen.range.contains(8));
    assert!(thirteen.range.contains(10));
    assert!(!thirteen.range.contains(17));
    let modern = forge("1.21.1", "1");
    assert!(modern.range.contains(21));
}

#[test]
fn loader_specific_java_rules() {
    use crate::java::{requirement_with, LoaderPresence};
    let optifine = requirement_with(
        "1.8.9",
        None,
        &LoaderPresence {
            optifine: true,
            ..Default::default()
        },
    );
    assert!(optifine.range.contains(8));
    assert!(
        !optifine.range.contains(17),
        "OptiFine on 1.8 must be Java 8"
    );

    let liteloader = requirement_with(
        "1.12.2",
        None,
        &LoaderPresence {
            liteloader: true,
            ..Default::default()
        },
    );
    assert!(liteloader.range.contains(8));
    assert!(!liteloader.range.contains(17));

    let cleanroom = requirement_with(
        "1.12.2",
        None,
        &LoaderPresence {
            cleanroom: Some("0.4.1"),
            ..Default::default()
        },
    );
    assert!(cleanroom.range.contains(21));
    let cleanroom = requirement_with(
        "1.12.2",
        None,
        &LoaderPresence {
            cleanroom: Some("0.5.0"),
            ..Default::default()
        },
    );
    assert!(cleanroom.range.contains(25));
}

#[test]
fn compatible_java_majors_wins_over_the_range() {
    use crate::java::requirement;
    let value = json!({
        "id":"pack","mainClass":"net.minecraft.client.main.Main","libraries":[],
        "compatibleJavaMajors":[17,19]
    });
    let requirement = requirement("1.12.2", &value);
    assert!(requirement.accepts(17));
    assert!(requirement.accepts(19));
    assert!(!requirement.accepts(8));
    assert!(!requirement.accepts(21));
}

#[test]
fn missing_java_message_names_the_version_and_a_download() {
    use crate::java::{requirement_with, LoaderPresence};
    let requirement = requirement_with(
        "1.21.1",
        None,
        &LoaderPresence {
            forge: Some("1"),
            ..Default::default()
        },
    );
    let message = requirement.missing_message(&[8, 17]);
    assert!(message.contains("Java 21"), "{message}");
    assert!(message.contains("adoptium.net"), "{message}");
    assert_eq!(requirement.describe(), "Java 21+");
}

#[test]
fn java_requirement_survives_a_non_canonical_game_dir() {
    // 用户把 .minecraft 指到别处（junction / 符号链接）时，传进来的路径常常
    // 不是 canonical 的；inside() 是拿 canonical 路径跟 root 比，不先
    // canonicalize 就一律判越界，调用方退回「任意 Java」，自动挑 Java 会挑错。
    let f = Fixture::new();
    let indirect = f.root.join("versions").join("..");
    let requirement = crate::java::requirement_for_dir(&indirect, "test")
        .expect("non-canonical game directory must still resolve");
    assert!(requirement.accepts(17));
}

#[test]
fn merge_appends_arrays_and_overrides_scalars() {
    let mut base = json!({"libraries": [{"name": "a:b:1"}], "arguments": {"game": ["--a"]}});
    let part = json!({"libraries": [{"name": "c:d:2"}], "arguments": {"game": ["--b"]}, "mainClass": "Loader"});
    crate::merge::merge_into(&mut base, &part);
    let libraries = base["libraries"].as_array().unwrap();
    assert_eq!(libraries.len(), 2);
    let game = base["arguments"]["game"].as_array().unwrap();
    assert_eq!(game.len(), 2);
    assert_eq!(base["mainClass"], "Loader");
}

#[test]
fn merged_arguments_keep_one_copy_of_each_option() {
    let first = json!({"minecraftArguments": "--tweakClass com.mumfrey.lites"});
    let second = json!({"minecraftArguments": "--tweakClass com.mumfrey.lites --username ${auth_player_name}"});
    let merged = crate::merge::merged_arguments(&[&first, &second]).unwrap();
    assert_eq!(merged.matches("--tweakClass").count(), 1);
    assert!(merged.contains("--username ${auth_player_name}"));
}

#[test]
fn lwjgl2_libraries_are_dropped_when_lwjgl3_is_present() {
    let mut version = json!({"libraries": [
        {"name": "org.lwjgl.lwjgl:lwjgl:2.9.4"},
        {"name": "org.lwjgl:lwjgl:3.3.1"},
        {"name": "other:thing:1"}
    ]});
    crate::merge::drop_conflicting_lwjgl2(&mut version);
    let names: Vec<&str> = version["libraries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|library| library["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["org.lwjgl:lwjgl:3.3.1", "other:thing:1"]);
}

#[test]
fn instance_names_follow_pcl_order() {
    let parts = vec![
        crate::merge::LoaderPart {
            kind: crate::merge::LoaderKind::LiteLoader,
            version: String::new(),
            version_id: "1.12.2-LiteLoader".into(),
        },
        crate::merge::LoaderPart {
            kind: crate::merge::LoaderKind::Forge,
            version: "47.4.9".into(),
            version_id: "1.20.1-forge-47.4.9".into(),
        },
        crate::merge::LoaderPart {
            kind: crate::merge::LoaderKind::Fabric,
            version: "0.20.1".into(),
            version_id: "fabric-loader-0.20.1-1.20.1".into(),
        },
    ];
    assert_eq!(
        crate::merge::instance_name("1.20.1", &parts),
        "1.20.1-Fabric_0.20.1-Forge_47.4.9-LiteLoader"
    );
}

#[test]
fn merged_version_drops_inheritance_and_renames_the_instance() {
    let base =
        json!({"id": "1.20.1", "mainClass": "net.minecraft.client.main.Main", "libraries": []});
    let loader = json!({"id": "1.20.1-forge-47.4.9", "inheritsFrom": "1.20.1", "jar": "1.20.1", "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher"});
    let merged = crate::merge::merged_version(&base, &[&loader], "1.20.1-Forge_47.4.9");
    assert_eq!(merged["id"], "1.20.1-Forge_47.4.9");
    assert!(merged.get("inheritsFrom").is_none());
    assert!(merged.get("jar").is_none());
    assert_eq!(
        merged["mainClass"],
        "cpw.mods.bootstraplauncher.BootstrapLauncher"
    );
}

#[test]
fn incomplete_instances_stay_hidden_from_the_version_list() {
    let f = Fixture::new();
    f.version(
        "1.20.1",
        json!({"id": "1.20.1", "type": "release", "mainClass": "net.minecraft.client.main.Main"}),
    );
    f.file("versions/1.20.1/.pclignore", b"installing");
    let list = scan_versions(&f.root).unwrap();
    assert!(list.iter().all(|version| version.id != "1.20.1"));
    fs::remove_file(f.root.join("versions/1.20.1/.pclignore")).unwrap();
    let list = scan_versions(&f.root).unwrap();
    assert!(list.iter().any(|version| version.id == "1.20.1"));
}

#[test]
fn legacy_incremental_profiles_merge_into_one_instance() {
    let fixture = Fixture::new();
    let vanilla = fixture.value();
    fixture.version(
        "forge-shell",
        json!({
            "id": "forge-shell",
            "inheritsFrom": "test",
            "mainClass": "net.minecraft.launchwrapper.Launch",
            "libraries": [{"name": "net.minecraft:launchwrapper:1.12"}],
            "minecraftArguments": "--tweakClass demo.Tweaker"
        }),
    );
    assert!(crate::merge::migrate_legacy_version(&fixture.root, "forge-shell").unwrap());
    let merged = crate::merge::read_version(&fixture.root, "forge-shell").unwrap();
    assert!(merged.get("inheritsFrom").is_none());
    assert_eq!(merged["mainClass"], "net.minecraft.launchwrapper.Launch");
    assert_eq!(merged["libraries"].as_array().unwrap().len(), 1);
    assert!(merged["minecraftArguments"]
        .as_str()
        .unwrap()
        .contains("--tweakClass demo.Tweaker"));
    // 原版目录保留，用户还能单独玩原版。
    assert!(fixture.root.join("versions/test/test.json").is_file());
    assert!(fixture
        .root
        .join("versions/forge-shell/forge-shell.jar")
        .is_file());
    // 再跑一次是空操作。
    assert!(!crate::merge::migrate_legacy_version(&fixture.root, "forge-shell").unwrap());
    let _ = vanilla;
}

#[test]
fn legacy_loader_directories_get_pcl_names() {
    let fixture = Fixture::new();
    fixture.version(
        "neoforge-21.1.99",
        json!({"id": "neoforge-21.1.99", "inheritsFrom": "test", "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher"}),
    );
    let name = |id: &str| crate::merge::pcl_name(&fixture.root, id);
    assert_eq!(
        name("fabric-loader-0.20.1-1.20.1").as_deref(),
        Some("1.20.1-Fabric_0.20.1")
    );
    assert_eq!(
        name("quilt-loader-0.20.0-beta.9-1.20.1").as_deref(),
        Some("1.20.1-Quilt_0.20.0-beta.9")
    );
    assert_eq!(
        name("forge-1.20.1-47.4.9").as_deref(),
        Some("1.20.1-Forge_47.4.9")
    );
    assert_eq!(
        name("neoforge-21.1.99").as_deref(),
        Some("test-NeoForge_21.1.99")
    );
    assert_eq!(name("test"), None);
    assert_eq!(name("1.20.1-Forge_47.4.9"), None);
}

#[test]
fn renaming_an_instance_moves_its_directory_and_json_id() {
    let fixture = Fixture::new();
    fixture.version(
        "fabric-loader-0.20.1-1.20.1",
        json!({"id": "fabric-loader-0.20.1-1.20.1", "inheritsFrom": "test", "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient"}),
    );
    fixture.file(
        "versions/fabric-loader-0.20.1-1.20.1/launcher-natives/a.dll",
        b"native",
    );
    crate::merge::rename_instance(
        &fixture.root,
        "fabric-loader-0.20.1-1.20.1",
        "1.20.1-Fabric_0.20.1",
    )
    .unwrap();
    let renamed = crate::merge::read_version(&fixture.root, "1.20.1-Fabric_0.20.1").unwrap();
    assert_eq!(renamed["id"], "1.20.1-Fabric_0.20.1");
    assert!(fixture
        .root
        .join("versions/1.20.1-Fabric_0.20.1/launcher-natives/a.dll")
        .is_file());
    assert!(!fixture
        .root
        .join("versions/fabric-loader-0.20.1-1.20.1")
        .exists());
    // 重名不能被覆盖。
    assert!(crate::merge::rename_instance(&fixture.root, "1.20.1-Fabric_0.20.1", "test").is_err());
}

#[test]
fn optifine_as_mod_stays_inside_the_instance() {
    let root = std::path::Path::new("C:/game");
    assert_eq!(
        crate::instance::optifine_mods_dir(
            root,
            "1.20.1-Forge_47.4.9-OptiFine_HD_U_I6",
            "1.20.1",
            false
        ),
        std::path::PathBuf::from("C:/game/versions/1.20.1-Forge_47.4.9-OptiFine_HD_U_I6/mods")
    );
    assert_eq!(
        crate::instance::optifine_mods_dir(
            root,
            "1.12.2-LiteLoader-OptiFine_HD_U_G5",
            "1.12.2",
            true
        ),
        std::path::PathBuf::from("C:/game/versions/1.12.2-LiteLoader-OptiFine_HD_U_G5/mods/1.12.2")
    );
}

#[test]
fn recommended_forge_build_moves_to_the_front() {
    use crate::loader_catalog::LoaderEntry;
    let mut entries: Vec<LoaderEntry> = ["47.4.28", "47.4.0", "47.3.0"]
        .iter()
        .map(|version| LoaderEntry {
            version: (*version).to_owned(),
            game: "1.20.1".into(),
            stable: true,
            forge: None,
        })
        .collect();
    assert!(crate::loader_catalog::prefer_recommended(
        &mut entries,
        "47.4.0"
    ));
    assert_eq!(entries[0].version, "47.4.0");
    assert!(entries[0].stable);
    assert!(!entries[1].stable);
    assert!(!crate::loader_catalog::prefer_recommended(
        &mut entries,
        "9.9.9"
    ));
}

#[test]
fn stamped_instances_remember_their_base_version_once() {
    use crate::merge::stamp_meta;

    let fixture = Fixture::new();
    let id = "1.20.1-forge-47.4.28";
    let dir = fixture.root.join("versions").join(id);
    fs::create_dir_all(&dir).unwrap();
    let json = dir.join(format!("{id}.json"));
    fs::write(
        &json,
        format!(r#"{{"id":"{id}","inheritsFrom":"1.20.1"}}"#),
    )
    .unwrap();

    stamp_meta(
        &fixture.root,
        id,
        "1.20.1",
        &[("forge", "47.4.28")],
    )
    .unwrap();
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&json).unwrap()).unwrap();
    assert_eq!(value["_ybmcl"]["gameVersion"], "1.20.1");
    assert_eq!(value["_ybmcl"]["loaders"][0]["loader"], "forge");
    assert_eq!(value["_ybmcl"]["loaders"][0]["version"], "47.4.28");

    // 合并时记下的那套才是准的：已经有 `_ybmcl` 就不再覆盖。
    stamp_meta(&fixture.root, id, "1.21", &[]).unwrap();
    value = serde_json::from_str(&fs::read_to_string(&json).unwrap()).unwrap();
    assert_eq!(value["_ybmcl"]["gameVersion"], "1.20.1");
    assert_eq!(value["_ybmcl"]["loaders"][0]["loader"], "forge");
}

#[test]
fn legacy_fabric_joins_the_merge_pipeline() {
    use crate::merge::{instance_name, LoaderKind, LoaderPart};

    assert_eq!(
        LoaderKind::parse("legacyfabric"),
        Some(LoaderKind::LegacyFabric)
    );
    assert_eq!(
        LoaderKind::parse("legacy-fabric"),
        Some(LoaderKind::LegacyFabric)
    );

    let parts = vec![LoaderPart {
        kind: LoaderKind::LegacyFabric,
        version: "0.20.1".into(),
        version_id: "fabric-loader-0.20.1-1.12.2".into(),
    }];
    assert_eq!(
        instance_name("1.12.2", &parts),
        "1.12.2-LegacyFabric_0.20.1"
    );
}

fn write_world_zip(path: &Path, files: &[(&str, &[u8])]) {
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    for (name, bytes) in files {
        zip.start_file(*name, options).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn extract_world_keeps_the_world_folder() {
    let fixture = Fixture::new();
    let archive = fixture.root.join("oneblock.zip");
    write_world_zip(
        &archive,
        &[
            ("Oneblock/level.dat", b"level"),
            ("Oneblock/region/r.0.0.mca", b"region"),
        ],
    );

    let saves = fixture.root.join("saves");
    let name = crate::world::extract_world(&archive, &saves).unwrap();

    assert_eq!(name, "Oneblock");
    assert_eq!(fs::read(saves.join("Oneblock/level.dat")).unwrap(), b"level");
    assert_eq!(
        fs::read(saves.join("Oneblock/region/r.0.0.mca")).unwrap(),
        b"region"
    );
}

#[test]
fn extract_world_flattens_a_root_level_archive() {
    let fixture = Fixture::new();
    let archive = fixture.root.join("flat.zip");
    write_world_zip(&archive, &[("level.dat", b"level"), ("data/a.dat", b"a")]);

    let saves = fixture.root.join("saves");
    let name = crate::world::extract_world(&archive, &saves).unwrap();

    assert_eq!(name, "saves");
    assert_eq!(fs::read(saves.join("level.dat")).unwrap(), b"level");
    assert_eq!(fs::read(saves.join("data/a.dat")).unwrap(), b"a");
}

#[test]
fn extract_world_skips_zip_slip_entries() {
    let fixture = Fixture::new();
    let archive = fixture.root.join("evil.zip");
    write_world_zip(&archive, &[("../evil.txt", b"nope")]);

    let saves = fixture.root.join("saves");
    let error = crate::world::extract_world(&archive, &saves).unwrap_err();

    assert!(matches!(error, CoreError::Archive(_)), "{error}");
    assert!(!fixture.root.join("evil.txt").exists());
}

#[test]
fn wiki_id_matches_the_shipped_table() {
    // 钠、JEI、Terralith 在 MC 百科上都有条目，两个平台的 slug 也一样。
    assert_eq!(crate::wiki::wiki_id("modrinth", "sodium"), Some(2785));
    assert_eq!(crate::wiki::wiki_id("curseforge", "JEI"), Some(459));
    assert_eq!(
        crate::wiki::wiki_url("curseforge", "terralith").as_deref(),
        Some("https://www.mcmod.cn/class/4557.html")
    );
    // 表里没有的项目不显示按钮。
    assert_eq!(crate::wiki::wiki_id("modrinth", "not-a-real-project"), None);
    assert_eq!(crate::wiki::wiki_id("curseforge", "  "), None);
}

/// 手工冒烟：FTB 的搜索要并发取详情，确认不会卡住（默认跳过，需要网络）。
#[test]
#[ignore]
fn ftb_search_returns_packs() {
    let page = crate::ftb::search("", "relevance", 0, 5).expect("search");
    assert!(!page.items.is_empty());
    assert!(page.total > 0);
    for item in &page.items {
        assert!(item.id.starts_with("ftb:"));
        assert!(!item.title.is_empty());
    }
}

/// 手工冒烟：把 FTB 的版本清单写成临时 mrpack，确认能被 read_plan 读出来。
#[test]
#[ignore]
fn ftb_plan_zip_is_readable() {
    let dir = std::env::temp_dir().join(format!("ybumcl-ftb-plan-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("plan.mrpack");
    crate::ftb::write_plan_zip("ftb:35", "37", &path).expect("plan");
    let mut zip = zip::ZipArchive::new(std::fs::File::open(&path).expect("open")).expect("zip");
    let mut text = String::new();
    {
        use std::io::Read;
        let mut entry = zip.by_name("modrinth.index.json").expect("index");
        entry.read_to_string(&mut text).expect("read");
    }
    let value: serde_json::Value = serde_json::from_str(&text).expect("json");
    let files = value["files"].as_array().expect("files").len();
    assert!(files > 50, "files = {files}");
    assert_eq!(value["dependencies"]["minecraft"], "1.12.2");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 手工冒烟：同一个文件给两个地址，竞速应该只留一份、内容对得上。
#[test]
#[ignore]
fn race_download_keeps_one_file() {
    let fixture = Fixture::new();
    let versions = crate::modrinth::versions("1KVo5zza", Some("26.3"), Some("fabric"))
        .expect("取版本列表");
    let version = versions.first().expect("至少一个版本").clone();
    let dest = fixture.root.join(&version.file_name);
    let job = crate::download::DownloadJob {
        // 同一个直链给两次，就相当于两条线路同时开工。
        urls: vec![version.file_url.clone(), version.file_url.clone()],
        dest: dest.clone(),
        size: Some(version.file_size),
        sha1: None,
    };
    let cancel = crate::download::CancellationToken::default();
    let (written, failed) = crate::download::fetch_files_to(
        &[job],
        &cancel,
        2,
        crate::download::DownloadStrategy::default(),
        |_| {},
    )
        .expect("下载不该报错");
    assert_eq!(written, 1, "应该算完成一个文件");
    assert!(failed.is_empty(), "竞速不该留下失败记录：{failed:?}");
    let size = std::fs::metadata(&dest).expect("文件应该存在").len();
    assert_eq!(size, version.file_size, "大小要和清单一致");
    // 竞速用的临时文件都要清干净。
    let leftovers: Vec<_> = std::fs::read_dir(&fixture.root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("race"))
        .collect();
    assert!(leftovers.is_empty(), "不该留下 {leftovers:?}");
}

#[test]
fn sweep_parts_only_removes_half_downloaded_files() {
    let fixture = Fixture::new();
    fixture.file("assets/objects/aa/a.part", b"half");
    fixture.file("assets/objects/aa/a.jar", b"whole");
    fixture.file("libraries/net/x/b.part", b"half");
    fixture.file("versions/test/c.part", b"half");
    fixture.file("versions/test/test.jar", b"keep");

    let removed = merge::sweep_parts(&fixture.root);

    assert_eq!(removed, 3, "应该只数到 3 个 .part");
    assert!(!fixture.root.join("assets/objects/aa/a.part").exists(), "a.part 要删掉");
    assert!(!fixture.root.join("libraries/net/x/b.part").exists(), "b.part 要删掉");
    assert!(!fixture.root.join("versions/test/c.part").exists(), "c.part 要删掉");
    assert!(fixture.root.join("assets/objects/aa/a.jar").exists(), "完整文件不能动");
    assert!(fixture.root.join("versions/test/test.jar").exists(), "完整文件不能动");
}

#[test]
fn relative_library_files_reads_local_libraries() {
    let fixture = Fixture::new();
    fixture.file("libraries/org/example/lib/1.0/lib-1.0.jar", b"jar-bytes");
    let json = json!({
        "libraries": [
            {
                "name": "org.example:lib:1.0",
                "downloads": {
                    "artifact": {
                        "relativeUrl": "org/example/lib/1.0/lib-1.0.jar",
                        "url": "http://127.0.0.1:9/lib-1.0.jar"
                    }
                }
            },
            {
                "name": "org.example:missing:1.0",
                "downloads": {
                    "artifact": { "path": "org/example/missing/1.0/missing-1.0.jar" }
                }
            }
        ]
    });

    let files = packages::relative_library_files(
        &json,
        &fixture.root,
        &CancellationToken::default(),
    )
    .unwrap();

    assert_eq!(files.len(), 1, "本地有的库带进 zip，缺的跳过");
    assert_eq!(files[0].0, "org/example/lib/1.0/lib-1.0.jar");
    assert_eq!(files[0].1, b"jar-bytes");
}

#[test]
fn plan_chunks_covers_the_file_without_gaps() {
    // 小于阈值不分块（阈值 256 KiB）
    assert!(download::plan_chunks(256 * 1024 - 1).is_empty());

    // 413 KB 这类小文件也要分块（至少两块，避免单连接被限速拖住）
    let small = 413 * 1024;
    let small_chunks = download::plan_chunks(small);
    assert!(small_chunks.len() >= 2);
    assert_eq!(small_chunks[0].0, 0);
    assert_eq!(small_chunks.last().unwrap().1, small - 1);

    // 11 MB → 至少两块，且首块从 0 开始、末块以 size-1 结束、中间无缝
    let size = 11 * 1024 * 1024;
    let chunks = download::plan_chunks(size);
    assert!(chunks.len() >= 2);
    assert_eq!(chunks[0].0, 0);
    assert_eq!(chunks.last().unwrap().1, size - 1);
    for pair in chunks.windows(2) {
        assert_eq!(pair[0].1 + 1, pair[1].0, "块之间不能有空隙");
    }

    // 大文件块数封顶，且总量仍然对齐
    let big = 256 * 1024 * 1024;
    let chunks = download::plan_chunks(big);
    assert!(chunks.len() <= 8);
    assert_eq!(chunks.last().unwrap().1, big - 1);
    let covered: u64 = chunks.iter().map(|(start, end)| end - start + 1).sum();
    assert_eq!(covered, big);
}

#[test]
fn speed_limit_only_slows_when_it_is_set() {
    // 0 = 不限速：记账直接返回，不该有任何等待。
    let unlimited = download::Rate::new(0);
    let start = std::time::Instant::now();
    for _ in 0..2000 {
        unlimited.throttle(4096);
    }
    assert!(
        start.elapsed() < std::time::Duration::from_millis(200),
        "限速为 0 时不应该等待，实际 {:?}",
        start.elapsed()
    );

    // 64 KB/s 下领 16 KiB 应该等约 0.25 s。
    let limited = download::Rate::new(64 * 1024);
    let start = std::time::Instant::now();
    limited.throttle(16 * 1024);
    let took = start.elapsed();
    assert!(
        took >= std::time::Duration::from_millis(150),
        "限速后应当阻塞，实际只等了 {took:?}"
    );
}

#[test]
fn speed_tiers_match_pcl() {
    // PCL `NetTaskSpeedLimitHigh`：0..=14 每档 0.1 MB/s，15..=31 每档 0.5，
    // 32..=41 每档 1 MB/s，42 不限。
    assert_eq!(download::tier_limit(0), 104_858);
    assert_eq!(download::tier_limit(14), 1_572_864);
    assert_eq!(download::tier_limit(15), 2_097_152);
    assert_eq!(download::tier_limit(31), 10_485_760);
    assert_eq!(download::tier_limit(32), 11_534_336);
    assert_eq!(download::tier_limit(41), 20_971_520);
    assert_eq!(download::tier_limit(42), 0);
    assert_eq!(download::tier_for_limit(2_097_152), 15);
    assert_eq!(download::tier_for_limit(0), 42);
}

#[test]
fn unchanged_instance_reinstall_is_a_no_op() {
    let f = Fixture::new();
    let name = "Pack Test";
    f.version(
        name,
        json!({
            "id": name,
            "_ybmcl": {"gameVersion": "test", "loaders": []},
            "mainClass": "net.minecraft.client.main.Main",
            "libraries": [],
            "assetIndex": {"id": "test"},
            "downloads": {"client": {"url": "https://piston-data.mojang.com/v1/objects/fixture/client.jar"}},
        }),
    );
    f.file(&format!("versions/{name}/mods/keep.jar"), b"keep");
    let request = crate::instance::InstanceRequest {
        game_dir: f.root.to_string_lossy().into(),
        game_version: "test".into(),
        instance_name: Some(name.into()),
        loaders: Vec::new(),
        java_path: None,
        strategy: Default::default(),
        threads: None,
        overwrite: true,
    };
    let summary = crate::instance::install_instance(
        request,
        crate::download::CancellationToken::default(),
        |_| {},
    )
    .unwrap();
    assert_eq!(summary.version_id, name);
    // 配置没变就别重装：实例里的东西原样留着，也不该产生备份。
    assert_eq!(
        fs::read(f.root.join(format!("versions/{name}/mods/keep.jar"))).unwrap(),
        b"keep"
    );
    assert!(!f.root.join("versions/install-backups").is_dir() || fs::read_dir(f.root.join("versions/install-backups")).unwrap().next().is_none());
}

#[test]
fn modlauncher_entry_points_are_allowed() {
    // Forge 1.13–1.17 用 modlauncher Launcher 起步，1.16.5 就是它。
    assert!(supported_main("cpw.mods.modlauncher.Launcher"));
    assert!(supported_main("net.neoforged.modlauncher.Launcher"));
    assert!(supported_main("cpw.mods.bootstraplauncher.BootstrapLauncher"));
    assert!(supported_main("net.minecraft.launchwrapper.Launch"));
    assert!(!supported_main("com.example.Evil"));
}

#[test]
fn build_revision_comes_from_the_parentheses() {
    let text = "openjdk version \"17.0.20\" 2025-01-21\nOpenJDK Runtime Environment (build 17.0.20+8-LTS)";
    assert_eq!(build_revision(text), Some("17.0.20+8-LTS".to_owned()));
    // 老 8 的版本串没有 build 括号，别硬编一个出来。
    let old = "java version \"1.8.0_503\"\nJava(TM) SE Runtime Environment (build 1.8.0_503-b09)";
    assert_eq!(build_revision(old), Some("1.8.0_503-b09".to_owned()));
    assert_eq!(build_revision("java version \"1.8.0_503\""), None);
}
