//! The module layering of `sagascript-core` (#372), guarded as the text of the
//! source files, since rustc has no notion of an upward `use`.
//!
//! ```text
//! models, glossary   leaves: depend on neither settings nor transcription
//!        ^
//!     settings       reads the model catalog; the machine is injected
//!        ^
//!  transcription     takes settings from its callers (the engine host config)
//!        ^
//!     machine        composes settings with transcription's probes
//! ```
//!
//! # The one upward line
//!
//! `settings/store.rs` re-exports `machine::{load, update, try_update}`:
//!
//! ```text
//! pub use crate::machine::{load, try_update, update};
//! ```
//!
//! It is a shim for the old paths `settings::store::{load, update, try_update}`,
//! kept for one release for callers outside the workspace (nothing in it uses
//! them: `nothing_in_the_workspace_uses_a_compatibility_path`), and it is an edge
//! from settings up to the layer that imports transcription. It is no longer a
//! loop at run time: `transcription::pianissimo_backend::{engine_config,
//! shared_client}` used to call `settings::store::load()`, so transcription
//! reached `machine::load()` and `machine` reached transcription's probes
//! again. They now receive the `Settings` (or, for the two deferred callers, the
//! function that loads them), and only `settings/` and `machine.rs` name the
//! store at all (`only_settings_and_the_machine_name_the_store`). Until the line
//! is deleted, `only_the_compat_shim_in_the_store_names_the_machine_module` lets
//! it name exactly these three paths, so no other `machine` import can slip in
//! beside them.
//!
//! # What the scan sees
//!
//! Every path the code of a file names, after comments and string literals are
//! dropped: `use` declarations are expanded (nested groups, `as` renames,
//! `self`, globs) into one full path per name, and the rest of the code is read
//! as `a::b::c` chains. A rename (`use crate::{machine as m}`) is therefore
//! seen at the `use`, whatever the alias is called later, and the bare module
//! names (`settings::..`, as after `use crate::*`) count as well as the
//! `crate::..` paths. `\n` and `\r\n` line ends read alike.

use std::fs;
use std::path::{Path, PathBuf};

fn src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

// ---- reading Rust source -------------------------------------------------

/// The source with comments and the contents of string and character
/// literals blanked out (each becomes one space), so that neither text in a
/// doc comment nor `"crate::settings"` in a message is read as a path.
fn code_only(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            out.push(' ');
        } else if c == '/' && next == Some('*') {
            // Block comments nest.
            let mut depth = 1;
            i += 2;
            while i < chars.len() && depth > 0 {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            out.push(' ');
        } else if c == 'r' && matches!(next, Some('"') | Some('#')) && !prev_is_ident(&chars, i) {
            // Raw string: r"..", r#".."#.
            let mut j = i + 1;
            let mut hashes = 0;
            while chars.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if chars.get(j) == Some(&'"') {
                j += 1;
                while j < chars.len() {
                    if chars[j] == '"' && (0..hashes).all(|k| chars.get(j + 1 + k) == Some(&'#')) {
                        j += 1 + hashes;
                        break;
                    }
                    j += 1;
                }
                i = j;
                out.push(' ');
            } else {
                out.push(c);
                i += 1;
            }
        } else if c == '"' {
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                i += if chars[i] == '\\' { 2 } else { 1 };
            }
            i += 1;
            out.push(' ');
        } else if c == '\'' && is_char_literal(&chars, i) {
            i += 1;
            while i < chars.len() && chars[i] != '\'' {
                i += if chars[i] == '\\' { 2 } else { 1 };
            }
            i += 1;
            out.push(' ');
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

fn prev_is_ident(chars: &[char], i: usize) -> bool {
    i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_')
}

/// `'x'` and `'\n'` are literals; `'a` in `&'a str` is a lifetime.
fn is_char_literal(chars: &[char], i: usize) -> bool {
    match chars.get(i + 1) {
        Some('\\') => true,
        Some(_) => chars.get(i + 2) == Some(&'\''),
        None => false,
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    PathSep,
    LBrace,
    RBrace,
    Comma,
    Star,
    Other,
}

fn tokens(code: &str) -> Vec<Token> {
    let chars: Vec<char> = code.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_alphanumeric() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Token::Ident(chars[start..i].iter().collect()));
            continue;
        }
        match c {
            ':' if chars.get(i + 1) == Some(&':') => {
                out.push(Token::PathSep);
                i += 2;
                continue;
            }
            '{' => out.push(Token::LBrace),
            '}' => out.push(Token::RBrace),
            ',' => out.push(Token::Comma),
            '*' => out.push(Token::Star),
            c if c.is_whitespace() => {}
            _ => out.push(Token::Other),
        }
        i += 1;
    }
    out
}

fn join(path: &mut String, segment: &str) {
    if !path.is_empty() {
        path.push_str("::");
    }
    path.push_str(segment);
}

/// One `use` tree starting at `i`, appended to `out` as full paths. Returns
/// the index after it. `prefix` is the path of the enclosing group.
fn use_tree(t: &[Token], mut i: usize, prefix: &str, out: &mut Vec<String>) -> usize {
    let mut path = prefix.to_string();
    if t.get(i) == Some(&Token::PathSep) {
        i += 1; // `use ::std::..`
    }
    while let Some(Token::Ident(name)) = t.get(i) {
        if name != "self" {
            join(&mut path, name);
        }
        i += 1;
        if t.get(i) != Some(&Token::PathSep) {
            break;
        }
        i += 1;
        match t.get(i) {
            Some(Token::LBrace) => {
                i += 1;
                loop {
                    i = use_tree(t, i, &path, out);
                    match t.get(i) {
                        Some(Token::Comma) => {
                            i += 1;
                            if t.get(i) == Some(&Token::RBrace) {
                                return i + 1;
                            }
                        }
                        Some(Token::RBrace) => return i + 1,
                        _ => return i,
                    }
                }
            }
            Some(Token::Star) => {
                join(&mut path, "*");
                i += 1;
                break;
            }
            _ => {}
        }
    }
    // `.. as alias`: the alias is a local name; the path is what was imported.
    if matches!(t.get(i), Some(Token::Ident(word)) if word == "as") {
        i += 2;
    }
    out.push(path);
    i
}

/// Every path a source text names, as `a::b::c` strings: the expansion of each
/// `use` declaration, then the `::` chains of the remaining code.
fn paths(source: &str) -> Vec<String> {
    let t = tokens(&code_only(source));
    let mut out = Vec::new();
    let mut rest: Vec<Token> = Vec::new();
    let mut i = 0;
    while i < t.len() {
        let starts_use = matches!(&t[i], Token::Ident(word) if word == "use")
            && matches!(t.get(i + 1), Some(Token::Ident(_)) | Some(Token::PathSep));
        if starts_use {
            i = use_tree(&t, i + 1, "", &mut out);
        } else {
            rest.push(t[i].clone());
            i += 1;
        }
    }
    let mut j = 0;
    while j < rest.len() {
        let Token::Ident(first) = &rest[j] else {
            j += 1;
            continue;
        };
        let mut chain = first.clone();
        let mut k = j + 1;
        while rest.get(k) == Some(&Token::PathSep) {
            match rest.get(k + 1) {
                Some(Token::Ident(segment)) => {
                    chain.push_str("::");
                    chain.push_str(segment);
                    k += 2;
                }
                Some(Token::Star) => {
                    chain.push_str("::*");
                    k += 2;
                    break;
                }
                _ => break,
            }
        }
        if chain.contains("::") {
            out.push(chain);
        }
        j = k;
    }
    out
}

/// Whether `path` names `module` of this crate: `crate::module..`, or the bare
/// `module::..` (as after `use crate::*`).
fn names_module(path: &str, module: &str) -> bool {
    let mut segments = path.split("::");
    match segments.next() {
        Some("crate") => segments.next() == Some(module),
        Some(first) => first == module,
        None => false,
    }
}

/// Paths that take the crate root as a whole or go two levels up, which would
/// hide which module is named.
fn reaches_the_root(path: &str) -> bool {
    path == "crate" || path == "crate::*" || path.starts_with("super::super")
}

// ---- reading the tree ----------------------------------------------------

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

/// `(file relative to src, path)` for every path named under `src/<dir>`
/// (`""`: all of src; `"models"`: the directory; `"glossary"`: `glossary.rs`).
fn tree_paths(dir: &str) -> Vec<(String, String)> {
    let root = src();
    let start = root.join(dir);
    let mut files = Vec::new();
    if start.is_dir() {
        rust_files(&start, &mut files);
    } else {
        files.push(root.join(format!("{dir}.rs")));
    }
    files.sort();
    let mut out = Vec::new();
    for file in files {
        let name = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        for path in paths(&fs::read_to_string(&file).unwrap()) {
            out.push((name.clone(), path));
        }
    }
    out
}

fn report(found: &[(String, String)]) -> String {
    found
        .iter()
        .map(|(file, path)| format!("{file}: {path}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The paths under `src/<dir>` that name one of `modules` or reach the root.
fn offenders(dir: &str, modules: &[&str]) -> Vec<(String, String)> {
    tree_paths(dir)
        .into_iter()
        .filter(|(_, path)| {
            reaches_the_root(path) || modules.iter().any(|module| names_module(path, module))
        })
        .collect()
}

// ---- the layering --------------------------------------------------------

#[test]
fn the_model_catalog_depends_on_neither_settings_nor_transcription() {
    let found = offenders("models", &["settings", "transcription", "machine"]);
    assert!(
        found.is_empty(),
        "models must stay a leaf:\n{}",
        report(&found)
    );
}

#[test]
fn the_glossary_is_a_leaf() {
    let found: Vec<_> = tree_paths("glossary")
        .into_iter()
        .filter(|(_, path)| path.starts_with("crate") || reaches_the_root(path))
        .collect();
    assert!(
        found.is_empty(),
        "glossary must stay a leaf:\n{}",
        report(&found)
    );
}

#[test]
fn settings_do_not_reach_into_transcription() {
    let found = offenders("settings", &["transcription"]);
    assert!(
        found.is_empty(),
        "settings must get machine state injected, not probe it:\n{}",
        report(&found)
    );
}

/// The one upward line (see the module documentation): the paths of the
/// shim, and nothing else, may name `machine` anywhere in the crate.
#[test]
fn only_the_compat_shim_in_the_store_names_the_machine_module() {
    let found: Vec<_> = tree_paths("")
        .into_iter()
        .filter(|(_, path)| names_module(path, "machine"))
        .collect();
    let expected: Vec<(String, String)> = ["load", "try_update", "update"]
        .map(|name| {
            (
                "settings/store.rs".to_string(),
                format!("crate::machine::{name}"),
            )
        })
        .to_vec();
    assert_eq!(
        found,
        expected,
        "nothing but `pub use crate::machine::{{load, try_update, update}};` in settings/store.rs may \
         import `machine`, which sits above settings and transcription. Removing it is a deliberate \
         step: see the module documentation of this test for what must change first."
    );
}

/// Whether `path` is `crate::a::b..` or the bare `a::b..` for the given modules.
fn names_path(path: &str, modules: &[&str]) -> bool {
    let mut segments = path.split("::").peekable();
    if segments.peek() == Some(&"crate") {
        segments.next();
    }
    modules
        .iter()
        .all(|module| segments.next() == Some(*module))
}

/// Settings are loaded by their callers, never by the code that needs them to
/// configure itself: outside `settings/` and `machine.rs` (which composes the
/// store with this machine's probes) no module of core names the store. That is
/// what `transcription` did in `pianissimo_backend` until #372 part 2b, which
/// closed a loop at run time through `machine`.
#[test]
fn only_settings_and_the_machine_name_the_store() {
    let found: Vec<_> = tree_paths("")
        .into_iter()
        .filter(|(file, path)| {
            !file.starts_with("settings/")
                && file != "machine.rs"
                && file != "machine_tests.rs"
                && names_path(path, &["settings", "store"])
        })
        .collect();
    assert!(
        found.is_empty(),
        "the settings store is named only by settings/ and machine.rs; take a `&Settings` from the \
         caller (which loads it with `machine::load`) instead:\n{}",
        report(&found)
    );
}

// ---- the compatibility paths have no users -------------------------------

/// The old names `lib.rs` re-exports (`pub use crate::path as name;`, or the
/// last segment without an `as`). Read from the source, so a new compatibility
/// line is covered the day it is added and a removed one leaves the scan.
fn lib_shims() -> Vec<String> {
    let lib = fs::read_to_string(src().join("lib.rs")).unwrap();
    let shims: Vec<String> = lib
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("pub use crate::"))
        .map(|rest| rest.trim_end_matches(';').trim())
        .map(|path| match path.split_once(" as ") {
            Some((_, alias)) => alias.trim(),
            None => path.rsplit("::").next().unwrap(),
        })
        .map(str::to_string)
        .collect();
    assert!(
        !shims.is_empty(),
        "lib.rs has no `pub use crate::..;` lines; if the last compatibility path is gone, \
         delete this check and the scan that uses lib_shims()"
    );
    shims
}

/// The functions `settings/store.rs` re-exports from `machine`: the old
/// `settings::store::{load, update, try_update}`.
fn store_shims() -> Vec<String> {
    let store = fs::read_to_string(src().join("settings/store.rs")).unwrap();
    paths(&store)
        .into_iter()
        .filter_map(|path| path.strip_prefix("crate::machine::").map(str::to_string))
        .collect()
}

/// What the scan of one file needs to know about the compatibility paths.
struct Shims {
    /// Old top-level module names (`meeting_review`, `speaker_hint`, ..).
    modules: Vec<String>,
    /// Old function names under `settings::store` (`load`, `update`, ..).
    store_functions: Vec<String>,
}

fn shims() -> Shims {
    Shims {
        modules: lib_shims(),
        store_functions: store_shims(),
    }
}

/// The compatibility paths a source file names, as `path` strings.
///
/// `in_core` is a file of `sagascript-core/src`, where the old module names are
/// reached as `crate::name` (a bare `speaker_hint::..` is the real module in
/// `diarization/`); any other file reaches core as `sagascript_core::name`, and
/// a bare `meeting_media::..` there is the app's own module of that name.
///
/// A path counts when it is:
/// - `crate::<old module>..` (core) or `sagascript_core::<old module>..`;
/// - `settings::store::<load|update|try_update>` however the prefix is spelt,
///   or `store::<..>` in a file that imports `sagascript_core::settings::store`;
/// - a rename of the store module itself (`.. store as s`), which would hide
///   the later calls from the second rule.
///
/// Not seen: `use sagascript_core as core;` and the like; nobody renames the crate.
fn compat_uses(source: &str, in_core: bool, shims: &Shims) -> Vec<String> {
    let all = paths(source);
    let imports_store = all
        .iter()
        .any(|path| path == "sagascript_core::settings::store" || path == "crate::settings::store");
    let mut found = Vec::new();
    for path in &all {
        let segments: Vec<&str> = path.split("::").collect();
        let module_prefix = if in_core { "crate" } else { "sagascript_core" };
        let old_module = segments.first() == Some(&module_prefix)
            && segments
                .get(1)
                .is_some_and(|name| shims.modules.iter().any(|shim| shim == name));
        let store_function = |from: usize| {
            segments.get(from) == Some(&"store")
                && segments
                    .get(from + 1)
                    .is_some_and(|name| shims.store_functions.iter().any(|shim| shim == name))
        };
        let old_store_function = (store_function(0) && imports_store)
            || (1..segments.len())
                .any(|from| segments[from - 1] == "settings" && store_function(from));
        if old_module || old_store_function {
            found.push(path.clone());
        }
    }
    let tokens = tokens(&code_only(source));
    let renamed_store = imports_store
        && tokens.windows(2).any(|pair| {
            matches!(pair, [Token::Ident(first), Token::Ident(second)] if first == "store" && second == "as")
        });
    if renamed_store {
        found.push("settings::store as <alias>".to_string());
    }
    found
}

/// Every `.rs` file of the workspace that is Rust the repository maintains:
/// the app (`src-tauri/src`) and the member crates (`src-tauri/crates`), as
/// `(path relative to src-tauri with / separators, text)`.
fn workspace_sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    rust_files(&root.join("crates"), &mut files);
    files.sort();
    files
        .into_iter()
        .filter(|path| !path.components().any(|part| part.as_os_str() == "target"))
        .map(|path| {
            let name = path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let text = fs::read_to_string(&path).unwrap();
            (name, text)
        })
        .collect()
}

/// The files that define the compatibility paths; everything else must not use them.
const SHIM_DEFINITIONS: [&str; 2] = [
    "crates/sagascript-core/src/lib.rs",
    "crates/sagascript-core/src/settings/store.rs",
];

/// Core files that still use a compatibility path, until their own change lands:
/// `(file, path)`. Empty: nothing is left.
const STILL_USING_A_COMPAT_PATH: [(&str, &str); 0] = [];

/// "No internal users" (#372, part 2b): the CLI, the app, the tests and the
/// rest of core name the new paths, so a compatibility path can be deleted
/// without touching them and a new caller cannot start leaning on one.
#[test]
fn nothing_in_the_workspace_uses_a_compatibility_path() {
    let shims = shims();
    let mut found = Vec::new();
    for (name, text) in workspace_sources() {
        if SHIM_DEFINITIONS.contains(&name.as_str()) {
            continue;
        }
        let in_core = name.starts_with("crates/sagascript-core/src/");
        for path in compat_uses(&text, in_core, &shims) {
            found.push((name.clone(), path));
        }
    }
    found.sort();
    found.dedup();
    let expected: Vec<(String, String)> = STILL_USING_A_COMPAT_PATH
        .iter()
        .map(|(file, path)| (file.to_string(), path.to_string()))
        .collect();
    assert_eq!(
        found,
        expected,
        "a compatibility path (an old `sagascript_core::meeting_review`-style module name from \
         lib.rs, or `settings::store::{{load, update, try_update}}`) is used outside the files \
         that define it. Use the new path: `meeting::..`, `diarization::..` or \
         `machine::{{load, update, try_update}}`. The files that define them are exempt: {}",
        SHIM_DEFINITIONS.join(", ")
    );
}

// ---- the scan itself -----------------------------------------------------

fn sorted(mut paths: Vec<String>) -> Vec<String> {
    paths.sort();
    paths
}

fn strings(paths: &[&str]) -> Vec<String> {
    sorted(paths.iter().map(|path| path.to_string()).collect())
}

#[test]
fn use_declarations_expand_groups_nested_groups_renames_self_and_globs() {
    let source = "use crate::{machine as this_machine, models as _};\n\
                  use crate::{a::{b, c as d, self}, e::*};\n\
                  pub(crate) use ::std::{fmt, io::{self, Read}};\n\
                  use super::super::settings;\n\
                  use crate::*;\n";
    assert_eq!(
        sorted(paths(source)),
        strings(&[
            "crate::machine",
            "crate::models",
            "crate::a::b",
            "crate::a::c",
            "crate::a",
            "crate::e::*",
            "std::fmt",
            "std::io",
            "std::io::Read",
            "super::super::settings",
            "crate::*",
        ])
    );
}

#[test]
fn code_chains_are_paths_and_comments_strings_and_lifetimes_are_not() {
    let source = "// use crate::machine::load;\n\
                  /* crate::machine::load /* nested crate::machine::x */ */\n\
                  /// crate::machine::doc\n\
                  fn f<'a>(x: &'a str) -> char {\n\
                  \tlet s = \"crate::machine::in_string\"; let r = r#\"crate::machine::raw\"#;\n\
                  \tlet c = ':'; let d = '\\'';\n\
                  \tsettings::store::load(); crate::models::Language::default(); Self::x(); x.y();\n\
                  \t'x'\n\
                  }\n";
    assert_eq!(
        sorted(paths(source)),
        strings(&[
            "settings::store::load",
            "crate::models::Language::default",
            "Self::x",
        ])
    );
}

#[test]
fn the_scan_reads_crlf_like_lf() {
    let lf = "use crate::{\n    machine as m,\n    models,\n};\n// c\nfn f() { m::x(); }\n";
    let crlf = lf.replace('\n', "\r\n");
    assert_eq!(paths(lf), paths(&crlf));
    assert!(paths(&crlf).contains(&"crate::machine".to_string()));
}

/// Two misses of the first version of this test, which looked for the
/// substrings `crate::settings`, `crate::transcription` and `crate::machine`.
#[test]
fn a_grouped_or_renamed_import_of_a_forbidden_module_is_seen() {
    // settings/store.rs: `machine` imported under another name, then used.
    let store = "use crate::{machine as this_machine, models as _};\n\
                 fn load() { this_machine::ThisMachine; }\n";
    let found: Vec<String> = paths(store)
        .into_iter()
        .filter(|path| names_module(path, "machine"))
        .collect();
    assert_eq!(found, ["crate::machine"], "the shim test must flag it");

    // models/sources.rs: `settings` imported under another name, then used.
    let sources = "use crate::{settings as _s, download as _d};\n\
                   fn f() { _s::EnginePrewarm::Off; }\n";
    let found: Vec<String> = paths(sources)
        .into_iter()
        .filter(|path| {
            ["settings", "transcription", "machine"]
                .iter()
                .any(|module| names_module(path, module))
        })
        .collect();
    assert_eq!(
        found,
        ["crate::settings"],
        "the models-leaf test must flag it"
    );

    // After `use crate::*` the modules are named bare.
    let bare = paths("use crate::*;\nfn f() { machine::load(); }\n");
    assert!(bare.iter().any(|path| names_module(path, "machine")));
    assert!(bare.iter().any(|path| reaches_the_root(path)));
}

// ---- the scan for compatibility paths itself -------------------------------

fn test_shims() -> Shims {
    Shims {
        modules: ["meeting_review", "speaker_hint", "diarization_report"]
            .map(str::to_string)
            .to_vec(),
        store_functions: ["load", "update", "try_update"]
            .map(str::to_string)
            .to_vec(),
    }
}

fn uses(source: &str, in_core: bool) -> Vec<String> {
    sorted(compat_uses(source, in_core, &test_shims()))
}

/// The mutation of the real check: each of these makes it red.
#[test]
fn a_use_of_an_old_module_or_store_path_is_seen_however_it_is_spelt() {
    // Plain, grouped, nested and renamed imports of an old module.
    assert_eq!(
        uses("use sagascript_core::meeting_review::MeetingReview;", false),
        strings(&["sagascript_core::meeting_review::MeetingReview"])
    );
    assert_eq!(
        uses(
            "use sagascript_core::{meeting_review as r, diarization_report::{DecoderEvidence, self}};",
            false
        ),
        strings(&[
            "sagascript_core::diarization_report",
            "sagascript_core::diarization_report::DecoderEvidence",
            "sagascript_core::meeting_review",
        ])
    );
    // In code, without an import.
    assert_eq!(
        uses(
            "fn f() -> sagascript_core::speaker_hint::SpeakerCountHint { todo!() }",
            false
        ),
        strings(&["sagascript_core::speaker_hint::SpeakerCountHint"])
    );
    // Inside core the old name is `crate::name`.
    assert_eq!(
        uses("use crate::speaker_hint::SpeakerCountHint;", true),
        strings(&["crate::speaker_hint::SpeakerCountHint"])
    );
    // The store functions: full path, through `settings`, through `store`.
    assert_eq!(
        uses(
            "fn f() { sagascript_core::settings::store::load(); }",
            false
        ),
        strings(&["sagascript_core::settings::store::load"])
    );
    assert_eq!(
        uses(
            "use sagascript_core::settings;\nfn f() { settings::store::try_update(|_| {}); }",
            false
        ),
        strings(&["settings::store::try_update"])
    );
    assert_eq!(
        uses(
            "use sagascript_core::settings::{store, Settings};\nfn f() { store::update(|_| {}); }",
            false
        ),
        strings(&["store::update"])
    );
    assert_eq!(
        uses(
            "use sagascript_core::settings::store::{load, save_to};",
            false
        ),
        strings(&["sagascript_core::settings::store::load"])
    );
    // A rename of the module hides the calls, so the rename is the finding.
    let renamed = uses(
        "use sagascript_core::settings::store as s;\nfn f() { s::load(); }",
        false,
    );
    assert_eq!(renamed, strings(&["settings::store as <alias>"]));
}

#[test]
fn the_new_paths_and_unrelated_names_are_not_compatibility_uses() {
    for source in [
        // The new paths.
        "use sagascript_core::meeting::review::MeetingReview;",
        "use sagascript_core::diarization::speaker_hint::SpeakerCountHint;",
        "use sagascript_core::machine::{load, update};",
        // The app's own module of the same name, and a field of the same name.
        "mod meeting_review; use crate::meeting_review::x; use meeting_review::y;",
        "struct S { speaker_hint: u8 } fn f(s: S) -> u8 { s.speaker_hint }",
        // Inside core, `speaker_hint` is the module of `diarization/`.
        "pub use speaker_hint::{SpeakerCountHint, SpeakerHintOutcome};",
        // Store functions that are not the old three, and `store` of another crate.
        "use sagascript_core::settings::store;\nfn f() { store::load_from(p); store::load_with(m); }",
        "fn f() { other::store::load(); store::update(1); }",
    ] {
        assert!(uses(source, false).is_empty(), "wrongly flagged: {source}");
    }
    assert!(uses("pub use speaker_hint::SpeakerCountHint;", true).is_empty());
    // Comments and strings (the contract scanner's fixtures) are not code.
    let text = "// sagascript_core::speaker_hint::X\n\
                /// settings::store::load\n\
                fn f() { let _ = \"sagascript_core::meeting_review::M\"; \
                let _ = r#\"use sagascript_core::settings::store::update;\"#; }\n";
    assert!(uses(text, false).is_empty());
}

#[test]
fn the_compatibility_scan_reads_crlf_like_lf() {
    let lf = "use sagascript_core::{\n    meeting_review::MeetingReview,\n    machine,\n};\n\
              use sagascript_core::settings::store;\nfn f() {\n    store::load();\n}\n";
    let crlf = lf.replace('\n', "\r\n");
    let found = uses(&crlf, false);
    assert_eq!(found, uses(lf, false));
    assert_eq!(
        found,
        strings(&[
            "sagascript_core::meeting_review::MeetingReview",
            "store::load"
        ])
    );
}

/// `lib.rs` is the source of the module list: its lines in either spelling.
#[test]
fn the_shim_lists_come_from_lib_rs_and_the_store() {
    let modules = lib_shims();
    for name in ["meeting_review", "speaker_hint", "diarization_report"] {
        assert!(
            modules.iter().any(|shim| shim == name),
            "{name} in {modules:?}"
        );
    }
    assert_eq!(
        sorted(store_shims()),
        strings(&["load", "try_update", "update"])
    );
}
