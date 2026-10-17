//! Test fixtures made of core types, shared by the workspace's tests (#363).
//!
//! Compiled for this crate's unit tests (`cfg(test)`) and, through the
//! `test-support` feature that `sagascript-cli` and the app enable as a
//! dev-dependency, for theirs; release builds never contain it. Fixtures that
//! need no core type (temporary directories, a test's own settings file,
//! SHA-256, the isolated CLI environment) are in the `sagascript-testsupport`
//! crate, which core's unit tests can use because it does not depend on core.

use std::cell::Cell;
use std::fs::File;
use std::io;
use std::path::Path;

use crate::fsutil::{PublishFs, RealFs};
use crate::meeting::{MeetingSegmentInput, MeetingSpeaker, MeetingTranscript};

/// The real filesystem with chosen failures injected into a publication
/// ([`PublishFs`], #342): the n-th hard link, every unlink, the creation of
/// the staging file, the file fsync or the folder fsync. Build one with [`FaultyFs::new`] and
/// the `failing_*` methods.
#[derive(Debug, Default)]
pub struct FaultyFs {
    fail_link_number: usize,
    fail_remove: bool,
    fail_create: bool,
    sync_file_error: Option<io::ErrorKind>,
    sync_dir_error: Option<io::ErrorKind>,
    links: Cell<usize>,
}

impl FaultyFs {
    /// A filesystem that fails nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Fail the `number`-th `hard_link` (counting from 1).
    pub fn failing_link(mut self, number: usize) -> Self {
        self.fail_link_number = number;
        self
    }

    /// Fail every `remove_file`, so a staging file stays behind.
    pub fn failing_remove(mut self) -> Self {
        self.fail_remove = true;
        self
    }

    /// Fail the creation of the staging file, which then does not exist.
    pub fn failing_create(mut self) -> Self {
        self.fail_create = true;
        self
    }

    /// Fail every folder fsync with an error of this kind.
    pub fn failing_sync_dir(mut self, kind: io::ErrorKind) -> Self {
        self.sync_dir_error = Some(kind);
        self
    }

    /// Fail every file fsync with an error of this kind.
    pub fn failing_sync_file(mut self, kind: io::ErrorKind) -> Self {
        self.sync_file_error = Some(kind);
        self
    }
}

impl PublishFs for FaultyFs {
    fn create_new(&self, path: &Path) -> io::Result<File> {
        if self.fail_create {
            return Err(io::Error::other("injected create failure"));
        }
        RealFs.create_new(path)
    }

    fn sync_file(&self, file: &File) -> io::Result<()> {
        match self.sync_file_error {
            Some(kind) => Err(io::Error::new(kind, "injected sync failure")),
            None => RealFs.sync_file(file),
        }
    }

    fn hard_link(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.links.set(self.links.get() + 1);
        if self.links.get() == self.fail_link_number {
            return Err(io::Error::other("injected link failure"));
        }
        RealFs.hard_link(from, to)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        if self.fail_remove {
            return Err(io::Error::other("injected unlink failure"));
        }
        RealFs.remove_file(path)
    }

    fn sync_dir(&self, dir: &Path) -> io::Result<()> {
        match self.sync_dir_error {
            Some(kind) => Err(io::Error::new(kind, "injected folder sync failure")),
            None => RealFs.sync_dir(dir),
        }
    }
}

/// Starts a synthetic [`MeetingTranscript`]: source hash `"a" * 64`, language
/// `"en"`, model `"tiny"`, 4 seconds, no segments and no speakers. Set what
/// the test relies on, then [`build`](MeetingTranscriptBuilder::build) it.
pub fn meeting_transcript() -> MeetingTranscriptBuilder {
    MeetingTranscriptBuilder {
        source_sha256: "a".repeat(64),
        language: "en".to_string(),
        model: "tiny".to_string(),
        duration_seconds: 4.0,
        segments: Vec::new(),
        speakers: Vec::new(),
    }
}

/// Builder returned by [`meeting_transcript`].
#[derive(Debug, Clone)]
#[must_use]
pub struct MeetingTranscriptBuilder {
    source_sha256: String,
    language: String,
    model: String,
    duration_seconds: f64,
    segments: Vec<MeetingSegmentInput>,
    speakers: Vec<MeetingSpeaker>,
}

impl MeetingTranscriptBuilder {
    pub fn source_sha256(mut self, source_sha256: impl Into<String>) -> Self {
        self.source_sha256 = source_sha256.into();
        self
    }

    pub fn language(mut self, language: impl Into<String>) -> Self {
        self.language = language.into();
        self
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn duration(mut self, seconds: f64) -> Self {
        self.duration_seconds = seconds;
        self
    }

    /// Appends a segment; [`MeetingTranscript::new`] sorts them and assigns ids.
    pub fn segment(
        mut self,
        start: f64,
        end: f64,
        text: impl Into<String>,
        speaker: impl Into<String>,
    ) -> Self {
        self.segments.push(MeetingSegmentInput {
            start,
            end,
            text: text.into(),
            speaker: speaker.into(),
        });
        self
    }

    pub fn speaker(mut self, id: impl Into<String>, label: impl Into<String>) -> Self {
        self.speakers.push(MeetingSpeaker {
            id: id.into(),
            label: label.into(),
        });
        self
    }

    /// Builds through [`MeetingTranscript::new`], so the fixture passes the
    /// same validation as real documents; panics if it does not.
    pub fn build(self) -> MeetingTranscript {
        MeetingTranscript::new(
            self.source_sha256,
            self.language,
            self.model,
            self.duration_seconds,
            self.segments,
            self.speakers,
        )
        .unwrap_or_else(|error| panic!("synthetic meeting transcript is invalid: {error}"))
    }
}

/// The settings, profiles and prompts the callers of the run resolver are
/// compared on (#346). The CLI's `RunRequest` and the app's file transcription
/// each resolve a profile and a prompt, and each test checks its result against
/// `RunSelection::resolve(..)` then `into_context(..)` of this crate for every
/// row of this table, so the two are equal to each other through the resolver
/// without either depending on the other.
///
/// Every settings value carries an explicit file model, so resolving a model
/// from it never looks at the models downloaded on the machine running the test.
pub mod run_context_table {
    use crate::settings::{FileModelPreference, HotkeyProfile, Language, Settings, WhisperModel};
    use crate::transcription::Glossary;

    /// One shape of stored settings and the profile arguments to try on it.
    pub struct Shape {
        pub name: &'static str,
        pub settings: Settings,
        /// `None` is "no profile asked for": the default profile.
        pub profiles: Vec<Option<&'static str>>,
    }

    /// One-run prompts: none, a plain hint, a blank one, one with an alias
    /// (hint-only), and one with surrounding whitespace.
    pub const PROMPTS: [Option<&str>; 5] = [
        None,
        Some("Acme, Foo"),
        Some("  \n"),
        Some("Grimmer = grimmer"),
        Some("  Acme, Foo  "),
    ];

    pub fn profile(id: &str, language: Language) -> HotkeyProfile {
        HotkeyProfile {
            id: id.into(),
            name: id.into(),
            shortcut: format!("Super+{id}"),
            push_to_talk_shortcut: None,
            toggle_shortcut: None,
            language,
        }
    }

    fn explicit_model() -> FileModelPreference {
        // The multilingual Base fits every language.
        FileModelPreference::Whisper(WhisperModel::Base)
    }

    /// Profile dictionaries in force; the saved global hint is left behind.
    pub fn migrated() -> Settings {
        let mut settings = Settings {
            language: Language::English,
            initial_prompt: "LegacyOnly".into(),
            hotkey_profiles: vec![
                profile("default", Language::English),
                profile("swedish", Language::Swedish),
                profile("automatic", Language::Auto),
            ],
            profile_glossary_migrated: true,
            file_transcription_model: explicit_model(),
            ..Settings::default()
        };
        for (id, source) in [
            ("default", "Sagascript"),
            ("swedish", "merge = merch\nGrimnir"),
            ("automatic", "Akronym = akr"),
        ] {
            settings.profile_glossaries.insert(id.into(), source.into());
        }
        settings
    }

    /// Before profile dictionaries: no profile is called `default`, and the
    /// saved global hint is the base of every run.
    pub fn legacy() -> Settings {
        let mut settings = Settings {
            initial_prompt: "merge = merch\nOpenRouter".into(),
            hotkey_profiles: vec![
                profile("swedish", Language::Swedish),
                profile("english", Language::English),
            ],
            profile_glossary_migrated: false,
            file_transcription_model: explicit_model(),
            ..Settings::default()
        };
        settings
            .profile_glossaries
            .insert("swedish".into(), "fika = coffee".into());
        settings
    }

    /// No profiles stored at all: the legacy default profile of `language`.
    pub fn without_profiles() -> Settings {
        Settings {
            language: Language::Swedish,
            hotkey_profiles: Vec::new(),
            file_transcription_model: explicit_model(),
            ..Settings::default()
        }
    }

    pub fn shapes() -> Vec<Shape> {
        vec![
            Shape {
                name: "migrated",
                settings: migrated(),
                profiles: vec![None, Some("default"), Some("swedish"), Some("automatic")],
            },
            Shape {
                name: "legacy",
                settings: legacy(),
                profiles: vec![None, Some("swedish"), Some("english")],
            },
            Shape {
                name: "without profiles",
                settings: without_profiles(),
                profiles: vec![None, Some("default")],
            },
        ]
    }

    /// What a dictionary holds, comparable: each term with its aliases.
    pub fn entries(glossary: &Glossary) -> Vec<(String, Vec<String>)> {
        glossary
            .entries()
            .iter()
            .map(|entry| (entry.canonical.clone(), entry.aliases.clone()))
            .collect()
    }
}
