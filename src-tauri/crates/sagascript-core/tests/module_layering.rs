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
//! # The one upward line, and why it exists
//!
//! `settings/store.rs` re-exports `machine::{load, update, try_update}`:
//!
//! ```text
//! pub use crate::machine::{load, try_update, update};
//! ```
//!
//! It is a shim for the old paths `settings::store::{load, update, try_update}`,
//! which the CLI and the app still call, and it is an edge from settings up to
//! the layer that imports transcription. Behind it is a second, runtime edge:
//! `transcription::pianissimo_backend::{engine_config, shared_client}` call
//! `settings::store::load()`, so transcription reaches `machine::load()` and
//! `machine` reaches transcription's probes again. Removing the line is a
//! deliberate step, not a clean-up: first the CLI and the app call
//! `machine::*`, and those two functions receive the `Settings` they read
//! (which changes *when* the settings are read: behaviour, so a separate
//! change). Then the expected list in the last layering test becomes empty.
//! Until then it must name exactly these three paths, so no other `machine`
//! import can slip in beside them.
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
