//! Removing absolute file paths from error text (ADR 0006, #360).
//!
//! Text that comes from outside Rust (the Swift engine host's messages and
//! stderr, OS API errors) names files by absolute path: `Failed to load model at
//! /Users/<name>/Library/Application Support/Sagascript/models/x.mlmodelc`. Such
//! text must not reach the app log or the UI, because the path carries the
//! user's account name and the names of their folders. [`scrub_paths`] is the
//! one function that does it, where the text enters a Rust error type
//! ([`crate::error::DictationError::engine`]), in the engine's log lines and
//! again when the app serializes a command error.
//!
//! A path becomes [`PLACEHOLDER`], followed by its file name when the path goes
//! deeper than the account folder: `/Users/x/Documents/a.m4a` is
//! `<path>/a.m4a`, `/Users/x` is `<path>`. The file name stays because it is
//! what the user (and a bug report) needs to tell which file failed; every
//! folder before it, however many words its name has, is removed.
//!
//! # Rules
//!
//! Privacy first: removing too much text is acceptable, keeping an account name
//! is not. Three properties hold for every input and are tested, also with
//! generated inputs:
//!
//! 1. A path never extends past the start of another recognised root, wherever
//!    that root starts (`HOME=/Users/zoe` after `/var/folders/xx` ends the first
//!    path before `HOME`, and `HOME=/Users/zoe` is a path of its own).
//! 2. The scan is linear: every character is looked at at most twice, and the
//!    check for a root costs the same at every position.
//! 3. Scrubbing twice gives the same text as scrubbing once.
//!
//! # Where a path ends
//!
//! The first word of a path (up to a space) always belongs to it. A word after
//! a space belongs to it only once a later word contains a separator
//! (`Application Support/Sagascript`, `Zoe’s Recordings/meeting.m4a`), so folder
//! names with spaces go whole and prose after the file name stays. The path
//! ends at the first of: a quote, `<`, `>`, `|`, `*`, `?`, a control character
//! or any other whitespace; a closing bracket that closes nothing opened inside
//! the path (balanced brackets are part of a folder name); a comma or semicolon
//! followed by a space; a `://`; another root; a word ending in a colon, a
//! period or an exclamation mark (a clause end); a word without a separator
//! after a file name (a name with an extension, `a.wav because …`). An
//! apostrophe between two letters (`Zoe’s`) is part of the name. Inside the
//! account folder (the first components) a period does not end the clause, so
//! `C:\Users\Mr. Smith\x` stays one path.
//!
//! # What it recognises
//!
//! The roots in `ROOTS` (macOS `/Users`, `/Volumes`, `/private`, Linux `/home`,
//! `/root`, `/var`, `/tmp`, matched ASCII case-insensitively because the macOS
//! file system is), their `/System/Volumes/Data/…` spelling, after any run of
//! slashes; `file://` URLs with or without a host; Windows drive, UNC and
//! `\\?\` / `\\.\` paths; the home directory the process reports; and the
//! Swift JSON encoder's `\/` spelling of a slash (the backslashes before a `/`
//! are dropped from the scrubbed text).
//!
//! Known limits, documented in ADR 0006: a root must start a word (`é/Users/…`,
//! `-o/Users/…` and a percent-encoded `%2FUsers%2F` are not recognised), `~zoe/`
//! and relative paths are not paths here, a comma, semicolon or colon followed
//! by a space ends a path even inside a folder name, and an account folder
//! name with a space that ends its clause (`C:\Users\John Smith.`) keeps the
//! words after the space, because they cannot be told from prose.

use std::borrow::Cow;
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

/// The longest run of slashes before a root that is still read as a path.
const MAX_SLASH_RUN: usize = 16;

/// How far back from a root to look for the `file://` that makes it a URL
/// path: the longest host name is 253 bytes.
const FILE_URL_LOOKBACK: usize = 270;

/// `text` with every absolute path replaced as described in the module
/// documentation. Idempotent and linear in the length of `text`.
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
    let text = unescape_slashes(text);
    let text: &str = &text;
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        if let Some(root) = root_at(text, at, home) {
            let end = path_end(text, at, root, home);
            let (path, tail) = split_trailing_punctuation(&text[at..end]);
            if !path.is_empty() {
                out.push_str(&replacement(&path[root.skip.min(path.len())..], root.depth));
                out.push_str(tail);
                at = end;
                continue;
            }
        }
        let Some(c) = text[at..].chars().next() else {
            break;
        };
        out.push(c);
        at += c.len_utf8();
    }
    out
}

/// The Swift host's JSON encoder writes `/` as `\/` (`\/Users\/zoe`). Drops
/// every run of backslashes that precedes a `/`, in one pass, so that such a
/// path is recognised and a second scrub finds nothing more to drop.
fn unescape_slashes(text: &str) -> Cow<'_, str> {
    if !text.contains("\\/") {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut backslashes = 0usize;
    for c in text.chars() {
        match c {
            '\\' => backslashes += 1,
            '/' => {
                backslashes = 0;
                out.push('/');
            }
            other => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
                out.push(other);
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes));
    Cow::Owned(out)
}

fn is_separator(c: char) -> bool {
    c == '/' || c == '\\'
}

/// Characters that end a path wherever they stand.
fn is_hard_terminator(c: char) -> bool {
    c.is_control() || matches!(c, '"' | '`' | '<' | '>' | '|' | '*' | '?' | '“' | '”' | '‘')
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

/// Whether the text at `at` starts a path root. The cheap test, `known_root`,
/// runs first: it rejects almost every position after a byte comparison.
fn root_at(text: &str, at: usize, home: Option<&str>) -> Option<Root> {
    let rest = &text[at..];
    let root = known_root(rest, home).or_else(|| file_url_drive_root(text, at))?;
    let before = text[..at].chars().next_back();
    if rest.starts_with('/') && after_file_url(&text[..at]) {
        return Some(root);
    }
    // A run of slashes right after a URL scheme (`https://`) is the URL's own.
    if before == Some(':') && rest.starts_with("//") {
        return None;
    }
    // `C:/Users/zoe` is one drive path, not a drive followed by a root.
    if before == Some(':') && ends_with_drive_letter(&text[..at]) {
        return None;
    }
    let starts_word = match before {
        None => true,
        Some(c) => {
            !(c.is_alphanumeric()
                || matches!(c, '.' | '-' | '_' | '~' | '%' | '/' | ')' | ']' | '}'))
        }
    };
    starts_word.then_some(root)
}

/// Whether `prefix` ends with a drive letter and its colon (`C:`) that start a word.
fn ends_with_drive_letter(prefix: &str) -> bool {
    let mut chars = prefix.chars().rev();
    match (chars.next(), chars.next(), chars.next()) {
        (Some(':'), Some(letter), before) => {
            letter.is_ascii_alphabetic() && !before.is_some_and(char::is_alphanumeric)
        }
        _ => false,
    }
}

fn known_root(rest: &str, home: Option<&str>) -> Option<Root> {
    let first = *rest.as_bytes().first()?;
    let home_may_start_here = home.is_some_and(|home| home.as_bytes().first() == Some(&first));
    if !(first == b'/' || first == b'\\' || first.is_ascii_alphabetic() || home_may_start_here) {
        return None;
    }
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
    if first == b'/' {
        // `//Users/zoe`: the extra slashes are part of the path. A longer run than
        // MAX_SLASH_RUN is not a spelling of a path (and counting it at every
        // position of a million slashes would not be linear).
        let slashes = rest
            .bytes()
            .take(MAX_SLASH_RUN + 1)
            .take_while(|&b| b == b'/')
            .count();
        if slashes > MAX_SLASH_RUN {
            return None;
        }
        if slashes >= 2 {
            return known_root(&rest[slashes - 1..], home);
        }
        if starts_with_ci(rest, FIRMLINK_PREFIX) && rest[FIRMLINK_PREFIX.len()..].starts_with('/') {
            let inner = known_root(&rest[FIRMLINK_PREFIX.len()..], home)?;
            return plain(inner.depth + FIRMLINK_DEPTH);
        }
        for (root, depth) in ROOTS {
            if starts_with_ci(rest, root) {
                return plain(*depth);
            }
        }
        return None;
    }
    if let Some(drive) = drive_root(rest) {
        return Some(drive);
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
            if let Some(drive) = drive_root(after) {
                return Some(Root {
                    depth: drive.depth,
                    skip: prefix.len(),
                });
            }
        }
    }
    // UNC share: `\\server\share\…` (the server and the share identify it).
    if rest.starts_with("\\\\") && rest[2..].chars().next().is_some_and(char::is_alphanumeric) {
        return plain(2);
    }
    None
}

/// `C:\…` or `C:/…`.
fn drive_root(rest: &str) -> Option<Root> {
    let mut chars = rest.chars();
    match (chars.next(), chars.next(), chars.next()) {
        (Some(letter), Some(':'), Some(separator))
            if letter.is_ascii_alphabetic() && is_separator(separator) =>
        {
            Some(Root {
                depth: DRIVE_DEPTH,
                skip: 0,
            })
        }
        _ => None,
    }
}

/// `file:///C:/Users/…`: the slash before the drive letter belongs to the URL.
fn file_url_drive_root(text: &str, at: usize) -> Option<Root> {
    let rest = &text[at..];
    let drive = rest.strip_prefix('/').and_then(drive_root)?;
    after_file_url(&text[..at]).then_some(Root {
        depth: drive.depth,
        skip: 1,
    })
}

/// Whether `prefix` ends with `file://` and a host name (possibly empty), the
/// authority of a file URL whose path comes next.
fn after_file_url(prefix: &str) -> bool {
    let mut from = prefix.len().saturating_sub(FILE_URL_LOOKBACK);
    while !prefix.is_char_boundary(from) {
        from += 1;
    }
    let window = &prefix[from..];
    let bytes = window.as_bytes();
    let needle = b"file://";
    let Some(last) = bytes.len().checked_sub(needle.len()) else {
        return false;
    };
    for index in (0..=last).rev() {
        if bytes[index..index + needle.len()].eq_ignore_ascii_case(needle) {
            return window[index + needle.len()..].chars().all(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '[' | ']')
            });
        }
    }
    false
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

/// What the scan of one path knows. See the module documentation for the rules.
struct Scan<'a> {
    text: &'a str,
    /// Identifying components of the root: inside them a period is no clause end.
    depth: usize,
    /// The end of what is certainly path.
    committed: usize,
    /// The current word is part of the path (the first word, or a word that
    /// contains a separator, which also commits the words before it).
    committing: bool,
    first_word: bool,
    word_start: usize,
    /// The word before the current one ends in a file extension.
    prev_file_like: bool,
    brackets: usize,
    components_done: usize,
    in_component: bool,
}

impl Scan<'_> {
    /// Takes the character at `index` into the scan.
    fn take(&mut self, index: usize, c: char) {
        if is_separator(c) {
            if self.in_component {
                self.components_done += 1;
                self.in_component = false;
            }
            self.committing = true;
        } else {
            self.in_component = true;
        }
        if self.committing {
            self.committed = index + c.len_utf8();
        }
    }

    /// The word `text[word_start..end]` is complete. Whether the path ends
    /// with it, or before it.
    fn finish_word(&mut self, end: usize) -> bool {
        let word = &self.text[self.word_start..end];
        if word.is_empty() {
            return false;
        }
        let in_account = self.components_done < self.depth;
        if !self.first_word && !self.committing && self.prev_file_like && !in_account {
            // A word without a separator after a file name is prose.
            return true;
        }
        let clause_end =
            word.ends_with(':') || (!in_account && (word.ends_with('.') || word.ends_with('!')));
        if clause_end {
            return true;
        }
        self.prev_file_like = looks_like_file(word);
        false
    }
}

/// The index just after the path that starts at `at` (the root's `skip` bytes
/// are not scanned): the end of the committed text when the scan stops.
fn path_end(text: &str, at: usize, root: Root, home: Option<&str>) -> usize {
    let start = at + root.skip;
    let mut scan = Scan {
        text,
        depth: root.depth,
        committed: start,
        committing: true,
        first_word: true,
        word_start: start,
        prev_file_like: false,
        brackets: 0,
        components_done: 0,
        in_component: false,
    };
    let mut prev: Option<char> = None;
    let mut chars = text[start..].char_indices().peekable();
    while let Some((offset, c)) = chars.next() {
        let index = start + offset;
        let next = chars.peek().map(|&(_, next)| next);
        // Rule 1: another root ends this path, wherever it starts.
        if index > start && root_at(text, index, home).is_some() {
            return scan.committed;
        }
        match c {
            ' ' => {
                if scan.finish_word(index) {
                    return scan.committed;
                }
                scan.first_word = false;
                scan.committing = false;
                scan.word_start = index + 1;
            }
            c if c.is_whitespace() || is_hard_terminator(c) => return scan.committed,
            '\'' | '’' => {
                let inside_a_word = prev.is_some_and(char::is_alphanumeric)
                    && next.is_some_and(char::is_alphanumeric);
                if !inside_a_word {
                    return scan.committed;
                }
                scan.take(index, c);
            }
            ',' | ';' => {
                if next.is_none_or(|next| next.is_whitespace() || is_hard_terminator(next)) {
                    return scan.committed;
                }
                scan.take(index, c);
            }
            '(' | '[' | '{' => {
                scan.brackets += 1;
                scan.take(index, c);
            }
            ')' | ']' | '}' => {
                if scan.brackets == 0 {
                    return scan.committed;
                }
                scan.brackets -= 1;
                scan.take(index, c);
            }
            ':' => {
                // `https://…` is a URL, not a folder.
                if text[index + 1..].starts_with("//") {
                    return scan.committed;
                }
                scan.take(index, c);
            }
            _ => scan.take(index, c),
        }
        prev = Some(c);
    }
    scan.committed
}

/// Whether the last component of `word` ends in a file extension: a dot, then
/// one to five letters or digits, after at least one character.
fn looks_like_file(word: &str) -> bool {
    let name = word.rsplit(is_separator).next().unwrap_or(word);
    let name = name.trim_end_matches(['.', ',', ':', ';', '!', ')', ']', '}', '\'', '’']);
    match name.rfind('.') {
        Some(dot) if dot > 0 => {
            let extension = &name[dot + 1..];
            (1..=5).contains(&extension.len())
                && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
        }
        _ => false,
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

    // ---- Round 2 of the review of #434: privacy-first boundaries, linear time,
    // idempotency, the Swift host's `\/` escaping, quotes and prose. ----

    #[test]
    fn a_path_never_extends_past_the_start_of_another_root() {
        let cases = [
            (
                "TMPDIR=/var/folders/xx HOME=/Users/zoe",
                "TMPDIR=<path>/xx HOME=<path>",
            ),
            ("x=/Users/a/in.wav y=/Users/zoe", "x=<path>/in.wav y=<path>"),
            (
                "open /Users/zoe/a.wav in dir=/Users/zoe",
                "open <path>/a.wav in dir=<path>",
            ),
            (
                "spawn failed: cwd=/Users/zoe/Library/Application Support/Sagascript bin=/Users/zoe/x/engine-host home=/Users/zoe",
                "spawn failed: cwd=<path>/Sagascript bin=<path>/engine-host home=<path>",
            ),
            (
                r"C:\Users\a\x.wav HOME=C:\Users\zoe",
                "<path>/x.wav HOME=<path>",
            ),
            ("a=/Users/a/x,/Users/zoe/y", "a=<path>/x,<path>/y"),
        ];
        for (input, expected) in cases {
            assert_eq!(scrub(input), expected, "{input:?}");
        }
    }

    #[test]
    fn a_drive_path_with_forward_slashes_is_one_path() {
        assert_eq!(
            scrub("open C:/Users/zoe/AppData/Local/x.bin missing"),
            "open <path>/x.bin missing"
        );
        assert_eq!(scrub("home C:/Users/zoe"), "home <path>");
        assert_eq!(
            scrub("could not read file:///C:/Users/zoe/x.bin"),
            "could not read file://<path>/x.bin"
        );
        // Two drive paths stay two paths.
        assert_eq!(
            scrub("C:/Users/a/x.wav HOME=C:/Users/zoe"),
            "<path>/x.wav HOME=<path>"
        );
    }

    /// A server name starts with a letter or digit: the placeholder itself
    /// (`<path>`) after two backslashes is not a UNC share, or a second pass
    /// would scrub the first pass's output (found by the fuzz test).
    #[test]
    fn a_placeholder_after_backslashes_is_not_a_unc_server() {
        let input = r#":homeC:[\\<path>!" (?)Users\)"#;
        let once = scrub(input);
        assert_eq!(once, input);
        assert_eq!(scrub(&once), once);
        assert_eq!(scrub(r"\\fileserver\audio\a.wav"), "<path>/a.wav");
        assert_eq!(scrub(r"\\10.0.0.5\audio\a.wav"), "<path>/a.wav");
    }

    #[test]
    fn the_swift_hosts_escaped_slashes_are_paths_too() {
        assert_eq!(
            scrub(
                r#"{"error":{"message":"Failed to load model at \/Users\/zoe\/Library\/Application Support\/Sagascript\/models\/foo.mlmodelc"},"id":1}"#
            ),
            r#"{"error":{"message":"Failed to load model at <path>/foo.mlmodelc"},"id":1}"#
        );
        assert_eq!(scrub(r"see \/home\/zoe\/a.wav now"), "see <path>/a.wav now");
        // Backslashes before a slash are dropped, once, so scrubbing again changes nothing.
        let once = scrub(r"a\\/b and c\/d");
        assert_eq!(once, "a/b and c/d");
        assert_eq!(scrub(&once), once);
    }

    #[test]
    fn quotes_and_punctuation_inside_a_folder_name_do_not_end_the_path() {
        let cases = [
            (
                "/Users/zoe/Library/Mobile Documents/com~apple~CloudDocs/Zoe’s Recordings/meeting.m4a",
                "<path>/meeting.m4a",
            ),
            ("/Users/zoe/Documents/Zoe's files/a.wav", "<path>/a.wav"),
            ("/Users/zoe/Backup/Zoe’s iPhone/x", "<path>/x"),
            ("/Users/zoe/Music/[Live] Set/a.wav", "<path>/a.wav"),
            ("/Users/zoe/Backup (old)/x.wav", "<path>/x.wav"),
            ("/Users/zoe/Smith,John/x.wav", "<path>/x.wav"),
            ("/Users/zoe/Work {v2}/x.wav", "<path>/x.wav"),
            // A bracket that closes one opened before the path ends the path.
            ("(see /Users/zoe/a.bin)", "(see <path>/a.bin)"),
            ("[/Users/zoe/Library/Caches/a b/c.bin]", "[<path>/c.bin]"),
            // A quote that closes one opened before the path still ends it.
            ("'/Users/zoe/Zoe's files/a.wav' failed", "'<path>/a.wav' failed"),
            ("\"/Users/zoe/a b/c.bin\", then", "\"<path>/c.bin\", then"),
        ];
        for (input, expected) in cases {
            assert_eq!(scrub(input), expected, "{input:?}");
        }
    }

    #[test]
    fn prose_after_the_file_name_is_not_swallowed() {
        let cases = [
            (
                "Failed to open /Users/zoe/a.wav because it is locked and/or missing",
                "Failed to open <path>/a.wav because it is locked and/or missing",
            ),
            (
                "Could not read /Users/zoe/a.wav - I/O error",
                "Could not read <path>/a.wav - I/O error",
            ),
            (
                "Could not open /Users/zoe/a.wav. Check Settings/Audio.",
                "Could not open <path>/a.wav. Check Settings/Audio.",
            ),
            (
                "/Users/zoe/Documents/My Meeting.m4a is 48 kHz/stereo",
                "<path>/My Meeting.m4a is 48 kHz/stereo",
            ),
            (
                "see /Users/zoe/a.wav and https://example.com/help",
                "see <path>/a.wav and https://example.com/help",
            ),
            (
                "copy /Users/a/in.wav to /Users/b/out.wav. Done",
                "copy <path>/in.wav to <path>/out.wav. Done",
            ),
            (
                "Could not open /Users/zoe/a.wav! Try Settings/Audio.",
                "Could not open <path>/a.wav! Try Settings/Audio.",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(scrub(input), expected, "{input:?}");
        }
    }

    #[test]
    fn the_second_pass_never_adds_a_placeholder_to_a_kept_file_name() {
        for name in ["tmp", "Users", "home", "root", "var", "Volumes", "private"] {
            let input = format!("/Users/zoe/{name}");
            let once = scrub(&input);
            assert_eq!(once, format!("<path>/{name}"), "{input:?}");
            assert_eq!(scrub(&once), once, "second pass changed {once:?}");
        }
        assert_eq!(scrub("/home/zoe/home: no"), "<path>/home: no");
        assert_eq!(scrub("/root/x/root and more"), "<path>/root and more");
        assert_eq!(scrub("/var/x/var and more"), "<path>/var and more");
    }

    #[test]
    fn a_root_after_a_run_of_slashes_or_a_file_url_host_is_a_root() {
        assert_eq!(scrub("open //Users/zoe/x.wav: no"), "open <path>/x.wav: no");
        assert_eq!(
            scrub("open /System/Volumes/Data//Users/zoe/x.wav: no"),
            "open <path>/x.wav: no"
        );
        assert_eq!(
            scrub("could not read file://studio.local/Users/zoe/x.bin"),
            "could not read file://studio.local<path>/x.bin"
        );
        assert_eq!(
            scrub("could not read file://[::1]/home/zoe/x.bin"),
            "could not read file://[::1]<path>/x.bin"
        );
        // A URL's own slashes are not a path, and a web path is not a user path.
        assert_eq!(
            scrub("see https://example.com//Users/zoe/x"),
            "see https://example.com//Users/zoe/x"
        );
        assert_eq!(
            scrub("see http://host/Users/zoe/x"),
            "see http://host/Users/zoe/x"
        );
    }

    #[test]
    fn spellings_that_are_not_recognised_are_documented_non_goals() {
        // The root must start a word: a letter or digit before it makes it part of another word.
        assert_eq!(scrub("é/Users/zoe/x"), "é/Users/zoe/x");
        assert_eq!(scrub("-o/Users/zoe/x"), "-o/Users/zoe/x");
        assert_eq!(scrub("~zoe/Documents/x.wav"), "~zoe/Documents/x.wav");
        assert_eq!(scrub("%2FUsers%2Fzoe%2Fx"), "%2FUsers%2Fzoe%2Fx");
    }

    #[test]
    fn known_limits_of_where_a_path_ends() {
        // A comma followed by a space, a semicolon followed by a space and a colon followed
        // by a space end a path even inside a folder name: the words after them stay visible
        // (they are folder names, never the account folder).
        assert_eq!(
            scrub("/Users/zoe/Smith, John/x.wav"),
            "<path>/Smith, John/x.wav"
        );
        assert_eq!(scrub("/Users/zoe/a: b/c.wav"), "<path>/a: b/c.wav");
        // An account folder with a space, written with nothing after it in its clause,
        // cannot be told from the words that follow it.
        assert_eq!(
            scrub(r"profile C:\Users\John Smith"),
            r"profile <path> Smith"
        );
        // A word with a separator after a file name joins the path (`and/or`): more is removed.
        assert_eq!(scrub("/Users/zoe/a.wav and/or other"), "<path>/or other");
        // With a folder after it the whole name goes.
        assert_eq!(
            scrub(r"profile C:\Users\John Smith\x.bin"),
            "profile <path>/x.bin"
        );
    }

    #[test]
    fn verified_positives_stay_positive() {
        let nserror = "Error Domain=NSCocoaErrorDomain Code=260 \"The file “foo.mlmodelc” couldn’t be opened because there is no such file.\" UserInfo={NSFilePath=/Users/zoe/Library/Application Support/Sagascript/models/foo.mlmodelc, NSUnderlyingError=0x600000 {Error Domain=NSPOSIXErrorDomain Code=2 \"No such file or directory\"}}";
        assert_eq!(
            scrub(nserror),
            "Error Domain=NSCocoaErrorDomain Code=260 \"The file “foo.mlmodelc” couldn’t be opened because there is no such file.\" UserInfo={NSFilePath=<path>/foo.mlmodelc, NSUnderlyingError=0x600000 {Error Domain=NSPOSIXErrorDomain Code=2 \"No such file or directory\"}}"
        );
        assert_eq!(
            scrub("file:///Users/zoe/Library/Application%20Support/S/a.mlmodelc could not open"),
            "file://<path>/a.mlmodelc could not open"
        );
        assert_eq!(
            scrub(r"Failed at C:\Users\Alice\AppData\Local\Sagascript\models\foo.bin: denied"),
            "Failed at <path>/foo.bin: denied"
        );
    }

    /// Every probe of the review, and more, scrubs to the same text twice (the app
    /// scrubs when an engine error is built and again when a command error is sent).
    #[test]
    fn scrubbing_is_idempotent_on_every_probe() {
        let probes = [
            "TMPDIR=/var/folders/xx HOME=/Users/zoe",
            "x=/Users/a/in.wav y=/Users/zoe",
            "open /Users/zoe/a.wav in dir=/Users/zoe",
            "/Users/zoe/tmp",
            "/Users/zoe/Users",
            "/Users/zoe/Music/[Live] Set/a.wav",
            "/Users/zoe/Library/Mobile Documents/com~apple~CloudDocs/Zoe’s Recordings/meeting.m4a",
            "Failed to open /Users/zoe/a.wav because it is locked and/or missing",
            "//Users/zoe/x.wav",
            r#"{"a":"\/Users\/zoe\/x"}"#,
            r"a\\/b",
            r"\\?\C:\Users\zoe\x.bin",
            "file://localhost/Users/zoe/x.bin",
            "file:///C:/Users/zoe/x.bin",
            "(/Users/zoe/a.bin) and [/home/zoe/b]",
            "profile C:\\Users\\John Smith",
            "<path>/tmp /tmp/<path>",
            "/tmp/ /var/ /home/ /Users/ /root/",
        ];
        for probe in probes {
            let once = scrub(probe);
            assert_eq!(scrub(&once), once, "not idempotent for {probe:?}: {once:?}");
        }
    }

    /// A deterministic generator (xorshift), so a failure reproduces.
    struct Generator(u64);

    impl Generator {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
            items[(self.next() % items.len() as u64) as usize]
        }
    }

    const ROOT_TOKENS: &[&str] = &[
        "/Users/zoe9",
        "/users/zoe9",
        "/home/zoe9",
        r"C:\Users\zoe9",
        "C:/Users/zoe9",
        "file:///Users/zoe9",
        "file://localhost/Users/zoe9",
        "//Users/zoe9",
        r"\/Users\/zoe9",
        "/System/Volumes/Data/Users/zoe9",
        r"\\?\C:\Users\zoe9",
        r"\\server\share",
        "/var/folders/zz",
        "/tmp/x",
        "/Volumes/Disk",
    ];
    const SEGMENTS: &[&str] = &[
        "Library",
        "Application Support",
        "a b",
        "x.wav",
        "Zoe’s files",
        "Zoe's files",
        "[Live] Set",
        "(old)",
        "tmp",
        "Users",
        "日本語 フォルダ",
        "é",
        "v1.2 notes",
        "and/or",
        "a: b",
        "a, b",
    ];
    const JOINERS: &[&str] = &["/", "/", "/", "\\", "//"];
    const BOUNDARIES: &[&str] = &[
        " ",
        " ",
        "=",
        ", ",
        ": ",
        ". ",
        "\n",
        " (",
        "\"",
        " and ",
        " because ",
        " - ",
        " https://example.com/x ",
        "; ",
        "'",
        " « ",
        "\t",
    ];
    const SOUP: &[&str] = &[
        "/", "\\", " ", "\\/", "Users", "home", "tmp", "var", "Volumes", "private", "root",
        "file://", "C:", "\\\\?\\", ":", "'", "’", "\"", "(", ")", "[", "]", ",", ";", ".", "!",
        "?", "é", "日", "😀", "=", "<path>", "x.wav", "zoe9", "\n", "\u{a0}",
    ];

    fn generated_path_text(generator: &mut Generator) -> String {
        let mut text = String::new();
        for _ in 0..(1 + generator.next() % 4) {
            text.push_str(generator.pick(ROOT_TOKENS));
            for _ in 0..(generator.next() % 5) {
                text.push_str(generator.pick(JOINERS));
                text.push_str(generator.pick(SEGMENTS));
            }
            text.push_str(generator.pick(BOUNDARIES));
        }
        text
    }

    fn generated_soup(generator: &mut Generator) -> String {
        (0..(1 + generator.next() % 40))
            .map(|_| generator.pick(SOUP))
            .collect()
    }

    #[test]
    fn generated_inputs_never_panic_and_scrub_idempotently() {
        let mut generator = Generator(0x9E37_79B9_7F4A_7C15);
        for round in 0..20_000 {
            let text = if round % 2 == 0 {
                generated_path_text(&mut generator)
            } else {
                generated_soup(&mut generator)
            };
            let once = scrub(&text);
            assert_eq!(scrub(&once), once, "not idempotent for {text:?}: {once:?}");
        }
    }

    /// Account names that follow a recognised root at the start of a word never survive.
    #[test]
    fn a_generated_account_name_never_survives_after_a_recognised_root() {
        let mut generator = Generator(0xD1B5_4A32_D192_ED03);
        for _ in 0..20_000 {
            let text = generated_path_text(&mut generator);
            let scrubbed = scrub(&text);
            assert!(!scrubbed.contains("zoe9"), "{text:?} -> {scrubbed:?}");
        }
    }

    /// The long run: 400k inputs by default, half of each kind. Another seed or length:
    /// `PATH_SCRUB_FUZZ_SEED=7 PATH_SCRUB_FUZZ_ROUNDS=5000000 cargo test --release -p sagascript-core
    /// --lib path_scrub::tests::fuzz -- --ignored --nocapture`.
    #[test]
    #[ignore = "a 400k-input fuzz run; the 40k-input versions above run by default"]
    fn fuzz_never_panics_and_stays_idempotent_and_private() {
        let env = |name: &str, default: u64| {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(default)
        };
        let seed = env("PATH_SCRUB_FUZZ_SEED", 1);
        let rounds = env("PATH_SCRUB_FUZZ_ROUNDS", 400_000);
        let mut generator =
            Generator(0xA076_1D64_78BD_642F ^ seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        // xorshift needs a non-zero state, and a few steps to spread the seed.
        for _ in 0..8 {
            generator.next();
        }
        let started = std::time::Instant::now();
        for round in 0..rounds {
            let (text, structured) = if round % 2 == 0 {
                (generated_path_text(&mut generator), true)
            } else {
                (generated_soup(&mut generator), false)
            };
            let once = scrub(&text);
            assert_eq!(scrub(&once), once, "not idempotent for {text:?}: {once:?}");
            if structured {
                assert!(!once.contains("zoe9"), "{text:?} -> {once:?}");
            }
        }
        eprintln!(
            "fuzz: {rounds} inputs (seed {seed}) in {:?}",
            started.elapsed()
        );
    }

    /// The engine host's stderr drain thread scrubs its lines; a stall there fills the
    /// host's pipe and blocks it. Scrubbing is linear: each of these 1 MB inputs takes
    /// 0.3-60 ms in a release build and up to 0.7 s in a debug build on a developer
    /// machine (2.2 s on a loaded CI runner running other tests alongside). The bounds
    /// (1 s release, 10 s debug) leave room for that noise and only catch quadratic
    /// behaviour, which needs minutes for these inputs.
    #[test]
    fn scrubbing_is_linear_on_adversarial_input() {
        let megabyte = 1 << 20;
        let inputs: Vec<(&str, String)> = vec![
            (
                "many words between a path and a later slash-word",
                format!("/Users/zoe/a {}y/z", "x ".repeat(megabyte / 2)),
            ),
            (
                "many ' a' then y/z",
                format!("/Users/zoe/a{}y/z", " a".repeat(megabyte / 2)),
            ),
            ("a million roots", "/Users/z/".repeat(megabyte / 9)),
            ("a million file URLs", "file://".repeat(megabyte / 7)),
            (
                "a million roots in words",
                "x=/Users/zoe a ".repeat(megabyte / 15),
            ),
            ("escaped slashes", r"\/".repeat(megabyte / 2)),
            (
                "a million backslashes",
                format!("{}/", "\\".repeat(megabyte)),
            ),
            ("brackets", "/Users/zoe/(".repeat(megabyte / 12)),
            (
                "tentative words and colons",
                format!("/Users/zoe/a {}", "w: ".repeat(megabyte / 3)),
            ),
        ];
        let bound = std::time::Duration::from_secs(if cfg!(debug_assertions) { 10 } else { 1 });
        for (name, input) in &inputs {
            let started = std::time::Instant::now();
            let output = scrub(input);
            let elapsed = started.elapsed();
            eprintln!(
                "path_scrub: {name}: {} KB in {elapsed:?}",
                input.len() / 1024
            );
            assert!(
                elapsed < bound,
                "{name}: {} bytes took {elapsed:?}",
                input.len()
            );
            assert!(!output.contains("zoe"), "{name}");
        }
    }
}
