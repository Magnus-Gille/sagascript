//! The command-line side of the run context (#346).
//!
//! `transcribe`, `record` and `meeting reprocess` take the same four flags to
//! say which profile, language and dictionary a run uses: `--profile`,
//! `--language`, `--prompt` and `--prompt-file`. [`RunRequest`] holds them as
//! typed on the command line and resolves them through
//! [`sagascript_core::run_context`], which owns the semantics. What belongs to
//! the command line stays here: parsing a language name, and reading the
//! prompt file (whose error text names the path as it was typed).
//!
//! The model is not resolved here. Each command has its own rules and its own
//! error order, so it chooses its model from [`RunSelection::language`]
//! itself, between the two steps:
//!
//! - `transcribe` resolves in one call ([`RunRequest::resolve`]); its model
//!   choice comes after the prompt file has been read.
//! - `record` and `meeting reprocess` choose the model first and read the
//!   prompt file afterwards ([`RunRequest::select`], then
//!   [`RunRequest::finish`]), so an unusable model is reported before an
//!   unreadable prompt file.

use std::path::Path;

use sagascript_core::error::DictationError;
use sagascript_core::run_context::{RunContext, RunOverrides, RunSelection};
use sagascript_core::settings::Settings;

use crate::transcribe::{parse_language, resolve_one_run_prompt};

/// The profile, language and one-run prompt flags of one invocation, as typed.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RunRequest<'a> {
    pub profile: Option<&'a str>,
    pub language: Option<&'a str>,
    pub prompt: Option<&'a str>,
    pub prompt_file: Option<&'a Path>,
}

impl RunRequest<'_> {
    /// Step one: the profile (unknown ids are an error) and the language
    /// (`--language` is parsed here, an unknown name is an error). Reads no
    /// file. The CLI never accepts `--profile` together with `--language`, so
    /// which of the two is reported first cannot be observed.
    pub(crate) fn select(&self, settings: &Settings) -> Result<RunSelection, DictationError> {
        let language = self.language.map(parse_language).transpose()?;
        RunSelection::resolve(
            settings,
            RunOverrides {
                profile: self.profile,
                language,
            },
        )
    }

    /// Step two: read the one-run prompt (`--prompt-file` wins over
    /// `--prompt`; an unreadable file is an error) and compose the dictionary.
    pub(crate) fn finish(
        &self,
        settings: &Settings,
        selection: RunSelection,
    ) -> Result<RunContext, DictationError> {
        let prompt = resolve_one_run_prompt(self.prompt, self.prompt_file)?;
        Ok(selection.into_context(settings, prompt.as_deref()))
    }

    /// Both steps, for a command whose model choice comes after them.
    pub(crate) fn resolve(&self, settings: &Settings) -> Result<RunContext, DictationError> {
        let selection = self.select(settings)?;
        self.finish(settings, selection)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use sagascript_core::settings::{HotkeyProfile, Language};
    use sagascript_testsupport::TempDir;

    fn profile(id: &str, language: Language) -> HotkeyProfile {
        HotkeyProfile {
            id: id.to_string(),
            name: id.to_string(),
            shortcut: format!("Super+{id}"),
            language,
            push_to_talk_shortcut: None,
            toggle_shortcut: None,
        }
    }

    /// Default English profile with the dictionary `Sagascript`, a Swedish
    /// profile with an alias, profile dictionaries in force.
    fn settings() -> Settings {
        let mut settings = Settings {
            hotkey_profiles: vec![
                profile("default", Language::English),
                profile("swedish", Language::Swedish),
            ],
            profile_glossary_migrated: true,
            ..Default::default()
        };
        for (id, source) in [
            ("default", "Sagascript"),
            ("swedish", "merge = merch\nGrimnir"),
        ] {
            settings
                .profile_glossaries
                .insert(id.to_string(), source.to_string());
        }
        settings
    }

    type Entries = Vec<(String, Vec<String>)>;

    fn entries(context: &RunContext) -> Entries {
        context
            .glossary
            .entries()
            .iter()
            .map(|entry| (entry.canonical.clone(), entry.aliases.clone()))
            .collect()
    }

    fn expected(entries: &[(&str, &[&str])]) -> Entries {
        entries
            .iter()
            .map(|(canonical, aliases)| {
                (
                    canonical.to_string(),
                    aliases.iter().map(|alias| alias.to_string()).collect(),
                )
            })
            .collect()
    }

    /// The input files the table refers to by name.
    struct Inputs {
        dir: TempDir,
    }

    impl Inputs {
        fn new() -> Self {
            let dir = TempDir::new();
            std::fs::write(dir.join("hint.txt"), "  Acme, Foo \n").unwrap();
            std::fs::write(dir.join("blank.txt"), " \n\t").unwrap();
            std::fs::create_dir(dir.join("a-directory")).unwrap();
            Self { dir }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.dir.join(name)
        }
    }

    struct Case {
        name: &'static str,
        profile: Option<&'static str>,
        language: Option<&'static str>,
        prompt: Option<&'static str>,
        prompt_file: Option<&'static str>,
        /// `Ok((profile id, language, dictionary))` or the complete error text.
        expect: Result<
            (
                &'static str,
                Language,
                &'static [(&'static str, &'static [&'static str])],
            ),
            &'static str,
        >,
    }

    const SAGASCRIPT: &[(&str, &[&str])] = &[("Sagascript", &[])];
    const SWEDISH: &[(&str, &[&str])] = &[("merge", &["merch"]), ("Grimnir", &[])];

    #[test]
    fn request_table() {
        use Language::{English, Swedish};
        let inputs = Inputs::new();
        let settings = settings();
        let cases = [
            Case {
                name: "default profile",
                profile: None,
                language: None,
                prompt: None,
                prompt_file: None,
                expect: Ok(("default", English, SAGASCRIPT)),
            },
            Case {
                name: "explicit profile",
                profile: Some("swedish"),
                language: None,
                prompt: None,
                prompt_file: None,
                expect: Ok(("swedish", Swedish, SWEDISH)),
            },
            Case {
                name: "unknown profile",
                profile: Some("nope"),
                language: None,
                prompt: None,
                prompt_file: None,
                expect: Err("Settings error: Unknown dictation profile 'nope'"),
            },
            Case {
                name: "same language as the profile",
                profile: None,
                language: Some("en"),
                prompt: None,
                prompt_file: None,
                expect: Ok(("default", English, SAGASCRIPT)),
            },
            Case {
                name: "the long language name is the same language",
                profile: None,
                language: Some("english"),
                prompt: None,
                prompt_file: None,
                expect: Ok(("default", English, SAGASCRIPT)),
            },
            Case {
                name: "changed language drops the profile dictionary",
                profile: None,
                language: Some("sv"),
                prompt: None,
                prompt_file: None,
                expect: Ok(("default", Swedish, &[])),
            },
            Case {
                name: "unknown language",
                profile: None,
                language: Some("xx"),
                prompt: None,
                prompt_file: None,
                expect: Err("Settings error: Unknown language 'xx'. Valid: en, sv, no, fi, auto"),
            },
            Case {
                name: "inline prompt in front of the profile dictionary",
                profile: None,
                language: None,
                prompt: Some("  Acme, Foo  "),
                prompt_file: None,
                expect: Ok((
                    "default",
                    English,
                    &[("Acme", &[]), ("Foo", &[]), ("Sagascript", &[])],
                )),
            },
            Case {
                name: "prompt file is read and trimmed",
                profile: None,
                language: None,
                prompt: None,
                prompt_file: Some("hint.txt"),
                expect: Ok((
                    "default",
                    English,
                    &[("Acme", &[]), ("Foo", &[]), ("Sagascript", &[])],
                )),
            },
            Case {
                name: "prompt file wins over the inline prompt",
                profile: None,
                language: None,
                prompt: Some("Other"),
                prompt_file: Some("hint.txt"),
                expect: Ok((
                    "default",
                    English,
                    &[("Acme", &[]), ("Foo", &[]), ("Sagascript", &[])],
                )),
            },
            Case {
                name: "blank prompt file is no prompt",
                profile: None,
                language: None,
                prompt: None,
                prompt_file: Some("blank.txt"),
                expect: Ok(("default", English, SAGASCRIPT)),
            },
            Case {
                name: "changed language keeps only the prompt file",
                profile: None,
                language: Some("sv"),
                prompt: None,
                prompt_file: Some("hint.txt"),
                expect: Ok(("default", Swedish, &[("Acme", &[]), ("Foo", &[])])),
            },
            Case {
                name: "explicit profile and its prompt",
                profile: Some("swedish"),
                language: None,
                prompt: Some("Acme"),
                prompt_file: None,
                expect: Ok((
                    "swedish",
                    Swedish,
                    &[("Acme", &[]), ("merge", &["merch"]), ("Grimnir", &[])],
                )),
            },
        ];
        for case in cases {
            let prompt_file = case.prompt_file.map(|name| inputs.path(name));
            let request = RunRequest {
                profile: case.profile,
                language: case.language,
                prompt: case.prompt,
                prompt_file: prompt_file.as_deref(),
            };
            match (case.expect, request.resolve(&settings)) {
                (Ok((id, language, dictionary)), Ok(context)) => {
                    assert_eq!(context.profile.id, id, "{}: profile", case.name);
                    assert_eq!(context.language, language, "{}: language", case.name);
                    assert_eq!(
                        entries(&context),
                        expected(dictionary),
                        "{}: dictionary",
                        case.name
                    );
                }
                (Err(message), Err(error)) => {
                    assert!(
                        matches!(error, DictationError::SettingsError(_)),
                        "{}: {error:?}",
                        case.name
                    );
                    assert_eq!(error.to_string(), message, "{}: error", case.name);
                }
                (expect, got) => panic!("{}: expected {expect:?}, got {got:?}", case.name),
            }
        }
    }

    #[test]
    fn an_unreadable_prompt_file_is_a_file_decode_error_naming_the_path_as_typed() {
        let inputs = Inputs::new();
        let missing = PathBuf::from("no-such-folder").join("hint.txt");
        let directory = inputs.path("a-directory");
        for (name, path) in [("missing", missing.as_path()), ("a directory", &directory)] {
            let request = RunRequest {
                prompt_file: Some(path),
                ..RunRequest::default()
            };
            let error = request.resolve(&settings()).unwrap_err();
            assert!(
                matches!(error, DictationError::FileDecodeError(_)),
                "{name}: {error:?}"
            );
            assert_eq!(error.code(), "file_decode_error", "{name}");
            // Existing text, kept: the path is shown the way it was typed.
            let prefix = format!(
                "File decode error: Failed to read prompt file '{}': ",
                path.display()
            );
            let message = error.to_string();
            assert!(message.starts_with(&prefix), "{name}: {message}");
        }
        // The typed relative path is not made absolute: nothing in the
        // message names the working directory or the user's home.
        let request = RunRequest {
            prompt_file: Some(&missing),
            ..RunRequest::default()
        };
        let message = request.resolve(&settings()).unwrap_err().to_string();
        let cwd = std::env::current_dir().unwrap();
        assert!(!message.contains(&cwd.display().to_string()), "{message}");
    }

    #[test]
    fn errors_come_in_a_fixed_order_profile_then_language_then_prompt_file() {
        let missing = PathBuf::from("no-such-folder").join("hint.txt");
        let settings = settings();
        let with_file = |profile, language| RunRequest {
            profile,
            language,
            prompt: None,
            prompt_file: Some(&missing),
        };
        let text = |request: RunRequest<'_>| request.resolve(&settings).unwrap_err().to_string();

        assert_eq!(
            text(with_file(Some("nope"), None)),
            "Settings error: Unknown dictation profile 'nope'"
        );
        assert_eq!(
            text(with_file(None, Some("xx"))),
            "Settings error: Unknown language 'xx'. Valid: en, sv, no, fi, auto"
        );
        assert!(text(with_file(None, None)).contains("Failed to read prompt file"));
    }

    #[test]
    fn select_reads_no_file_and_finish_reads_it() {
        // `record` and `meeting reprocess` choose their model between the two
        // steps, so a prompt file that cannot be read must not surface from the
        // first (the model's own error comes first).
        let missing = PathBuf::from("no-such-folder").join("hint.txt");
        let settings = settings();
        let request = RunRequest {
            prompt_file: Some(&missing),
            ..RunRequest::default()
        };
        let selection = request.select(&settings).expect("select reads no file");
        assert_eq!(selection.profile.id, "default");
        assert_eq!(selection.language, Language::English);
        let error = request.finish(&settings, selection).unwrap_err();
        assert_eq!(error.code(), "file_decode_error");
    }

    #[test]
    fn the_two_steps_equal_the_one_call() {
        let inputs = Inputs::new();
        let hint = inputs.path("hint.txt");
        let settings = settings();
        for (profile, language) in [
            (None, None),
            (Some("swedish"), None),
            (None, Some("sv")),
            (None, Some("en")),
        ] {
            let request = RunRequest {
                profile,
                language,
                prompt: None,
                prompt_file: Some(&hint),
            };
            let one = request.resolve(&settings).unwrap();
            let selection = request.select(&settings).unwrap();
            let two = request.finish(&settings, selection).unwrap();
            assert_eq!(one.profile, two.profile);
            assert_eq!(one.language, two.language);
            assert_eq!(entries(&one), entries(&two));
        }
    }

    /// The same flags through each command's own argument type give the same
    /// profile, language and dictionary (#346: they used to resolve apart).
    #[cfg(all(feature = "diarization", feature = "record"))]
    #[test]
    fn the_three_commands_resolve_the_same_context_for_the_same_flags() {
        use clap::Parser;

        use crate::meeting_reprocessing_cli::RuntimeArgs;
        use crate::record::RecordArgs;
        use crate::transcribe::TranscribeArgs;

        #[derive(Parser)]
        struct Transcribe {
            #[command(flatten)]
            args: TranscribeArgs,
        }
        #[derive(Parser)]
        struct Reprocess {
            #[command(flatten)]
            args: RuntimeArgs,
        }
        #[derive(Parser)]
        struct Record {
            #[command(flatten)]
            args: RecordArgs,
        }

        let inputs = Inputs::new();
        let hint = inputs.path("hint.txt").display().to_string();
        let settings = settings();
        let flag_sets: Vec<Vec<&str>> = vec![
            vec![],
            vec!["--profile", "swedish"],
            vec!["--language", "sv"],
            vec!["--language", "en"],
            vec!["--prompt", "Acme, Foo"],
            vec!["--profile", "swedish", "--prompt-file", &hint],
            vec!["--language", "sv", "--prompt-file", &hint],
            vec!["--language", "auto", "--prompt", "Acme"],
        ];
        for flags in flag_sets {
            let transcribe = Transcribe::try_parse_from(
                ["sagascript", "a.wav"]
                    .into_iter()
                    .chain(flags.iter().copied()),
            )
            .unwrap();
            let reprocess =
                Reprocess::try_parse_from(["sagascript"].into_iter().chain(flags.iter().copied()))
                    .unwrap();
            let record =
                Record::try_parse_from(["sagascript"].into_iter().chain(flags.iter().copied()))
                    .unwrap();

            let from_transcribe = RunRequest::from(&transcribe.args)
                .resolve(&settings)
                .unwrap();
            let reprocess_request = RunRequest::from(&reprocess.args);
            let selection = reprocess_request.select(&settings).unwrap();
            let from_reprocess = reprocess_request.finish(&settings, selection).unwrap();
            let record_request = RunRequest::from(&record.args);
            let selection = record_request.select(&settings).unwrap();
            let from_record = record_request.finish(&settings, selection).unwrap();

            for (command, other) in [("reprocess", &from_reprocess), ("record", &from_record)] {
                assert_eq!(
                    from_transcribe.profile, other.profile,
                    "{command} {flags:?}: profile"
                );
                assert_eq!(
                    from_transcribe.language, other.language,
                    "{command} {flags:?}: language"
                );
                assert_eq!(
                    entries(&from_transcribe),
                    entries(other),
                    "{command} {flags:?}: dictionary"
                );
            }
        }
    }
}
