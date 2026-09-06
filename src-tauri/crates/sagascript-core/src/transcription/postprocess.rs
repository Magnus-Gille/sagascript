use crate::settings::Language;

#[derive(Clone, Copy)]
struct Token<'a> {
    start: usize,
    end: usize,
    text: &'a str,
}

/// Make Whisper's music annotations readable without rewriting ordinary speech.
///
/// Explicit labels such as `[Music]` and `(Musik)` are always canonicalized.
/// Bare labels are only treated as annotations when Whisper repeats them at
/// least three times in a row; this preserves genuine phrases such as
/// "jag gillar musik" and even a spoken double repetition.
/// Exact `[BLANK_AUDIO]` artifacts are removed from the display text.
pub fn normalize_nonspeech_markers(text: &str, language: Language) -> String {
    let text = remove_blank_audio_markers(text);
    let tokens = token_spans(&text);
    if tokens.is_empty() {
        return text;
    }

    let marker = music_marker(language);
    let mut rendered = String::with_capacity(text.len());
    let mut cursor = 0;
    let mut index = 0;

    while index < tokens.len() {
        let Some(mut has_explicit_label) = classify_music_token(tokens[index].text) else {
            index += 1;
            continue;
        };

        let mut end = index + 1;
        while end < tokens.len() {
            let Some(explicit) = classify_music_token(tokens[end].text) else {
                break;
            };
            has_explicit_label |= explicit;
            end += 1;
        }

        let run_len = end - index;
        if has_explicit_label || run_len >= 3 {
            rendered.push_str(&text[cursor..tokens[index].start]);
            rendered.push_str(marker);
            cursor = tokens[end - 1].end;
        }
        index = end;
    }

    if cursor == 0 {
        return text;
    }
    rendered.push_str(&text[cursor..]);
    rendered
}

const BLANK_AUDIO_MARKER: &str = "[BLANK_AUDIO]";

fn remove_blank_audio_markers(text: &str) -> String {
    if !text.contains(BLANK_AUDIO_MARKER) {
        return text.to_string();
    }

    let mut rendered = String::with_capacity(text.len());
    for (index, part) in text.split(BLANK_AUDIO_MARKER).enumerate() {
        if index == 0 {
            rendered.push_str(part);
            continue;
        }
        // Only normalize the boundary left by a removed marker. Paragraphs
        // and intentional spacing elsewhere must remain untouched.
        let left_trimmed_len = rendered.trim_end().len();
        let right_trimmed = part.trim_start();
        let boundary = format!(
            "{}{}",
            &rendered[left_trimmed_len..],
            &part[..part.len() - right_trimmed.len()]
        );
        rendered.truncate(left_trimmed_len);
        let part = right_trimmed;
        if !rendered.is_empty() && !part.is_empty() {
            if boundary.contains('\n') {
                rendered.push_str(if boundary.matches('\n').count() > 1 {
                    "\n\n"
                } else {
                    "\n"
                });
            } else if !part.starts_with(['.', ',', '!', '?', ';', ':']) {
                rendered.push(' ');
            }
        }
        rendered.push_str(part);
    }

    if rendered
        .chars()
        .all(|character| !character.is_alphanumeric())
    {
        String::new()
    } else {
        rendered
    }
}

fn token_spans(text: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut start = None;
    for (index, character) in text.char_indices() {
        if character.is_whitespace() {
            if let Some(token_start) = start.take() {
                tokens.push(Token {
                    start: token_start,
                    end: index,
                    text: &text[token_start..index],
                });
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(token_start) = start {
        tokens.push(Token {
            start: token_start,
            end: text.len(),
            text: &text[token_start..],
        });
    }
    tokens
}

fn music_marker(language: Language) -> &'static str {
    match language {
        Language::Swedish => "[MUSIK]",
        Language::Norwegian => "[MUSIKK]",
        // Preserve the established annotation vocabulary for Finnish until
        // localized annotation rules have their own speech-safety evidence.
        Language::English | Language::Finnish | Language::Auto => "[MUSIC]",
    }
}

/// Returns whether the token is an explicitly delimited music label.
fn classify_music_token(token: &str) -> Option<bool> {
    let without_terminal = token.trim_end_matches(['.', ',', '!', '?', ';', ':']);
    let (inner, explicit) = if let Some(inner) = without_terminal
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    {
        (inner, true)
    } else if let Some(inner) = without_terminal
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))
    {
        (inner, true)
    } else {
        (without_terminal, false)
    };

    matches!(inner.to_lowercase().as_str(), "music" | "musik" | "musikk").then_some(explicit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_long_bare_swedish_runs() {
        assert_eq!(
            normalize_nonspeech_markers(
                "Tack! Musik musik MUSIK. Samtal Musik Musik Musik Musik",
                Language::Swedish,
            ),
            "Tack! [MUSIK] Samtal [MUSIK]"
        );
    }

    #[test]
    fn preserves_one_or_two_bare_mentions() {
        assert_eq!(
            normalize_nonspeech_markers("Jag gillar musik", Language::Swedish),
            "Jag gillar musik"
        );
        assert_eq!(
            normalize_nonspeech_markers("Musik musik", Language::Swedish),
            "Musik musik"
        );
    }

    #[test]
    fn canonicalizes_explicit_labels_and_adjacent_duplicates() {
        assert_eq!(
            normalize_nonspeech_markers("[Musik] (MUSIC), [musikk]", Language::Swedish),
            "[MUSIK]"
        );
    }

    #[test]
    fn uses_configured_language_for_the_marker() {
        assert_eq!(
            normalize_nonspeech_markers("Music Music Music", Language::English),
            "[MUSIC]"
        );
        assert_eq!(
            normalize_nonspeech_markers("Musik Musik Musik", Language::Norwegian),
            "[MUSIKK]"
        );
        assert_eq!(
            normalize_nonspeech_markers("[MUSIC] Pidän musiikista", Language::Finnish),
            "[MUSIC] Pidän musiikista"
        );
    }

    #[test]
    fn separated_runs_are_not_merged() {
        assert_eq!(
            normalize_nonspeech_markers(
                "Musik Musik Musik tal Musik Musik Musik",
                Language::Swedish,
            ),
            "[MUSIK] tal [MUSIK]"
        );
    }

    #[test]
    fn normalization_is_idempotent() {
        let once = normalize_nonspeech_markers("Musik\tMusik  Musik [Musik]", Language::Swedish);
        assert_eq!(once, "[MUSIK]");
        assert_eq!(normalize_nonspeech_markers(&once, Language::Swedish), once);
    }

    #[test]
    fn no_op_preserves_whitespace_byte_for_byte() {
        let transcript = "  Jag  gillar\tmusik.\nNästa rad.  ";
        assert_eq!(
            normalize_nonspeech_markers(transcript, Language::Swedish),
            transcript
        );
    }

    #[test]
    fn removes_repeated_blank_audio_markers_from_display() {
        for language in [Language::Swedish, Language::English] {
            assert_eq!(
                normalize_nonspeech_markers("[BLANK_AUDIO] [BLANK_AUDIO] [BLANK_AUDIO]", language,),
                ""
            );
        }
    }

    #[test]
    fn removes_blank_audio_between_real_words() {
        assert_eq!(
            normalize_nonspeech_markers("One.\n\n[BLANK_AUDIO]\n\nTwo.", Language::English),
            "One.\n\nTwo."
        );
        assert_eq!(
            normalize_nonspeech_markers("First.\n\nHello [BLANK_AUDIO] world", Language::English),
            "First.\n\nHello world"
        );
        assert_eq!(
            normalize_nonspeech_markers("Hello [BLANK_AUDIO] world", Language::English),
            "Hello world"
        );
        assert_eq!(
            normalize_nonspeech_markers("Hello[BLANK_AUDIO]world", Language::English),
            "Hello world"
        );
        assert_eq!(
            normalize_nonspeech_markers("Hello  [BLANK_AUDIO]   world", Language::English),
            "Hello world"
        );
    }

    #[test]
    fn removes_adjacent_and_attached_blank_audio_artifacts() {
        assert_eq!(
            normalize_nonspeech_markers("[BLANK_AUDIO][BLANK_AUDIO]", Language::Swedish),
            ""
        );
        assert_eq!(
            normalize_nonspeech_markers("[BLANK_AUDIO].", Language::Swedish),
            ""
        );
        assert_eq!(
            normalize_nonspeech_markers("Hello [BLANK_AUDIO].", Language::English),
            "Hello."
        );
    }

    #[test]
    fn preserves_raw_segment_text_and_genuine_thanks() {
        let raw_segment = crate::transcription::TranscriptSegment {
            start: 0.0,
            end: 1.0,
            text: "Hello [BLANK_AUDIO] world".to_string(),
            avg_logprob: None,
            no_speech_prob: 0.0,
        };
        let raw_json = serde_json::to_value(&raw_segment).unwrap();
        assert_eq!(
            normalize_nonspeech_markers(&raw_segment.text, Language::English),
            "Hello world"
        );
        assert_eq!(serde_json::to_value(&raw_segment).unwrap(), raw_json);
        assert_eq!(
            normalize_nonspeech_markers("Tack! Tack! Tack.", Language::Swedish),
            "Tack! Tack! Tack."
        );
        assert_eq!(
            normalize_nonspeech_markers("[BLANK_AUDIO] Tack! Tack! Tack.", Language::Swedish),
            "Tack! Tack! Tack."
        );
        assert_eq!(
            normalize_nonspeech_markers("Thank you", Language::English),
            "Thank you"
        );
    }
}
