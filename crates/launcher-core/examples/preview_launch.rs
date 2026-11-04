//! Inspect a real installation without starting a game or exposing credentials.
use launcher_core::{build_launch_plan, LaunchRequest};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !(3..=4).contains(&args.len()) {
        return Err("Usage: preview_launch <absolute-game-dir> <version-id> <java-path> [instances/<safe-id>]".into());
    }
    let plan = build_launch_plan(LaunchRequest {
        game_dir: args[0].clone(),
        game_directory: args.get(3).cloned(),
        version_id: args[1].clone(),
        java_path: args[2].clone(),
        username: "Player".into(),
        max_memory_mb: 2048,
        access_token: None,
        uuid: None,
        ..LaunchRequest::default()
    })?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "javaPath": plan.java_path, "workingDirectory": plan.working_directory,
            "versionId": plan.version_id, "args": plan.redacted_args(), "warnings": plan.warnings,
        }))?
    );
    Ok(())
}
