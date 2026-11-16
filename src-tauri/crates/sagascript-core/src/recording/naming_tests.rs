use super::*;

/// (input, expected sanitised title).
const CASES: &[(&str, &str)] = &[
    ("Team meeting", "Team meeting"),
    ("Möte om åäö", "Möte om åäö"),
    ("a/b\\c", "abc"),
    ("../../etc/passwd", "etcpasswd"),
    ("...", DEFAULT_TITLE),
    ("  spaced   out  ", "spaced out"),
    ("report. . .", "report"),
    ("CON", "CON_"),
    ("nul", "nul_"),
    ("COM1", "COM1_"),
    ("lpt9.log", "lpt9_.log"),
    ("con .txt", "con_ .txt"),
    ("CONSOLE", "CONSOLE"),
    ("Meet:ing*?\"<>|", "Meeting"),
    ("a\u{0}b\u{7}c", "abc"),
    ("x\u{1b}y", "xy"),
    ("\u{202E}abc", "abc"),
    ("zero\u{200B}width", "zerowidth"),
    ("Standup \u{1F399}", "Standup \u{1F399}"),
    ("Q1 [draft]", "Q1 draft"),
    ("", DEFAULT_TITLE),
    ("   ", DEFAULT_TITLE),
    ("/\\:*?\"<>|\u{0}\u{200B}", DEFAULT_TITLE),
    ("v1.2 notes", "v1.2 notes"),
    (".bashrc", "bashrc"),
    ("tab\there", "tab here"),
    ("line1\nline2", "line1 line2"),
    ("nbsp\u{a0}x", "nbsp x"),
    ("a\u{2028}b", "a b"),
    ("Q&A (2+2=4) #1 @home! ~50%", "Q&A (2+2=4) #1 @home! ~50%"),
    ("it's_a-test, ok.", "it's_a-test, ok"),
    ("{x}$y^z`w;", "xyzw"),
    ("a / b", "a b"),
    ("a....b", "ab"),
    ("CON ", "CON_"),
    ("\u{FEFF}Meeting\u{FEFF}", "Meeting"),
    ("a\u{200F}b\u{2066}c\u{2069}", "abc"),
    ("Q1 \u{2014} draft", "Q1 \u{2014} draft"),
    ("..CON..", "CON_"),
    ("a. .b", "a. .b"),
];

/// Inputs that have historically broken folder names.
const NASTY: &[&str] = &[
    "",
    " ",
    ".",
    "..",
    "...",
    " . ",
    "CON",
    "con.",
    "CON .x",
    "CON..",
    "con.txt",
    "con.txt_",
    "\u{202E}\u{202E}",
    "a\u{202E}..b",
    "a/./../b",
    "\t\n\r",
    "NUL.txt",
    "a.\u{200B}.b",
    "lpt1 ",
    "AUX_",
    "\u{1F399}.",
    "Recording",
    "a . . b",
    "x\u{0}\u{0}..",
    "a .. .",
];

/// Characters that must never appear in a sanitised title.
const OUTPUT_FORBIDDEN: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|', '[', ']', ';'];

const SEED: u64 = 0x2545_F491_4F6C_DD1D;

const ALPHABET: &[char] = &[
    'a',
    'Z',
    '0',
    '9',
    '.',
    '.',
    ' ',
    ' ',
    '/',
    '\\',
    ':',
    '*',
    '?',
    '"',
    '<',
    '>',
    '|',
    '[',
    ']',
    ';',
    '\0',
    '\t',
    '\u{7}',
    'å',
    'ö',
    '\u{1F399}',
    '\u{202E}',
    '-',
    '_',
    '{',
    '}',
    '$',
    '^',
    '`',
    '\u{200B}',
    '\u{FEFF}',
    '\u{a0}',
    '\u{2028}',
    '(',
    '&',
];

/// Small deterministic xorshift generator for the property tests.
struct XorShift(u64);

impl XorShift {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

fn random_text(rng: &mut XorShift) -> String {
    let len = rng.below(200);
    (0..len)
        .map(|_| ALPHABET[rng.below(ALPHABET.len())])
        .collect()
}

#[test]
fn sanitize_title_table() {
    for (input, expected) in CASES {
        assert_eq!(sanitize_title(input), *expected, "input {input:?}");
    }
}

#[test]
fn sanitize_title_nasty_inputs_are_idempotent_and_valid() {
    for input in NASTY {
        let out = sanitize_title(input);
        assert_eq!(sanitize_title(&out), out, "not idempotent for {input:?}");
        assert_title_invariants(&out, input);
    }
}

#[test]
fn sanitize_title_random_inputs_are_idempotent_and_valid() {
    let mut rng = XorShift(SEED);
    for _ in 0..2000 {
        let input = random_text(&mut rng);
        let out = sanitize_title(&input);
        assert_eq!(sanitize_title(&out), out, "not idempotent for {input:?}");
        assert_title_invariants(&out, &input);
    }
}

fn assert_title_invariants(out: &str, input: &str) {
    assert!(!out.is_empty(), "empty output for {input:?}");
    assert!(
        out.len() <= MAX_TITLE_BYTES,
        "{} bytes for {input:?}",
        out.len()
    );
    assert!(
        !out.contains(|c: char| OUTPUT_FORBIDDEN.contains(&c) || c.is_control()),
        "forbidden character in {out:?} from {input:?}"
    );
    assert!(!out.starts_with(['.', ' ']), "bad start in {out:?}");
    assert!(!out.ends_with(['.', ' ']), "bad end in {out:?}");
    assert!(!out.contains(".."), "double dot in {out:?}");
}

#[test]
fn title_longer_than_limit_is_cut_to_exactly_the_limit() {
    let out = sanitize_title(&"a".repeat(300));
    assert_eq!(out, "a".repeat(100));
    assert_eq!(out.len(), MAX_TITLE_BYTES);
}

#[test]
fn two_byte_title_is_cut_to_fifty_chars() {
    let out = sanitize_title(&"å".repeat(60));
    assert_eq!(out, "å".repeat(50));
    assert_eq!(out.len(), 100);
}

#[test]
fn four_byte_emoji_title_is_cut_at_char_boundary() {
    let out = sanitize_title(&"\u{1F399}".repeat(30));
    assert_eq!(out, "\u{1F399}".repeat(25));
    assert!(out.len() <= MAX_TITLE_BYTES);
}

#[test]
fn cut_then_trim_removes_trailing_space() {
    let out = sanitize_title(&format!("{} b", "a".repeat(99)));
    assert_eq!(out, "a".repeat(99));
}

#[test]
fn cut_never_splits_a_two_byte_char() {
    let out = sanitize_title(&format!("{}å", "a".repeat(99)));
    assert_eq!(out, "a".repeat(99));
}

#[test]
fn reserved_name_with_long_tail_is_fixed_after_the_stem_and_idempotent() {
    let input = format!("CON.{}_", "a".repeat(100));
    let out = sanitize_title(&input);
    assert!(out.len() <= MAX_TITLE_BYTES, "{} bytes", out.len());
    assert!(out.starts_with("CON_."), "{out:?}");
    assert_eq!(sanitize_title(&out), out);
}

#[test]
fn folder_name_typical_example() {
    assert_eq!(
        folder_name(
            "2026-11-05 14.02",
            "Team meeting",
            "a1b2c3d4-e5f6-7890-abcd-ef1234567890"
        ),
        "2026-11-05 14.02 Team meeting [a1b2c3]"
    );
}

#[test]
fn folder_name_hostile_stamp_and_title_cannot_inject_separators() {
    assert_eq!(
        folder_name("../2026/11", "a/../b", "../../x"),
        "202611 ab [000000]"
    );
    assert_eq!(folder_name("[evil]/..", "x", "abc123"), "evil x [abc123]");
}

#[test]
fn folder_name_sanitises_short_id() {
    assert_eq!(folder_name("s", "t", "A1B2C3D4"), "s t [a1b2c3]");
    assert_eq!(folder_name("s", "t", "]/..["), "s t [000000]");
}

#[test]
fn folder_name_empty_id_and_stamp_fall_back() {
    assert_eq!(folder_name("2026", "t", ""), "2026 t [000000]");
    assert_eq!(folder_name("", "T", "abc"), "unknown-time T [abc]");
    assert_eq!(folder_name("../", "T", "abc"), "unknown-time T [abc]");
}

#[test]
fn folder_name_stamp_is_cut_to_24_bytes() {
    let name = folder_name(&"9".repeat(500), "t", "abc");
    assert_eq!(name, format!("{} t [abc]", "9".repeat(24)));
}

#[test]
fn folder_name_length_is_bounded() {
    let name = folder_name(&"9".repeat(500), &"å".repeat(300), &"f".repeat(50));
    assert_eq!(name.len(), 134);
    assert!(name.len() <= 140);
}

#[test]
fn folder_name_random_inputs_stay_bounded_and_structured() {
    let mut rng = XorShift(SEED ^ 0xFFFF);
    for _ in 0..2000 {
        let stamp = random_text(&mut rng);
        let title = random_text(&mut rng);
        let id = random_text(&mut rng);
        let name = folder_name(&stamp, &title, &id);
        assert!(name.len() <= 140, "{} bytes for {name:?}", name.len());
        assert_eq!(name.matches('[').count(), 1, "{name:?}");
        assert!(name.ends_with(']'), "{name:?}");
        assert!(!name.contains('/') && !name.contains('\\'), "{name:?}");
        assert!(!name.contains(".."), "{name:?}");
        let open = name.rfind('[').expect("opening bracket");
        let id_part = &name[open + 1..name.len() - 1];
        assert!(
            (1..=SHORT_ID_LEN).contains(&id_part.len()),
            "id {id_part:?} in {name:?}"
        );
        assert!(
            id_part
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "id {id_part:?} in {name:?}"
        );
    }
}

#[test]
fn short_id_of_takes_first_six_lowercase_hex_digits() {
    assert_eq!(
        short_id_of("A1B2C3D4-E5F6-7890-ABCD-EF1234567890"),
        "a1b2c3"
    );
    assert_eq!(short_id_of("12-34-5678"), "123456");
}

#[test]
fn short_id_of_without_hex_digits_is_zero_id() {
    assert_eq!(short_id_of("zzz"), "000000");
    assert_eq!(short_id_of(""), "000000");
}
