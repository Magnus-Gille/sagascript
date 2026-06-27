use std::process::Command;

fn main() {
    // Stamp build info
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=GIT_HASH={}", git_hash.trim());
    println!(
        "cargo:rustc-env=BUILD_DATE={}",
        chrono::Utc::now().format("%Y-%m-%d")
    );

    // Only run the Tauri build script for the GUI build. A headless CLI build
    // (`--no-default-features`, e.g. on Linux) has no Tauri context to generate.
    #[cfg(feature = "gui")]
    tauri_build::build();
}
