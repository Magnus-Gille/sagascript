//! Removing absolute file paths from error text (ADR 0006, #360).
//!
//! Text that comes from outside Rust (the Swift engine host's messages and
//! stderr, OS API errors) names files by absolute path: `Failed to load model at
//! /Users/<name>/Library/Application Support/Sagascript/models/x.mlmodelc`. Such
//! text must not reach the app log or the UI, because the path carries the
//! user's account name and the names of their folders. [`scrub_paths`] is the
//! one function that does it, where the text enters a Rust error type
//! ([`crate::error::DictationError::engine`]) and again when the app serializes
//! a command error.
//!
//! A path becomes [`PLACEHOLDER`], followed by its file name when the path goes
//! deeper than the account folder: `/Users/x/Documents/a.m4a` is
//! `<path>/a.m4a`, `/Users/x` is `<path>`. The file name stays because it is
//! what the user (and a bug report) needs to tell which file failed; every
//! folder before it, however many words its name has, is removed.
//!
//! Where a path ends is decided from the text alone: at a quote, bracket,
//! comma, semicolon, line break, a colon followed by a space, or another
//! path. A space continues the path while a later word, before the clause
//! ends, still contains a separator (`Application Support/Sagascript`), so
//! folder names with spaces are consumed whole.
//!
//! What it recognises, and so what the account name is guaranteed to be
//! removed from: the roots in `ROOTS` (macOS `/Users`, `/Volumes`, `/private`,
//! Linux `/home`, `/root`, `/var`, `/tmp`, matched ASCII case-insensitively
//! because the macOS file system is), their `/System/Volumes/Data/…` spelling,
//! `file://` and `file://localhost` URLs, Windows drive, UNC and `\\?\` /
//! `\\.\` paths, and the home directory the process reports. Not recognised
//! (non-goals): a path JSON-escaped as `\/Users\/zoe`, a percent-encoded
//! `%2FUsers%2F`, `~zoe/…`, a relative path, and a home directory the
//! process does not know about.

use std::sync::OnceLock;

/// What a removed path leaves behind.
pub const PLACEHOLDER: &str = "<path>";

/// Roots that identify a path without needing the user's home directory, with
/// the number of leading components that can identify the user or machine
/// (`/Users/x` is two). The file name is only kept below that depth.
const ROOTS: &[(&str, usize)] = &[
    ("/Users/", 2),
    ("/home/", 2),
    ("/root/", 1),
    ("/Volumes/", 2),
    ("/private/", 1),
    ("/var/", 1),
    ("/tmp/", 1),
];

/// A drive-letter path (`C:\…`, `C:/…`) is user-specific on every drive.
const DRIVE_DEPTH: usize = 3;

/// The macOS spelling of the folders below it: `/System/Volumes/Data/Users/x`
/// is `/Users/x`, three components deeper.
const FIRMLINK_PREFIX: &str = "/System/Volumes/Data";
const FIRMLINK_DEPTH: usize = 3;

/// `text` with every absolute path replaced as described in the module
/// documentation. Idempotent. Recognises the macOS and Linux account folders,
/// volumes and temporary folders, Windows drive and UNC paths, `file://` URLs
/// and the current user's home directory.
pub fn scrub_paths(text: &str) -> String {
    scrub_paths_with_home(text, home_dir_text())
}

fn home_dir_text() -> Option<&'static str> {
    static HOME: OnceLock<Option<String>> = OnceLock::new();
    HOME.get_or_init(|| {
        dirs::home_dir()
            .map(|home| home.to_string_lossy().into_owned())
            // A home of `/` or a bare drive would scrub every path.
            .filter(|home| home.chars().filter(|c| is_separator(*c)).count() >= 2)
    })
    .as_deref()
}

/// [`scrub_paths`] with an explicit home directory, for tests.
pub fn scrub_paths_with_home(text: &str, home: Option<&str>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        let Some(c) = text[at..].chars().next() else {
            break;
        };
        match root_at(text, at, home) {
            Some(root) => {
                let end = path_end(text, at + root.skip, home);
                let (path, tail) = split_trailing_punctuation(&text[at..end]);
                out.push_str(&replacement(&path[root.skip.min(path.len())..], root.depth));
                out.push_str(tail);
                at = end;
            }
            None => {
                out.push(c);
                at += c.len_utf8();
            }
        }
    }
    out
}

fn is_separator(c: char) -> bool {
    c == '/' || c == '\\'
}

/// Characters that end a path for good. A space only ends it when no further
/// separator follows before the clause ends (see the module documentation).
fn is_hard_terminator(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '"' | '\''
                | '`'
                | '<'
                | '>'
                | '|'
                | '*'
                | '?'
                | ';'
                | ','
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '“'
                | '”'
                | '‘'
                | '’'
        )
}

/// A recognised root: how many leading components identify the user, and how
/// many bytes at its start (`\\?\`, the slash of `/C:/`) are a prefix that is not
/// part of the path proper.
#[derive(Clone, Copy)]
struct Root {
    depth: usize,
    skip: usize,
}

fn starts_with_ci(text: &str, prefix: &str) -> bool {
    text.len() >= prefix.len()
        && text.is_char_boundary(prefix.len())
        && text[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn ends_with_ci(text: &str, suffix: &str) -> bool {
    text.len() >= suffix.len()
        && text.is_char_boundary(text.len() - suffix.len())
        && text[text.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

/// Whether the text at `at` starts a path root.
fn root_at(text: &str, at: usize, home: Option<&str>) -> Option<Root> {
    let rest = &text[at..];
    let before = text[..at].chars().next_back();
    let after_file_url =
        ends_with_ci(&text[..at], "file://") || ends_with_ci(&text[..at], "file://localhost");
    let starts_word = after_file_url
        || match before {
            None => true,
            Some(c) if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '~' | '%') => false,
            // `file:///Users/…` is handled above; any other slash continues a path.
            Some('/') => false,
            Some(_) => true,
        };
    if !starts_word {
        return None;
    }
    if after_file_url {
        // `file:///C:/Users/…`: the slash before the drive letter belongs to the URL.
        let mut chars = rest.chars();
        if let (Some('/'), Some(letter), Some(':'), Some(separator)) =
            (chars.next(), chars.next(), chars.next(), chars.next())
        {
            if letter.is_ascii_alphabetic() && is_separator(separator) {
                return Some(Root {
                    depth: DRIVE_DEPTH,
                    skip: 1,
                });
            }
        }
    }
    known_root(rest, home)
}

fn known_root(rest: &str, home: Option<&str>) -> Option<Root> {
    let plain = |depth| Some(Root { depth, skip: 0 });
    if let Some(home) = home {
        if let Some(after) = rest.strip_prefix(home) {
            if ends_component(after) {
                return plain(
                    home.split(is_separator)
                        .filter(|part| !part.is_empty())
                        .count(),
                );
            }
        }
    }
    if starts_with_ci(rest, FIRMLINK_PREFIX) && rest[FIRMLINK_PREFIX.len()..].starts_with('/') {
        let inner = known_root(&rest[FIRMLINK_PREFIX.len()..], home)?;
        return Some(Root {
            depth: inner.depth + FIRMLINK_DEPTH,
            skip: 0,
        });
    }
    for (root, depth) in ROOTS {
        let bare = &root[..root.len() - 1];
        if starts_with_ci(rest, root) || (rest.len() == bare.len() && starts_with_ci(rest, bare)) {
            return plain(*depth);
        }
    }
    let mut chars = rest.chars();
    if let (Some(letter), Some(':'), Some(separator)) = (chars.next(), chars.next(), chars.next()) {
        if letter.is_ascii_alphabetic() && is_separator(separator) {
            return plain(DRIVE_DEPTH);
        }
    }
    // The verbatim (`\\?\`) and device (`\\.\`) prefixes of the paths above.
    for prefix in ["\\\\?\\", "\\\\.\\"] {
        if let Some(after) = rest.strip_prefix(prefix) {
            if after
                .get(..4)
                .is_some_and(|unc| unc.eq_ignore_ascii_case("UNC\\"))
            {
                // `\\?\UNC\server\share\…`: UNC, the server and the share identify it.
                return Some(Root {
                    depth: 3,
                    skip: prefix.len(),
                });
            }
            let mut chars = after.chars();
            if let (Some(letter), Some(':'), Some(separator)) =
                (chars.next(), chars.next(), chars.next())
            {
                if letter.is_ascii_alphabetic() && is_separator(separator) {
                    return Some(Root {
                        depth: DRIVE_DEPTH,
                        skip: prefix.len(),
                    });
                }
            }
        }
    }
    // UNC share: `\\server\share\…` (the server and the share identify it).
    if rest.starts_with("\\\\")
        && rest[2..]
            .chars()
            .next()
            .is_some_and(|c| !is_separator(c) && c != '?' && c != '.')
    {
        return plain(2);
    }
    None
}

/// Whether `after` (the text following a matched folder name) ends that
/// folder's name rather than continuing it (`zoe/…`, `zoe.`, `zoe ` but not
/// `zoella` or `zoe.old`).
fn ends_component(after: &str) -> bool {
    let mut chars = after.chars();
    match chars.next() {
        None => true,
        Some(c) if is_separator(c) || c.is_whitespace() || is_hard_terminator(c) => true,
        Some('.' | ':' | '!') => chars
            .next()
            .is_none_or(|next| next.is_whitespace() || is_hard_terminator(next)),
        Some(_) => false,
    }
}

/// The index just after the path that starts at `start`.
fn path_end(text: &str, start: usize, home: Option<&str>) -> usize {
    let mut end = start;
    loop {
        // Path characters, up to a space, a hard terminator or `:` + space.
        let mut chars = text[end..].char_indices().peekable();
        let mut stop = text.len() - end;
        while let Some((offset, c)) = chars.next() {
            let ends_here = c == ' '
                || is_hard_terminator(c)
                || c.is_whitespace()
                || (c == ':' && chars.peek().is_none_or(|(_, next)| next.is_whitespace()));
            if ends_here {
                stop = offset;
                break;
            }
        }
        end += stop;
        if !text[end..].starts_with(' ') {
            return end;
        }
        match continues_after_space(&text[end + 1..], home) {
            true => end += 1,
            false => return end,
        }
    }
}

/// Whether the words after a space belong to the same path: a later word of
/// the clause contains a separator, and neither the words before it nor that
/// word start a path of their own (`a.bin to /Users/b/c`) or end the clause
/// (a word ending in a colon, `a.m4a: not found`).
fn continues_after_space(rest: &str, home: Option<&str>) -> bool {
    let mut remaining = rest;
    loop {
        let word_end = remaining
            .find(|c: char| c == ' ' || is_hard_terminator(c) || c.is_whitespace())
            .unwrap_or(remaining.len());
        let word = &remaining[..word_end];
        if word.is_empty() || root_at(remaining, 0, home).is_some() {
            return false;
        }
        if word.contains(is_separator) {
            return true;
        }
        if word.ends_with(':') {
            return false;
        }
        match remaining[word_end..].strip_prefix(' ') {
            Some(next) => remaining = next,
            None => return false,
        }
    }
}

/// Splits sentence punctuation off the end of a path (`…/a.bin.`).
fn split_trailing_punctuation(path: &str) -> (&str, &str) {
    let trimmed = path.trim_end_matches(['.', ':', '!', ' ']);
    (trimmed, &path[trimmed.len()..])
}

/// `<path>` or `<path>/file`, keeping the last component when the path goes
/// deeper than the first `identifying_depth` components.
fn replacement(path: &str, identifying_depth: usize) -> String {
    let components: Vec<&str> = path
        .split(is_separator)
        .filter(|part| !part.is_empty())
        .collect();
    // A drive letter ("C:") or a UNC prefix is a component of its own.
    match components.last() {
        Some(last) if components.len() > identifying_depth => format!("{PLACEHOLDER}/{last}"),
        _ => PLACEHOLDER.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scrub(text: &str) -> String {
        scrub_paths_with_home(text, None)
    }

    #[test]
    fn swift_style_model_load_error_loses_the_path_but_keeps_the_file_name() {
        let message = "Failed to load model at /Users/x/Library/Application Support/Sagascript/models/foo.mlmodelc: The file couldn't be opened.";
        assert_eq!(
            scrub(message),
            "Failed to load model at <path>/foo.mlmodelc: The file couldn't be opened."
        );
    }

    #[test]
    fn account_name_never_survives_in_any_position() {
        let user = "zoltan.kovacs";
        let cases = [
            format!("open /Users/{user}/Documents/Client Files/Q3 plan.m4a: not found"),
            format!("open '/Users/{user}/a b/c.bin' failed"),
            format!("open \"/home/{user}/.local/share/sagascript/x.bin\" failed"),
            format!("(/Users/{user}/Library/Caches/x.mlmodelc) missing"),
            format!("x=/Users/{user}/y z=1"),
            format!("file:///Users/{user}/Movies/talk.mov could not be read"),
            format!("C:\\Users\\{user}\\AppData\\Local\\Sagascript\\models\\foo.bin missing"),
            format!("C:/Users/{user}/AppData/Local/x.bin missing"),
            format!("D:\\Projects\\{user}\\notes.wav missing"),
            format!("line one\n/Users/{user}/a.bin\nline three"),
            format!("/Users/{user}"),
            format!("/Users/{user}/"),
            format!("at /home/{user}."),
        ];
        for case in cases {
            let scrubbed = scrub(&case);
            assert!(
                !scrubbed.contains(user),
                "{case:?} still names the account: {scrubbed:?}"
            );
        }
    }

    #[test]
    fn linux_and_temporary_paths() {
        assert_eq!(
            scrub("cannot read /home/alice/.local/share/sagascript/models/a.bin"),
            "cannot read <path>/a.bin"
        );
        assert_eq!(
            scrub("wrote /private/var/folders/zz/abc123/T/tmp.wav ok"),
            "wrote <path>/tmp.wav ok"
        );
        assert_eq!(
            scrub("wrote /var/folders/zz/abc123/T/tmp.wav ok"),
            "wrote <path>/tmp.wav ok"
        );
        assert_eq!(
            scrub("in /tmp/sagascript-1234/x.json."),
            "in <path>/x.json."
        );
        assert_eq!(
            scrub("copy to /Volumes/Client Disk/Audio/a.wav failed"),
            "copy to <path>/a.wav failed"
        );
    }

    #[test]
    fn windows_drive_and_unc_paths() {
        assert_eq!(
            scrub(r"Failed at C:\Users\Alice\AppData\Local\Sagascript\models\foo.bin: denied"),
            "Failed at <path>/foo.bin: denied"
        );
        assert_eq!(
            scrub(r"share \\fileserver\audio\2026\a.wav is gone"),
            "share <path>/a.wav is gone"
        );
    }

    #[test]
    fn a_bare_account_folder_has_no_file_name_to_keep() {
        assert_eq!(scrub("home is /Users/x"), "home is <path>");
        assert_eq!(scrub("home is /home/x/"), "home is <path>");
        assert_eq!(scrub(r"home is C:\Users\x"), "home is <path>");
    }

    #[test]
    fn the_actual_home_directory_is_removed_wherever_it_is() {
        let home = Some("/srv/people/zoe");
        assert_eq!(
            scrub_paths_with_home("open /srv/people/zoe/notes/a.txt: no", home),
            "open <path>/a.txt: no"
        );
        assert_eq!(
            scrub_paths_with_home("home /srv/people/zoe.", home),
            "home <path>."
        );
        // Another account's sibling folder is not the home directory.
        assert_eq!(
            scrub_paths_with_home("see /srv/people/zoella/a.txt", home),
            "see /srv/people/zoella/a.txt"
        );
    }

    #[test]
    fn several_paths_in_one_message() {
        assert_eq!(
            scrub("copy /Users/a/in.wav to /Users/b/out.wav, then stop"),
            "copy <path>/in.wav to <path>/out.wav, then stop"
        );
    }

    #[test]
    fn text_without_a_path_is_unchanged() {
        for text in [
            "Transcription failed: model busy",
            "engine host error [busy]: queue full",
            "use and/or the other",
            "see https://example.com/home/x and https://example.com/Users/y",
            "src/home/x.rs line 3",
            "relative/path/a.bin",
            "50% done, C: drive",
            "http://localhost:8080/a",
            "",
        ] {
            assert_eq!(scrub(text), text);
        }
    }

    #[test]
    fn non_ascii_text_around_and_inside_a_path_survives() {
        assert_eq!(
            scrub("Kunde inte öppna /Users/åsa/Möten/Årsmöte.m4a – försök igen"),
            "Kunde inte öppna <path>/Årsmöte.m4a – försök igen"
        );
    }

    #[test]
    fn scrubbing_twice_changes_nothing() {
        let once =
            scrub("Failed at /Users/x/Library/Application Support/S/a.bin: no (see /home/y/b)");
        assert_eq!(scrub(&once), once);
    }

    #[test]
    fn the_process_home_directory_is_scrubbed_by_the_public_function() {
        if let Some(home) = dirs::home_dir() {
            let home = home.to_string_lossy().into_owned();
            if home.matches(['/', '\\']).count() >= 2 {
                let scrubbed = scrub_paths(&format!("open {home}/secret dir/a.txt: no"));
                assert!(!scrubbed.contains(&home), "{scrubbed}");
                assert!(scrubbed.contains("a.txt"));
            }
        }
    }

    #[test]
    fn folder_names_with_several_words_stay_inside_the_path() {
        assert_eq!(
            scrub("open /Users/x/Movies/Final Cut Pro Library/talk.fcpbundle: failed"),
            "open <path>/talk.fcpbundle: failed"
        );
    }

    #[test]
    fn file_urls_keep_their_scheme_and_lose_the_path() {
        assert_eq!(
            scrub("file:///Users/x/Library/Application%20Support/S/a.mlmodelc could not open"),
            "file://<path>/a.mlmodelc could not open"
        );
    }

    #[test]
    fn a_path_in_curly_quotes_ends_at_the_quote() {
        assert_eq!(
            scrub("The file “/Users/x/Library/Caches/a b/c.bin” couldn’t be opened."),
            "The file “<path>/c.bin” couldn’t be opened."
        );
    }

    #[test]
    fn the_macos_firmlink_spelling_of_a_home_folder_is_a_home_folder() {
        assert_eq!(
            scrub("open /System/Volumes/Data/Users/zoe/Documents/x.bin: no"),
            "open <path>/x.bin: no"
        );
        assert_eq!(
            scrub("open /System/Volumes/Data/home/zoe/x.bin: no"),
            "open <path>/x.bin: no"
        );
        assert_eq!(scrub("home /System/Volumes/Data/Users/zoe"), "home <path>");
        // The Data volume's other folders are not user-specific roots.
        assert_eq!(
            scrub("open /System/Volumes/Data/Applications/x.app"),
            "open /System/Volumes/Data/Applications/x.app"
        );
    }

    #[test]
    fn a_file_url_for_localhost_is_a_file_url() {
        assert_eq!(
            scrub("could not read file://localhost/Users/zoe/x.bin"),
            "could not read file://localhost<path>/x.bin"
        );
        assert_eq!(
            scrub("could not read file:///C:/Users/zoe/x.bin"),
            "could not read file://<path>/x.bin"
        );
    }

    #[test]
    fn a_case_insensitive_file_system_can_spell_the_roots_in_lowercase() {
        assert_eq!(scrub("open /users/zoe/x.wav: no"), "open <path>/x.wav: no");
        assert_eq!(scrub("open /USERS/Zoe/x.wav: no"), "open <path>/x.wav: no");
        assert_eq!(
            scrub("open /volumes/Client Disk/a.wav"),
            "open <path>/a.wav"
        );
        assert_eq!(
            scrub("open /Private/Var/folders/zz/a/T/x.wav"),
            "open <path>/x.wav"
        );
    }

    #[test]
    fn a_long_folder_name_is_not_kept_but_the_file_name_is() {
        assert_eq!(
            scrub("open /Users/zoe/Documents/My Audio Files With Many Words In Name/recording.m4a: no"),
            "open <path>/recording.m4a: no"
        );
        // The path stops at the colon that ends the clause, not at a later slash.
        assert_eq!(
            scrub("open /Users/zoe/Documents/My Meeting.m4a: not found, see logs/app.log"),
            "open <path>/My Meeting.m4a: not found, see logs/app.log"
        );
        assert_eq!(
            scrub("copy /Users/a/in.wav to /Users/b/out.wav, then stop"),
            "copy <path>/in.wav to <path>/out.wav, then stop"
        );
    }

    #[test]
    fn windows_verbatim_and_device_prefixes_are_part_of_the_path() {
        assert_eq!(
            scrub(r"open \\?\C:\Users\zoe\x.bin: no"),
            "open <path>/x.bin: no"
        );
        assert_eq!(
            scrub(r"open \\?\UNC\server\share\dir\x.bin: no"),
            "open <path>/x.bin: no"
        );
        assert_eq!(
            scrub(r"open \\.\C:\Users\zoe\x.bin: no"),
            "open <path>/x.bin: no"
        );
        assert_eq!(scrub(r"home \\?\C:\Users\zoe"), "home <path>");
    }

    #[test]
    fn the_account_name_is_gone_from_every_recognised_spelling() {
        let user = "zoltan.kovacs";
        for case in [
            format!("/System/Volumes/Data/Users/{user}/a b/c.bin"),
            format!("file://localhost/Users/{user}/Movies/x.mov"),
            format!("/users/{user}/x.wav"),
            format!(r"\\?\C:\Users\{user}\AppData\x.bin"),
            format!("/Users/{user}/Documents/My Audio Files With Many Words In Name/r.m4a"),
        ] {
            let scrubbed = scrub(&format!("failed: {case}: no"));
            assert!(!scrubbed.contains(user), "{case:?} -> {scrubbed:?}");
            assert_eq!(scrub(&scrubbed), scrubbed, "idempotent: {scrubbed:?}");
        }
    }
}
