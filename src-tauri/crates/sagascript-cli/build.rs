use std::process::Command;

#[path = "../../build_identity.rs"]
mod build_identity;

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
    let dirty = build_identity::local_source_dirty().unwrap_or(true);
    if dirty {
        format!("{hash}-dirty")
    } else {
        hash
    }
}

fn local_source_revision() -> String {
    command_output(&["rev-parse", "HEAD"])
        .filter(|revision| revision.len() == 40 && revision.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap_or_else(|| "unknown".to_string())
}

fn local_source_dirty() -> String {
    match build_identity::local_source_dirty() {
        Some(dirty) => dirty.to_string(),
        None => "unknown".to_string(),
    }
}

fn source_revision() -> String {
    match std::env::var_os("SAGASCRIPT_SOURCE_REVISION_INPUT") {
        Some(raw) => match raw.to_str() {
            Some(revision)
                if revision.len() == 40 && revision.bytes().all(|b| b.is_ascii_hexdigit()) =>
            {
                revision.to_string()
            }
            _ => "unknown".to_string(),
        },
        None => local_source_revision(),
    }
}

fn source_dirty() -> String {
    match std::env::var_os("SAGASCRIPT_SOURCE_DIRTY_INPUT") {
        Some(raw) => match raw.to_str() {
            Some("true") => "true".to_string(),
            Some("false") => "false".to_string(),
            _ => "unknown".to_string(),
        },
        None => local_source_dirty(),
    }
}

fn valid_source_revision_input() -> bool {
    std::env::var("SAGASCRIPT_SOURCE_REVISION_INPUT").is_ok_and(|revision| {
        revision.len() == 40 && revision.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

fn valid_source_dirty_input() -> bool {
    matches!(
        std::env::var("SAGASCRIPT_SOURCE_DIRTY_INPUT").as_deref(),
        Ok("true" | "false")
    )
}

fn emit_source_rerun_triggers() {
    // Watch the workspace's source trees without watching its root or target/.
    // The latter contains Cargo's build output and would make the build script
    // invalidate itself forever.
    for path in [
        "../../build_identity.rs",
        "../../../scripts/build-identity-generated-paths.txt",
        "../../crates",
        "../../src",
        "../../../src",
        "../../../package-lock.json",
        "../../Cargo.lock",
        "../../Cargo.toml",
        "Cargo.toml",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
}

fn emit_git_rerun_triggers() {
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

fn has_complete_ci_metadata() -> bool {
    ["SAGASCRIPT_GIT_HASH", "SAGASCRIPT_BUILD_DATE"]
        .into_iter()
        .all(|key| std::env::var(key).is_ok_and(|value| !value.is_empty()))
        && valid_source_revision_input()
        && valid_source_dirty_input()
}

fn main() {
    println!("cargo:rerun-if-env-changed=SAGASCRIPT_GIT_HASH");
    println!("cargo:rerun-if-env-changed=SAGASCRIPT_BUILD_DATE");
    println!("cargo:rerun-if-env-changed=SAGASCRIPT_SOURCE_REVISION_INPUT");
    println!("cargo:rerun-if-env-changed=SAGASCRIPT_SOURCE_DIRTY_INPUT");
    // CI supplies the complete source identity. Avoid watching Git paths in
    // that mode: hosted checkouts commonly omit packed-refs, and Cargo treats
    // a missing watched path as dirty on every invocation. Local builds keep
    // the Git watches because they still derive identity from the checkout.
    if !has_complete_ci_metadata() {
        emit_git_rerun_triggers();
    }
    emit_source_rerun_triggers();

    let git_hash = metadata_value("SAGASCRIPT_GIT_HASH", local_git_hash);
    let build_date = metadata_value("SAGASCRIPT_BUILD_DATE", || {
        // Keep local builds reproducible enough to identify without adding a
        // chrono build dependency to the CLI crate. Release CI always supplies
        // an ISO date explicitly.
        command_output(&["show", "-s", "--format=%cs", "HEAD"])
            .unwrap_or_else(|| "unknown".to_string())
    });

    println!("cargo:rustc-env=SAGASCRIPT_CLI_GIT_HASH={git_hash}");
    println!("cargo:rustc-env=SAGASCRIPT_CLI_BUILD_DATE={build_date}");
    println!(
        "cargo:rustc-env=SAGASCRIPT_SOURCE_REVISION={}",
        source_revision()
    );
    println!("cargo:rustc-env=SAGASCRIPT_SOURCE_DIRTY={}", source_dirty());
}
