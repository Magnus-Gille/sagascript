//! The module layering of `sagascript-core` (#372), guarded as the text of the
//! source files, since rustc has no notion of an upward `use`.
//!
//! ```text
//! models, glossary   leaves: depend on neither settings nor transcription
//!        ^
//!     settings       reads the model catalog; the machine is injected
//!        ^
//!  transcription     reads settings (the engine host config)
//!        ^
//!     machine        composes settings with transcription's probes
//! ```
//!
//! The one upward reference left is the re-export in `settings/store.rs` of
//! `machine::{load, update, try_update}`: a shim for the old paths, to be
//! removed one release after the CLI and the app call `machine` directly.

use std::fs;
use std::path::{Path, PathBuf};

fn src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// The code lines of the files under `src/<dir>` (`""`: all of src; comment
/// lines dropped), as `(file name relative to src, trimmed line)`.
fn code_lines(dir: &str) -> Vec<(String, String)> {
    let root = src();
    let mut files = Vec::new();
    let start = root.join(dir);
    if start.is_dir() {
        rust_files(&start, &mut files);
    } else {
        files.push(root.join(format!("{dir}.rs")));
    }
    let mut lines = Vec::new();
    for file in files {
        let name = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        for line in fs::read_to_string(&file).unwrap().lines() {
            let line = line.trim();
            if !line.starts_with("//") {
                lines.push((name.clone(), line.to_string()));
            }
        }
    }
    lines
}

fn offenders(dir: &str, forbidden: &[&str]) -> Vec<String> {
    code_lines(dir)
        .into_iter()
        .filter(|(_, line)| forbidden.iter().any(|path| line.contains(path)))
        .map(|(file, line)| format!("{file}: {line}"))
        .collect()
}

#[test]
fn the_model_catalog_depends_on_neither_settings_nor_transcription() {
    let found = offenders(
        "models",
        &[
            "crate::settings",
            "crate::transcription",
            "crate::machine",
            "super::super",
        ],
    );
    assert!(
        found.is_empty(),
        "models must stay a leaf:\n{}",
        found.join("\n")
    );
}

#[test]
fn the_glossary_is_a_leaf() {
    let found = offenders("glossary", &["crate::", "super::super"]);
    assert!(
        found.is_empty(),
        "glossary must stay a leaf:\n{}",
        found.join("\n")
    );
}

#[test]
fn settings_do_not_reach_into_transcription() {
    let found = offenders("settings", &["crate::transcription", "transcription::"]);
    assert!(
        found.is_empty(),
        "settings must get machine state injected, not probe it:\n{}",
        found.join("\n")
    );
}

#[test]
fn only_the_compat_shim_in_the_store_names_the_machine_module() {
    assert_eq!(
        offenders("", &["crate::machine"]),
        vec!["settings/store.rs: pub use crate::machine::{load, try_update, update};".to_string()],
        "nothing but the one shim line may import `machine`: it sits above settings and transcription"
    );
}
