//! The top level of `sagascript-core/src` (#372), locked.
//!
//! The rule (owner decision of 2026-10-17 on #372):
//!
//! 1. A subsystem is one directory (`meeting/`, `diarization/`, ...), never
//!    several files beside `lib.rs`.
//! 2. The top level holds `lib`, `error` and a named allowlist of cross-cutting
//!    modules, each justified in one line in `docs/ARCHITECTURE.md` section 1.1.
//! 3. `*_tests.rs` files (test modules pulled in with `#[path]`, #363) do not count.
//! 4. This test compares the files and directories of `src/`, the `mod` lines of
//!    `lib.rs` and the table in the architecture document with that rule.
//!
//! A new file beside `lib.rs` therefore fails here. That is the point: give the
//! code a directory of its own, or decide that the module is cross-cutting by
//! adding it to [`CROSS_CUTTING`] and to the table in `docs/ARCHITECTURE.md`.
//!
//! `lib.rs` also re-exports the old paths of modules that moved into `meeting/`
//! and `diarization/`, for one release, `#[doc(hidden)]`. Nothing in the
//! workspace names them any more (`tests/module_layering.rs` locks that). They
//! are listed in [`COMPAT_SHIMS`] so they do not multiply; removing one means
//! removing it from both places.
//!
//! Everything is read as text with `str::lines`, so `\n` and `\r\n` read alike.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// The top level beside the subsystem directories: the crate root and the one
/// error type every subsystem returns.
const ROOT_AND_ERROR: [&str; 2] = ["lib", "error"];

/// Cross-cutting modules that belong to no one subsystem. Each has a row in the
/// table in `docs/ARCHITECTURE.md` section 1.1 saying why.
const CROSS_CUTTING: [&str; 9] = [
    "download",
    "fsutil",
    "glossary",
    "machine",
    "path_scrub",
    "run_context",
    "sync",
    "test_support",
    "testing",
];

/// Old paths `lib.rs` re-exports for one release; see the module documentation.
/// Each goes after the release that ships them (#372).
const COMPAT_SHIMS: [&str; 11] = [
    "diarization_evaluation",
    "diarization_qualification",
    "diarization_reference",
    "diarization_report",
    "diarization_strata",
    "meeting_media",
    "meeting_reprocess",
    "meeting_reprocess_plan",
    "meeting_reprocess_proposal",
    "meeting_review",
    "speaker_hint",
];

/// The heading of the table in the architecture document.
const TABLE_HEADER: &str = "| Top-level module | Why it sits at the top level |";

fn src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn architecture_document() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/ARCHITECTURE.md");
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| name.to_string()).collect()
}

fn allowed_files() -> BTreeSet<String> {
    set(&ROOT_AND_ERROR)
        .into_iter()
        .chain(set(&CROSS_CUTTING))
        .collect()
}

// ---- reading the tree ----------------------------------------------------

/// The `.rs` files directly in `dir`, without their extension, leaving out
/// `*_tests.rs` (rule 3), and the names of its subdirectories.
fn files_and_directories(dir: &Path) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut files = BTreeSet::new();
    let mut directories = BTreeSet::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if path.is_dir() {
            directories.insert(name);
        } else if let Some(stem) = name.strip_suffix(".rs") {
            if !stem.ends_with("_tests") {
                files.insert(stem.to_string());
            }
        }
    }
    (files, directories)
}

/// The module names `lib.rs` declares (`mod x;` in any visibility, whatever
/// attribute is on the line before) and the names its `pub use crate::..;` lines
/// bring in: the `as` name, or the last segment without one.
fn lib_declarations(lib: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut modules = BTreeSet::new();
    let mut reexports = BTreeSet::new();
    for line in lib.lines().map(str::trim) {
        if line.starts_with("//") || line.starts_with("#[") || line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("pub use ") {
            let path = rest
                .strip_prefix("crate::")
                .and_then(|path| path.strip_suffix(';'))
                .filter(|path| !path.contains(['{', '*', ',']))
                .unwrap_or_else(|| {
                    panic!(
                        "lib.rs: write a re-export as one line `pub use crate::path as name;` \
                         (no groups, no globs) so this test can read it: `{line}`"
                    )
                });
            let name = match path.split_once(" as ") {
                Some((_, alias)) => alias,
                None => path.rsplit("::").next().unwrap(),
            };
            reexports.insert(name.trim().to_string());
        } else if let Some(name) = line
            .strip_prefix("pub mod ")
            .or_else(|| line.strip_prefix("pub(crate) mod "))
            .or_else(|| line.strip_prefix("mod "))
        {
            modules.insert(name.trim_end_matches(';').trim().to_string());
        }
    }
    (modules, reexports)
}

/// The rows of the table under [`TABLE_HEADER`]: `(module, justification)`.
fn documented_modules(document: &str) -> Vec<(String, String)> {
    let mut lines = document.lines().map(str::trim);
    assert!(
        lines.any(|line| line == TABLE_HEADER),
        "docs/ARCHITECTURE.md section 1.1 has no table headed `{TABLE_HEADER}`"
    );
    lines
        .skip(1) // the `|---|---|` line
        .take_while(|line| line.starts_with('|'))
        .map(|row| {
            let cells: Vec<&str> = row.trim_matches('|').split('|').map(str::trim).collect();
            assert_eq!(cells.len(), 2, "a row has a module and a reason: `{row}`");
            (cells[0].trim_matches('`').to_string(), cells[1].to_string())
        })
        .collect()
}

fn listed(names: &BTreeSet<String>) -> String {
    names.iter().cloned().collect::<Vec<_>>().join(", ")
}

// ---- the rule ------------------------------------------------------------

#[test]
fn the_top_level_files_are_lib_error_and_the_allowlist() {
    let (files, directories) = files_and_directories(&src());
    let allowed = allowed_files();
    let stray: BTreeSet<String> = files.difference(&allowed).cloned().collect();
    // A cross-cutting module may grow into a directory (`foo/mod.rs`) once it
    // has submodules, as `test_support` did; it is still on the allowlist.
    let present: BTreeSet<String> = files.union(&directories).cloned().collect();
    let missing: BTreeSet<String> = allowed.difference(&present).cloned().collect();
    assert!(
        stray.is_empty(),
        "new top-level file(s) in sagascript-core/src: {}.\n\
         A subsystem gets a directory of its own (`foo/mod.rs`), not several files beside lib.rs. \
         If the module is truly cross-cutting, add it to CROSS_CUTTING in this test and a row to \
         the table in docs/ARCHITECTURE.md section 1.1: that is a deliberate decision (#372).",
        listed(&stray)
    );
    assert!(
        missing.is_empty(),
        "the allowlist names module(s) that no longer exist: {}. Remove them from CROSS_CUTTING here \
         and from the table in docs/ARCHITECTURE.md section 1.1.",
        listed(&missing)
    );
}

#[test]
fn every_top_level_directory_is_a_subsystem_with_a_mod_rs() {
    let (files, directories) = files_and_directories(&src());
    for directory in &directories {
        assert!(
            src().join(directory).join("mod.rs").is_file(),
            "src/{directory}/ has no mod.rs: a subsystem directory is declared by its mod.rs"
        );
        assert!(
            !files.contains(directory),
            "src/{directory}.rs and src/{directory}/ both exist: a subsystem is one directory"
        );
    }
}

#[test]
fn lib_rs_declares_exactly_the_files_directories_and_compat_shims() {
    let lib = fs::read_to_string(src().join("lib.rs")).unwrap();
    let (modules, reexports) = lib_declarations(&lib);

    let (files, directories) = files_and_directories(&src());
    let on_disk: BTreeSet<String> = files
        .union(&directories)
        .filter(|name| name.as_str() != "lib")
        .cloned()
        .collect();
    assert_eq!(
        modules,
        on_disk,
        "the `mod` lines of lib.rs and the files and directories of src/ differ: only in lib.rs: \
         [{}]; only on disk: [{}]",
        listed(&modules.difference(&on_disk).cloned().collect()),
        listed(&on_disk.difference(&modules).cloned().collect())
    );
    assert_eq!(
        reexports,
        set(&COMPAT_SHIMS),
        "the `pub use crate::..;` lines of lib.rs are the old paths of moved modules and are \
         listed in COMPAT_SHIMS; a new one is a decision, a removed one leaves the list too"
    );
}

#[test]
fn the_architecture_document_justifies_each_top_level_module() {
    let rows = documented_modules(&architecture_document());
    for (module, reason) in &rows {
        assert!(
            reason.chars().filter(|c| c.is_alphabetic()).count() >= 10,
            "the row for `{module}` needs a one-line reason, not `{reason}`"
        );
    }
    let documented: BTreeSet<String> = rows.iter().map(|(module, _)| module.clone()).collect();
    assert_eq!(
        documented.len(),
        rows.len(),
        "a module has two rows in the table in docs/ARCHITECTURE.md section 1.1"
    );
    assert_eq!(
        documented,
        allowed_files(),
        "the table in docs/ARCHITECTURE.md section 1.1 and the allowlist in this test differ: \
         only documented: [{}]; only allowed: [{}]",
        listed(&documented.difference(&allowed_files()).cloned().collect()),
        listed(&allowed_files().difference(&documented).cloned().collect())
    );
}

// ---- the readers ---------------------------------------------------------

#[test]
fn lib_rs_is_read_the_same_with_unix_and_windows_line_ends() {
    let lf =
        "//! docs\n\npub mod audio;\n#[cfg(any(test, feature = \"x\"))]\npub mod test_support;\n\
              mod private;\n// old paths\npub use crate::meeting::media as meeting_media;\n\
              pub use crate::diarization::speaker_hint;\n";
    let crlf = lf.replace('\n', "\r\n");
    for text in [lf, crlf.as_str()] {
        let (modules, reexports) = lib_declarations(text);
        assert_eq!(modules, set(&["audio", "private", "test_support"]));
        assert_eq!(reexports, set(&["meeting_media", "speaker_hint"]));
    }
}

#[test]
fn a_re_export_the_reader_cannot_see_is_refused_not_skipped() {
    for line in [
        "pub use crate::{a::b as c, d};",
        "pub use crate::a::*;",
        "pub use other::a;",
    ] {
        let result = std::panic::catch_unwind(|| lib_declarations(line));
        assert!(result.is_err(), "`{line}` must not be skipped silently");
    }
}

#[test]
fn the_table_is_read_the_same_with_unix_and_windows_line_ends() {
    let lf = format!(
        "intro\n\n{TABLE_HEADER}\n|---|---|\n| `lib` | the crate root and its list of modules |\n\
         | `error` | the one error type every subsystem returns |\n\nafter the table\n"
    );
    let crlf = lf.replace('\n', "\r\n");
    for text in [lf.as_str(), crlf.as_str()] {
        let rows = documented_modules(text);
        let names: Vec<&str> = rows.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["lib", "error"]);
        assert!(rows[0].1.starts_with("the crate root"));
    }
}

#[test]
fn test_files_do_not_count_as_top_level_modules() {
    let dir = sagascript_testsupport::TempDir::new();
    fs::create_dir_all(dir.path().join("sub")).unwrap();
    for file in ["a.rs", "a_tests.rs", "b.rs", "notes.md"] {
        fs::write(dir.path().join(file), "").unwrap();
    }
    let (files, directories) = files_and_directories(dir.path());
    assert_eq!(files, set(&["a", "b"]));
    assert_eq!(directories, set(&["sub"]));
}
