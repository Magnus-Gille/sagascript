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

#[tauri::command]
pub async fn get_platform() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        Ok("macos".to_string())
    }
    #[cfg(target_os = "windows")]
    {
        Ok("windows".to_string())
    }
    #[cfg(target_os = "linux")]
    {
        Ok("linux".to_string())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Ok("unknown".to_string())
    }
}
