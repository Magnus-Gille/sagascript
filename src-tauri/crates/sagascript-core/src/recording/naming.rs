//! Folder names for recordings: `<stamp> <sanitised title> [<short id>]`.
//!
//! The output is safe as a path component on macOS, Windows, Linux and FAT.

/// Longest sanitised title, in UTF-8 bytes (cut at a char boundary).
pub const MAX_TITLE_BYTES: usize = 100;
/// Used when nothing of the title survives.
pub const DEFAULT_TITLE: &str = "Recording";
/// Number of hex digits of the short id.
pub const SHORT_ID_LEN: usize = 6;

const STAMP_MAX_BYTES: usize = 24;
const UNKNOWN_TIME: &str = "unknown-time";
const NO_ID: &str = "000000";

/// Characters deleted by rule 2 and rule 3.
const FORBIDDEN: &[char] = &[
    '/', '\\', ':', '*', '?', '"', '<', '>', '|', '\0', '[', ']', ';', '{', '}', '$', '^', '`',
];

/// Windows device names, matched case-insensitively on the part before the first dot.
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Turn free text into a string that is safe as part of a folder name on macOS, Windows, Linux and FAT.
pub fn sanitize_title(title: &str) -> String {
    let text = trim_ends(cut_bytes(&normalise(title), MAX_TITLE_BYTES)).to_string();
    // A Windows device name stays reserved whatever follows its first dot
    // (`con.txt` is the console), so the underscore goes right after the stem:
    // `CON` -> `CON_`, `con.txt` -> `con_.txt`. `con_` is no device name, so
    // the rule is idempotent. The text is cut first to leave room for it.
    let text = if is_reserved(&text) {
        let text = trim_ends(cut_bytes(&text, MAX_TITLE_BYTES - 1)).to_string();
        let stem_end = text.find('.').unwrap_or(text.len());
        let stem_end = text[..stem_end].trim_end_matches(' ').len();
        format!("{}_{}", &text[..stem_end], &text[stem_end..])
    } else {
        text
    };
    if text.is_empty() {
        DEFAULT_TITLE.to_string()
    } else {
        text
    }
}

/// `"<stamp> <sanitised title> [<short id>]"`, never longer than 140 bytes.
pub fn folder_name(stamp: &str, title: &str, short_id: &str) -> String {
    format!(
        "{} {} [{}]",
        sanitize_stamp(stamp),
        sanitize_title(title),
        short_id_from(short_id)
    )
}

/// First SHORT_ID_LEN lowercase hex digits of a UUID string such as "a1b2c3d4-....".
pub fn short_id_of(uuid: &str) -> String {
    short_id_from(uuid)
}

/// Rules 1 to 5: whitespace, removals, dot runs, and trimming.
fn normalise(text: &str) -> String {
    let spaced = collapse_spaces(text.chars());
    let kept: String = spaced.chars().filter(|&c| !is_removed(c)).collect();
    let mut dotless = kept;
    while dotless.contains("..") {
        dotless = dotless.replace("..", "");
    }
    trim_ends(&collapse_spaces(dotless.chars())).to_string()
}

/// Maps every whitespace character to a space and merges runs of spaces.
fn collapse_spaces(chars: impl Iterator<Item = char>) -> String {
    let mut out = String::new();
    for c in chars {
        let c = if c.is_whitespace() { ' ' } else { c };
        if c == ' ' && out.ends_with(' ') {
            continue;
        }
        out.push(c);
    }
    out
}

fn is_removed(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
                | '\u{FEFF}'
        )
        || FORBIDDEN.contains(&c)
}

fn trim_ends(text: &str) -> &str {
    text.trim_matches(|c| c == ' ' || c == '.')
}

fn is_reserved(text: &str) -> bool {
    let stem = text.split('.').next().unwrap_or("").trim_matches(' ');
    RESERVED.iter().any(|name| stem.eq_ignore_ascii_case(name))
}

/// Longest prefix of at most `max` bytes that ends on a char boundary.
fn cut_bytes(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The stamp gets rules 1 to 5 and a byte cut, without the reserved-name and default rules.
fn sanitize_stamp(stamp: &str) -> String {
    let text = trim_ends(cut_bytes(&normalise(stamp), STAMP_MAX_BYTES)).to_string();
    if text.is_empty() {
        UNKNOWN_TIME.to_string()
    } else {
        text
    }
}

fn short_id_from(text: &str) -> String {
    let digits: String = text
        .chars()
        .filter(char::is_ascii_hexdigit)
        .map(|c| c.to_ascii_lowercase())
        .take(SHORT_ID_LEN)
        .collect();
    if digits.is_empty() {
        NO_ID.to_string()
    } else {
        digits
    }
}

#[cfg(test)]
#[path = "naming_tests.rs"]
mod tests;
