const MAX_TEXT_BYTES: usize = 32_768;

fn utf16_boundary(text: &str, unit: usize) -> Option<usize> {
    let mut units = 0usize;
    for (byte, character) in text.char_indices() {
        if unit == units {
            return Some(byte);
        }
        let next = units.checked_add(character.len_utf16())?;
        if unit < next {
            return None;
        }
        units = next;
    }
    (unit == units).then_some(text.len())
}

pub fn replace_utf16_range(
    original: &str,
    location: usize,
    length: usize,
    inserted: &str,
) -> Option<String> {
    if original.len() > MAX_TEXT_BYTES || inserted.len() > MAX_TEXT_BYTES {
        return None;
    }
    let end = location.checked_add(length)?;
    let start_byte = utf16_boundary(original, location)?;
    let end_byte = utf16_boundary(original, end)?;
    let result_bytes = original[..start_byte]
        .len()
        .checked_add(inserted.len())?
        .checked_add(original[end_byte..].len())?;
    if result_bytes > MAX_TEXT_BYTES {
        return None;
    }

    let mut result = String::with_capacity(result_bytes);
    result.push_str(&original[..start_byte]);
    result.push_str(inserted);
    result.push_str(&original[end_byte..]);
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::replace_utf16_range;

    #[test]
    fn ascii_insert_replace_and_end() {
        assert_eq!(replace_utf16_range("abc", 1, 0, "X"), Some("aXbc".into()));
        assert_eq!(replace_utf16_range("abc", 1, 1, "X"), Some("aXc".into()));
        assert_eq!(replace_utf16_range("abc", 3, 0, "!"), Some("abc!".into()));
    }

    #[test]
    fn swedish_text_uses_utf16_units() {
        assert_eq!(
            replace_utf16_range("Tack så mycket", 5, 2, "gärna"),
            Some("Tack gärna mycket".into())
        );
    }

    #[test]
    fn emoji_accepts_codepoint_boundaries_and_rejects_surrogates() {
        assert_eq!(replace_utf16_range("a😀b", 1, 2, "X"), Some("aXb".into()));
        assert_eq!(replace_utf16_range("a😀b", 3, 1, "!"), Some("a😀!".into()));
        assert_eq!(replace_utf16_range("a😀b", 2, 0, "X"), None);
        assert_eq!(replace_utf16_range("a😀b", 2, 1, "X"), None);
    }

    #[test]
    fn combining_marks_are_not_normalized() {
        assert_eq!(
            replace_utf16_range("e\u{301}x", 1, 1, ""),
            Some("ex".into())
        );
        assert_eq!(
            replace_utf16_range("e\u{301}x", 0, 1, "é"),
            Some("é\u{301}x".into())
        );
    }

    #[test]
    fn overflow_and_out_of_range_are_rejected() {
        assert_eq!(replace_utf16_range("abc", usize::MAX, 1, ""), None);
        assert_eq!(replace_utf16_range("abc", usize::MAX, 0, ""), None);
        assert_eq!(replace_utf16_range("abc", 4, 0, ""), None);
        assert_eq!(replace_utf16_range("abc", 2, 2, ""), None);
    }

    #[test]
    fn byte_caps_are_exact_and_excess_is_rejected() {
        let exact = "a".repeat(32_768);
        assert_eq!(
            replace_utf16_range(&exact, 32_768, 0, ""),
            Some(exact.clone())
        );
        assert_eq!(replace_utf16_range(&exact, 32_768, 0, "x"), None);
        let inserted = "b".repeat(32_768);
        assert_eq!(replace_utf16_range("", 0, 0, &inserted), Some(inserted));
        assert_eq!(replace_utf16_range("", 0, 0, &"c".repeat(32_769)), None);
        assert_eq!(replace_utf16_range(&"d".repeat(32_769), 0, 0, ""), None);
        assert_eq!(
            replace_utf16_range(&"😀".repeat(8_192), 0, 0, ""),
            Some("😀".repeat(8_192))
        );
        assert_eq!(replace_utf16_range(&"😀".repeat(8_192), 0, 0, "x"), None);
    }
}
