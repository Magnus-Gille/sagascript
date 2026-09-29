use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
}

fn main() {
    println!("cargo:rerun-if-env-changed=SAGASCRIPT_GIT_SHA");
    if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
        println!("cargo:rerun-if-changed={git_dir}/index");
    }
    let sha = std::env::var("SAGASCRIPT_GIT_SHA")
        .ok()
        .or_else(|| git(&["rev-parse", "HEAD"]))
        .filter(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap_or_else(|| "0".repeat(40));
    let dirty = git(&["status", "--porcelain=v1"]).is_some_and(|s| !s.is_empty());
    println!("cargo:rustc-env=SAGASCRIPT_HOST_GIT_SHA={sha}");
    println!("cargo:rustc-env=SAGASCRIPT_HOST_DIRTY={dirty}");
}
