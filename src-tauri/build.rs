use std::process::Command;

fn command_output(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
}

fn metadata_value(key: &str, fallback: impl FnOnce() -> String) -> String {
    std::env::var(key).ok().unwrap_or_else(fallback)
}

fn local_git_hash() -> String {
    let hash =
        command_output(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    let dirty = command_output(&["status", "--porcelain=v1", "--untracked-files=all"])
        .is_some_and(|status| !status.is_empty());
    if dirty {
        format!("{hash}-dirty")
    } else {
        hash
    }
}

fn emit_source_rerun_triggers() {
    // Watch source trees that affect the binary's identity without watching
    // the package root, which would include target/ and cause rebuild loops.
    for path in [
        "src",
        "crates",
        "../src",
        "../package-lock.json",
        "Cargo.lock",
        "Cargo.toml",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
}

fn emit_git_rerun_triggers() {
    println!("cargo:rerun-if-env-changed=SAGASCRIPT_GIT_HASH");
    println!("cargo:rerun-if-env-changed=SAGASCRIPT_BUILD_DATE");
    println!("cargo:rerun-if-changed=build-meta.env");

    if let Some(git_dir) = command_output(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
        println!("cargo:rerun-if-changed={git_dir}/index");
        if let Some(reference) = command_output(&["symbolic-ref", "-q", "HEAD"]) {
            if let Some(reference_path) =
                command_output(&["rev-parse", "--git-path", reference.as_str()])
            {
                println!("cargo:rerun-if-changed={reference_path}");
            }
        }
        if let Some(packed_refs) = command_output(&["rev-parse", "--git-path", "packed-refs"]) {
            println!("cargo:rerun-if-changed={packed_refs}");
        }
    }
}

fn main() {
    emit_git_rerun_triggers();
    emit_source_rerun_triggers();

    let git_hash = metadata_value("SAGASCRIPT_GIT_HASH", local_git_hash);
    let build_date = metadata_value("SAGASCRIPT_BUILD_DATE", || {
        chrono::Utc::now().format("%Y-%m-%d").to_string()
    });

    println!("cargo:rustc-env=GIT_HASH={git_hash}");
    println!("cargo:rustc-env=BUILD_DATE={build_date}");

    tauri_build::build();
}
