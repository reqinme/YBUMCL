# launcher-core

Windows-focused vanilla/Fabric launch planning and verified installation from Mojang and Fabric HTTPS services. This crate has no Tauri dependency, account authentication, or game process spawning.

```rust,ignore
let versions = launcher_core::scan_versions(std::path::Path::new("C:/Games/.minecraft"))?;
let java = launcher_core::detect_java();
let plan = launcher_core::build_launch_plan(request)?;
// Caller runs plan.java_path with plan.args and plan.working_directory.
// For diagnostics use plan.redacted_args(), never raw arguments.
```

All public data structs serialize and deserialize with camelCase field names. Public functions:

- `scan_versions(&Path) -> Result<Vec<VersionSummary>, CoreError>` lists sorted installed version metadata. `supported` means the resolved metadata has a supported vanilla or Fabric entry point; preparation still checks every dependency.
- `detect_java() -> Vec<JavaInstallation>` checks JAVA_HOME, PATH, and common Windows vendor folders, deduplicates canonical executable paths, and runs candidates with `-version`. Each candidate has a five-second probe timeout.
- `build_launch_plan(LaunchRequest) -> Result<LaunchPlan, CoreError>` validates username (1–16 ASCII letters, digits, underscores), memory (512–65536 MiB), selected Java/version paths, the Java major requirement declared by metadata, client JAR, applicable libraries, assets index and object files, legacy virtual/resources files, and logging configuration.
- `LaunchPlan::redacted_args() -> Vec<String>` returns an argument copy with access tokens redacted.

Preparation supports modern game/JVM argument arrays with ordered Windows OS/architecture/version and disabled optional-feature rules, legacy quoted `minecraftArguments`, Maven fallback library paths, Windows native classifiers, vanilla assets and logging placeholders. It builds a Windows semicolon-separated classpath. Vanilla and Fabric metadata support recursive `inheritsFrom` with cycle detection and a 32-file limit. Parent arguments precede child arguments; child libraries override the same Maven group/artifact/classifier; the root ancestor supplies the client JAR. Both supported Fabric KnotClient entry points are accepted. Other loader entry points, custom JAR selections, JVM agents, and unrecognized JVM options are rejected. The supported JVM-option allowlist intentionally favors known vanilla metadata over extensibility. Installed metadata and JARs must still be trusted; this is not a sandbox or cryptographic installation verifier.

Vanilla 26.3 JVM support includes decimal `-XX:StackShadowPages` values from 1 through 512, exact `--enable-native-access=ALL-UNNAMED`, and the exact export `--add-exports java.base/jdk.internal.misc=ALL-UNNAMED`. Version-specific native subdirectories such as `launcher-natives/java` are preserved after containment validation. The optional `arguments.default-user-jvm` section is intentionally ignored in v1; requested maximum memory remains authoritative and collector tuning is left to Java defaults. Its newer `versionRange` rules therefore do not affect these unused defaults.

Preparation has a filesystem side effect: after argument/dependency validation it extracts native archives into `versions/<versionId>/launcher-natives`. Existing entries can be overwritten. Extraction rejects traversal, absolute paths, Windows device/stream names, symlinks/junction escapes and expanded native data above 512 MiB. No native extraction happens during scanning. Failed extraction can leave partial native files, which the next successful preparation overwrites. Concurrent modification of an installation during preparation is unsupported.

When accessToken is absent or empty, identity is explicitly offline: the UUID is Java's MD5 name UUID for `OfflinePlayer:<username>` (compact hexadecimal), and the placeholder token is `0`. A supplied token requires a valid UUID. Authentication, token refresh, entitlements, and online connectivity belong to the caller. Optional modern client ID/XUID values are empty because the request contract does not provide them.

Launch preparation does not download or repair missing files. The separate `install_version` API installs vanilla files and optionally a Fabric profile (`InstallRequest.fabric_loader`), checks file digests, reuses verified files, reports progress, and supports cancellation. `download::list_fabric_loaders` queries compatible loaders. Legacy virtual assets must already exist; preparation does not synthesize them. Java discovery, installation and launch planning are blocking operations; desktop integrations should call them off the UI thread.

Run `cargo test -p launcher-core` from the workspace root. Tests use isolated temporary installation fixtures and do not start Minecraft or require a Java installation.
