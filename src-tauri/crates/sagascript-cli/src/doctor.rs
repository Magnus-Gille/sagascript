//! `sagascript doctor`: read-only environment checks. Never records, never
//! plays sound and never triggers an OS permission prompt.

use clap::Args;
use serde_json::json;

use sagascript_core::error::DictationError;

#[derive(Args)]
pub struct DoctorArgs {
    /// Output machine-readable JSON
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: DoctorArgs) -> Result<(), DictationError> {
    #[cfg(feature = "record")]
    let system = {
        let s = sagascript_core::audio::system::system_audio_status();
        json!({
            "supported": s.supported,
            "per_app_capture": s.per_app_capture,
            "permission": s.permission,
            "backend": s.backend,
            "detail": s.detail,
        })
    };
    #[cfg(not(feature = "record"))]
    let system = json!({
        "supported": false,
        "permission": "unsupported",
        "backend": "none",
        "detail": "This build has no audio capture (built without the `record` feature).",
    });

    if args.json {
        let report = json!({ "system_audio": system });
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    } else {
        println!("System audio capture");
        println!("  supported:  {}", system["supported"]);
        println!("  backend:    {}", system["backend"].as_str().unwrap_or("none"));
        println!("  per-app:    {}", system["per_app_capture"].as_bool().unwrap_or(false));
        println!("  permission: {}", system["permission"].as_str().unwrap_or("unknown"));
        println!("  {}", system["detail"].as_str().unwrap_or(""));
    }
    Ok(())
}
