//! Facts about the running build and host.

// -- Build info --

#[tauri::command]
pub async fn get_build_info() -> Result<BuildInfo, String> {
    Ok(BuildInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        git_hash: env!("GIT_HASH").to_string(),
        build_date: env!("BUILD_DATE").to_string(),
    })
}

#[derive(serde::Serialize)]
pub struct BuildInfo {
    version: String,
    git_hash: String,
    build_date: String,
}

/// The host OS as the frontend reads it (`src/lib/platform.ts`): `macos`,
/// `windows` or `linux`; `unknown` for anything else, so Onboarding takes its
/// portable path.
#[tauri::command]
pub async fn get_platform() -> Result<String, String> {
    Ok(platform_name(std::env::consts::OS).to_string())
}

/// `std::env::consts::OS` names more systems (`freebsd`, `android`, ...) than
/// the frontend knows; they all stay `unknown`, as they were when the answer
/// came from `cfg` blocks.
fn platform_name(os: &str) -> &'static str {
    match os {
        "macos" => "macos",
        "windows" => "windows",
        "linux" => "linux",
        _ => "unknown",
    }
}

#[cfg(test)]
#[path = "system_tests.rs"]
mod tests;
