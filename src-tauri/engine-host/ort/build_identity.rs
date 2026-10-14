//! When the ONNX host's build counts as dirty (#367). The rule is shared with
//! `scripts/engine-host-identity.sh` (Core ML host) and `scripts/ci-build-identity.mjs` (Windows
//! packaging gate) through `scripts/build-identity-generated-paths.txt`. This build script runs on
//! Windows, so it reads that list instead of invoking the shell script. Included by `build.rs`
//! and, so that these tests run with `cargo test`, by `src/main.rs`.

/// `scripts/build-identity-generated-paths.txt`: one path per line, `#` comments.
const GENERATED_PATHS: &str = include_str!("../../../scripts/build-identity-generated-paths.txt");

/// The generated files that never make a build dirty.
pub fn generated_paths() -> impl Iterator<Item = &'static str> {
    GENERATED_PATHS
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
}

/// True when `git status --porcelain=v1` lists anything besides the generated files.
/// (Porcelain paths are relative to the repository root regardless of the working directory.)
pub fn is_dirty(porcelain: &str) -> bool {
    porcelain
        .lines()
        .filter(|line| !line.is_empty())
        .any(|line| {
            let path = line.get(3..).unwrap_or(line).trim_matches('"');
            !generated_paths().any(|allowed| allowed == path)
        })
}

#[cfg(test)]
mod tests {
    use super::{generated_paths, is_dirty};

    #[test]
    fn the_shared_list_names_the_tauri_schemas() {
        let paths: Vec<&str> = generated_paths().collect();
        assert_eq!(
            paths,
            [
                "src-tauri/gen/schemas/acl-manifests.json",
                "src-tauri/gen/schemas/capabilities.json",
                "src-tauri/gen/schemas/desktop-schema.json",
                "src-tauri/gen/schemas/windows-schema.json",
            ]
        );
    }

    #[test]
    fn only_allowed_generated_files_are_ignored() {
        assert!(!is_dirty(""));
        assert!(!is_dirty(" M src-tauri/gen/schemas/windows-schema.json\n?? src-tauri/gen/schemas/acl-manifests.json"));
        assert!(is_dirty(
            " M src-tauri/gen/schemas/windows-schema.json\n M src-tauri/src/lib.rs"
        ));
        assert!(is_dirty("?? scripts/__pycache__/x.pyc"));
        assert!(is_dirty("?? src-tauri/gen/schemas/macOS-schema.json"));
    }
}
