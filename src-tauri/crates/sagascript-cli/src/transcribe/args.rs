//! The `sagascript transcribe` command line (#343). The flags, their help text
//! and their value parsers are the CLI contract.

use std::path::PathBuf;

use clap::Args;

#[cfg(feature = "diarization")]
use sagascript_core::error::DictationError;

use crate::run_request::RunRequest;

#[derive(Args)]
pub struct TranscribeArgs {
    /// Emit JSON progress events on stderr even without a terminal. Percent is
    /// local to the named stage; stdout remains the transcript output. Diarized
    /// events include the effective decoder settings.
    #[arg(long)]
    pub progress_json: bool,
    /// Audio/video files or directories to transcribe. Directories include
    /// supported files in deterministic filename order.
    #[arg(required = true, value_name = "INPUT")]
    pub files: Vec<PathBuf>,

    /// Recurse into input directories. Has no effect on explicit files.
    #[arg(long)]
    pub recursive: bool,

    /// Language for transcription [possible values: en, sv, no, fi, auto (less accurate)]
    #[arg(short, long, value_name = "LANG")]
    pub language: Option<String>,

    /// Use this dictation profile's language and personal dictionary
    #[arg(long, value_name = "ID", conflicts_with = "language")]
    pub profile: Option<String>,

    /// File model ID to use [see: sagascript list-models]
    #[arg(short, long, value_name = "MODEL_ID")]
    pub model: Option<String>,

    /// Output result as JSON: text, language, model, duration, and a
    /// `segments` array with per-segment timing/confidence plus
    /// `coverage_ratio`, `uncovered_spans`, repetition quarantine spans,
    /// sampled language regions/probabilities, and warnings. Raw segments
    /// include `quarantined`.
    #[arg(long)]
    pub json: bool,

    /// Emit one compact {source,status,result|error} JSON object per input.
    #[arg(long, conflicts_with = "json")]
    pub jsonl: bool,

    /// Stop after the first item-level failure. The default continues and
    /// exits non-zero after emitting every successful/failed item.
    #[arg(long)]
    pub fail_fast: bool,

    /// Copy transcription result to clipboard
    #[arg(long)]
    pub clipboard: bool,

    /// Enable speaker diarization (requires diarization models — run: sagascript download-model diarization)
    #[cfg(feature = "diarization")]
    #[arg(long)]
    pub diarize: bool,

    /// Experimental: embed only same-region samples where one track is active
    /// and all other diarization tracks are inactive.
    #[cfg(feature = "diarization")]
    #[arg(long, requires = "diarize")]
    pub diarize_exclusive_speech_embeddings: bool,

    /// Emit the validated shared meeting transcript JSON document.
    /// Requires diarization and cannot be combined with legacy JSON outputs.
    #[cfg(feature = "diarization")]
    #[arg(
        long,
        requires = "diarize",
        conflicts_with_all = ["json", "jsonl", "clipboard", "correct_hints"]
    )]
    pub meeting_json: bool,

    /// Agglomerative clustering threshold for speaker diarization (0.0–2.0, default 0.34). Higher = fewer speakers.
    #[cfg(feature = "diarization")]
    #[arg(long, value_name = "THRESHOLD", default_value = "0.34",
          value_parser = parse_diarize_threshold,
          help = "Agglomerative clustering threshold for speaker diarization (0.0–2.0, default 0.34). Higher = fewer speakers.")]
    pub diarize_threshold: f32,

    /// Include local region/word evidence, distances and ASR diagnostics in JSON.
    /// These measurements are not calibrated speaker probabilities.
    #[cfg(feature = "diarization")]
    #[arg(long, requires = "diarize")]
    pub diarize_diagnostics: bool,

    #[cfg(feature = "diarization")]
    #[command(flatten)]
    pub speaker_count: crate::speaker_args::SpeakerCountArgs,

    /// Read/write reusable threshold-independent diarization analysis and
    /// word timestamps. A matching cache makes threshold-only retries fast.
    #[cfg(feature = "diarization")]
    #[arg(long, value_name = "PATH", requires = "diarize")]
    pub diarize_cache: Option<PathBuf>,

    /// Hint the decoder with domain-specific vocabulary (Whisper initial prompt).
    /// Reduces mishearings of proper nouns, foreign names, and jargon by priming
    /// the model with likely terms.
    /// Example: --hint "Notre Dame, Sara, Estrid, Grimnir, MCP, Fortnox"
    #[arg(long, visible_alias = "hint", value_name = "TEXT")]
    pub prompt: Option<String>,

    /// Read the hint/initial prompt from a file instead of the command line
    /// (handy for longer vocabulary lists). Mutually exclusive with --hint/--prompt.
    /// Leading/trailing whitespace is trimmed; an empty file means no hint.
    #[arg(
        long,
        visible_alias = "hint-file",
        value_name = "PATH",
        conflicts_with = "prompt"
    )]
    pub prompt_file: Option<PathBuf>,

    /// Pianissimo only: steer decoding with the profile glossary terms (context
    /// biasing). The value is the logit bonus for dictionary-continuing tokens
    /// (try 1-5; 0 or absent = off).
    #[arg(long, value_name = "WEIGHT")]
    pub glossary_boost: Option<f32>,

    /// Pianissimo only: extra context-biasing terms, one per line (experiments).
    /// Used together with --glossary-boost.
    #[arg(long, value_name = "FILE", requires = "glossary_boost")]
    pub boost_terms_file: Option<PathBuf>,

    /// Opt in to strict one-edit vocabulary correction from the selected hint.
    /// Requires --json so every applied correction is reported with its source
    /// text and segment confidence. Only single-word, unambiguous hints apply.
    #[arg(long, requires = "json")]
    pub correct_hints: bool,

    /// Enable voice activity detection (Silero VAD) to skip non-speech regions,
    /// reducing silence hallucination and repetition loops. Downloads a small
    /// model on first use. Overrides the `vad_enabled` setting. Diarized VAD is
    /// opt-in even when the saved setting is enabled, and
    /// uses source-mapped segment timing instead of DTW word timing.
    #[arg(long)]
    pub vad: bool,

    /// Disable VAD for this run, even if the `vad_enabled` setting is on.
    #[arg(long, conflicts_with = "vad")]
    pub no_vad: bool,

    /// Beam search width: 0 = greedy (fast), >=2 = beam search (more accurate,
    /// slower). Overrides the saved `beam_size` setting. When omitted, a saved
    /// `beam_size` >=2 is used; otherwise ordinary file transcription defaults
    /// to 5. Diarized transcription always defaults to 0 regardless of saved
    /// settings and accepts explicit values of 0 or 2–8.
    #[arg(long = "beam", value_name = "N")]
    pub beam_size: Option<u32>,

    /// Split long files across this many concurrent Whisper states (1–4).
    /// When omitted, long beam-search files use two states for measured-safe speedup.
    #[arg(
        long,
        value_name = "N",
        value_parser = parse_parallel_chunks
    )]
    #[cfg_attr(feature = "diarization", arg(conflicts_with = "diarize"))]
    pub parallel: Option<usize>,
}

impl<'a> From<&'a TranscribeArgs> for RunRequest<'a> {
    fn from(args: &'a TranscribeArgs) -> Self {
        Self {
            profile: args.profile.as_deref(),
            language: args.language.as_deref(),
            prompt: args.prompt.as_deref(),
            prompt_file: args.prompt_file.as_deref(),
        }
    }
}

/// The validated speaker-count hint of the speaker-count flags.
#[cfg(feature = "diarization")]
pub(super) fn validated_speaker_hint(
    counts: &sagascript_core::diarization::speaker_hint::SpeakerCountFlags,
) -> Result<Option<sagascript_core::diarization::speaker_hint::SpeakerCountHint>, DictationError> {
    counts.validated()
}

/// The validated speaker-count hint from `--speakers`/`--min-speakers`/`--max-speakers`.
#[cfg(feature = "diarization")]
pub(super) fn speaker_hint_from_args(
    args: &TranscribeArgs,
) -> Result<Option<sagascript_core::diarization::speaker_hint::SpeakerCountHint>, DictationError> {
    let hint = validated_speaker_hint(&args.speaker_count.into())?;
    if hint.is_some() && !args.diarize {
        return Err(DictationError::SettingsError(
            "--speakers, --min-speakers, --max-speakers and --force-speakers require --diarize"
                .to_string(),
        ));
    }
    Ok(hint)
}

/// Validates a `--diarize-threshold` value: must parse as a finite f32 in the
/// documented 0.0-2.0 range. NaN/infinite or out-of-range values silently
/// produce degenerate agglomerative clustering downstream, so reject them at
/// the CLI boundary rather than at the clustering call site.
#[cfg(feature = "diarization")]
pub(super) fn parse_diarize_threshold(s: &str) -> Result<f32, String> {
    let value: f32 = s
        .parse()
        .map_err(|_| format!("'{s}' is not a valid number"))?;
    if !value.is_finite() {
        return Err(format!(
            "diarize-threshold must be a finite number, got '{s}'"
        ));
    }
    if !(0.0..=2.0).contains(&value) {
        return Err(format!(
            "diarize-threshold must be between 0.0 and 2.0, got {value}"
        ));
    }
    Ok(value)
}

pub(super) fn parse_parallel_chunks(s: &str) -> Result<usize, String> {
    let value = s
        .parse::<usize>()
        .map_err(|_| format!("'{s}' is not a valid chunk count"))?;
    if !(1..=4).contains(&value) {
        return Err(format!("parallel must be between 1 and 4, got {value}"));
    }
    Ok(value)
}
