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
//! what the user (and a bug report) needs to tell which file failed.

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

/// How many whitespace-separated words after a space may still belong to the
/// path, looking for the next separator: `Application Support/Sagascript`
/// (1 word) and `Final Cut Pro Library/x` (3 words) stay one path.
const MAX_WORDS_IN_SEGMENT: usize = 3;

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
            Some(depth) => {
                let end = path_end(text, at, home);
                let (path, tail) = split_trailing_punctuation(&text[at..end]);
                out.push_str(&replacement(path, depth));
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
/// separator follows within [`MAX_WORDS_IN_SEGMENT`] words.
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

/// Whether the text at `at` starts a path root, and how many leading
/// components identify the user.
fn root_at(text: &str, at: usize, home: Option<&str>) -> Option<usize> {
    let rest = &text[at..];
    let before = text[..at].chars().next_back();
    let starts_word = match before {
        None => true,
        Some(c) if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '~' | '%') => false,
        // `file:///Users/…`: the root follows the URL's own slashes.
        Some('/') => text[..at].ends_with("file://"),
        Some(_) => true,
    };
    if !starts_word {
        return None;
    }
    if let Some(home) = home {
        if let Some(after) = rest.strip_prefix(home) {
            if ends_component(after) {
                let components = home.split(is_separator).filter(|part| !part.is_empty());
                return Some(components.count());
            }
        }
    }
    for (root, depth) in ROOTS {
        if rest.starts_with(root) || (root.len() > 1 && rest == &root[..root.len() - 1]) {
            return Some(*depth);
        }
    }
    let mut chars = rest.chars();
    if let (Some(letter), Some(':'), Some(separator)) = (chars.next(), chars.next(), chars.next()) {
        if letter.is_ascii_alphabetic() && is_separator(separator) {
            return Some(DRIVE_DEPTH);
        }
    }
    // UNC share: `\\server\share\…`.
    if rest.starts_with("\\\\") && rest[2..].chars().next().is_some_and(|c| !is_separator(c)) {
        return Some(3);
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

/// Whether the words after a space belong to the same path: one of the next
/// [`MAX_WORDS_IN_SEGMENT`] words contains a separator and none of the words
/// before it starts a path of its own (`a.bin to /Users/b/c`).
fn continues_after_space(rest: &str, home: Option<&str>) -> bool {
    let mut remaining = rest;
    for _ in 0..=MAX_WORDS_IN_SEGMENT {
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
        match remaining[word_end..].strip_prefix(' ') {
            Some(next) => remaining = next,
            None => return false,
        }
    }
    false
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
}
