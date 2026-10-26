//! Explicit, foreground live-meeting capture for the CLI.
//!
//! This module deliberately keeps the CLI adapter thin: capture, transcription,
//! and temporary recording ownership remain in the core `Session`.  The CLI
//! only polls snapshots, renders incremental changes, and asks before exporting
//! or discarding the temporary result.

use std::collections::BTreeMap;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use clap::Args;
use sagascript_core::error::DictationError;
use sagascript_core::meeting::live::document::{Gap, LiveSegment, SegmentStatus, Source};
use sagascript_core::meeting::live::session::Session;
use sagascript_core::meeting::live::state::{
    Phase, Refinement, RefinementStatus, Snapshot, SourceStatus,
};
use sagascript_core::meeting::MeetingExportFormat;
use sagascript_core::settings::Language;
use serde::Serialize;

use crate::cancel;
use crate::output::{errln, outln, Out};

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(30);
const REFINE_TIMEOUT: Duration = Duration::from_secs(4 * 60 * 60);
const MAX_DURATION_SECONDS: f64 = 14_400.0;

#[derive(Args, Debug, Clone, Default)]
pub struct LiveArgs {
    /// Stop after this many seconds. Omit to continue until Ctrl-C.
    #[arg(long, value_name = "SECONDS", value_parser = validate_duration)]
    pub duration: Option<f64>,
    /// Emit one JSON object per status or changed segment.
    #[arg(long)]
    pub jsonl: bool,
    /// Explicitly save the final Swedish transcript to this path.
    #[arg(long, value_name = "PATH")]
    pub text_output: Option<PathBuf>,
    /// Explicitly save the captured audio to this path.
    #[arg(long, value_name = "PATH")]
    pub audio_output: Option<PathBuf>,
}

pub fn validate_duration(value: &str) -> Result<f64, String> {
    let parsed = value
        .parse::<f64>()
        .map_err(|_| "duration must be a finite number of seconds".to_owned())?;
    if !parsed.is_finite() || parsed <= 0.0 || parsed > MAX_DURATION_SECONDS {
        return Err(format!(
            "duration must be finite, greater than 0, and at most {MAX_DURATION_SECONDS} seconds"
        ));
    }
    Ok(parsed)
}

/// The stable, incremental wire events emitted by `--jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum LiveEvent {
    Status {
        id: String,
        revision: u64,
        phase: Phase,
        refinement: Option<Refinement>,
        elapsed_ms: u64,
        sources: Vec<SourceStatus>,
        error: Option<String>,
    },
    Segment {
        id: String,
        source: Source,
        start_ms: u64,
        end_ms: u64,
        text: String,
        status: SegmentStatus,
        revision: u64,
    },
    TranscriptReset {
        revision: u64,
    },
    Gap {
        source: Source,
        start_ms: u64,
        end_ms: u64,
        reason: String,
    },
}

#[derive(Default)]
pub struct EventReducer {
    session_id: Option<String>,
    phase: Option<Phase>,
    refinement: Option<Refinement>,
    error: Option<String>,
    sources: Vec<SourceStatus>,
    segments: BTreeMap<(u8, String), LiveSegment>,
    gaps: BTreeMap<(u8, u64, u64, String), Gap>,
}

impl EventReducer {
    /// Return only the status and segment changes since the preceding snapshot.
    /// A new session resets the reducer, so a reused CLI adapter cannot leak
    /// events from the previous capture.
    pub fn update(&mut self, snapshot: &Snapshot) -> Vec<LiveEvent> {
        if self.session_id.as_deref() != Some(snapshot.id.as_str()) {
            *self = Self {
                session_id: Some(snapshot.id.clone()),
                ..Self::default()
            };
        }

        let mut events = Vec::new();
        let refinement_completed = snapshot
            .refinement
            .as_ref()
            .is_some_and(|refinement| refinement.status == RefinementStatus::Completed);
        let first_refinement_completion = refinement_completed
            && !self
                .refinement
                .as_ref()
                .is_some_and(|refinement| refinement.status == RefinementStatus::Completed);

        if self.phase != Some(snapshot.phase)
            || self.refinement != snapshot.refinement
            || self.error != snapshot.error
            || self.sources != snapshot.sources
        {
            events.push(LiveEvent::Status {
                id: snapshot.id.clone(),
                revision: snapshot.revision,
                phase: snapshot.phase,
                refinement: snapshot.refinement.clone(),
                elapsed_ms: snapshot.elapsed_ms,
                sources: snapshot.sources.clone(),
                error: snapshot.error.clone(),
            });
            self.phase = Some(snapshot.phase);
            self.refinement = snapshot.refinement.clone();
            self.error = snapshot.error.clone();
            self.sources = snapshot.sources.clone();
        }

        if first_refinement_completion {
            self.segments.clear();
            events.push(LiveEvent::TranscriptReset {
                revision: snapshot.revision,
            });
        }

        for segment in &snapshot.segments {
            let key = (source_rank(segment.source), segment.id.clone());
            if self.segments.get(&key) != Some(segment) {
                events.push(LiveEvent::Segment {
                    id: segment.id.clone(),
                    source: segment.source,
                    start_ms: segment.start_ms,
                    end_ms: segment.end_ms,
                    text: segment.text.clone(),
                    status: segment.status,
                    revision: segment.revision,
                });
                self.segments.insert(key, segment.clone());
            }
        }
        for gap in &snapshot.gaps {
            let key = (
                source_rank(gap.source),
                gap.start_ms,
                gap.end_ms,
                gap.reason.clone(),
            );
            if let std::collections::btree_map::Entry::Vacant(entry) = self.gaps.entry(key) {
                events.push(LiveEvent::Gap {
                    source: gap.source,
                    start_ms: gap.start_ms,
                    end_ms: gap.end_ms,
                    reason: gap.reason.clone(),
                });
                entry.insert(gap.clone());
            }
        }
        events
    }
}

pub fn run(args: LiveArgs, out: &Out) -> Result<(), DictationError> {
    if !Session::supported() {
        return Err(DictationError::SettingsError(
            "Live meetings require Apple Silicon, macOS 14.2+, and the local Pianissimo model."
                .into(),
        ));
    }

    cancel::install();
    let mut settings = sagascript_core::machine::load();
    settings.language = Language::Swedish;
    settings.pianissimo_dictation = true;
    let session = Session::start(settings).map_err(DictationError::TranscriptionFailed)?;

    let result = run_session(&session, &args, out);
    if result.is_err() && !session.is_done() {
        session.shutdown();
        wait_until_done(&session, DRAIN_TIMEOUT);
    }
    result
}

fn run_session(session: &Arc<Session>, args: &LiveArgs, out: &Out) -> Result<(), DictationError> {
    let duration_ms = args
        .duration
        .map(|seconds| (seconds * 1000.0).round() as u64);
    let started = Instant::now();
    let mut reducer = EventReducer::default();
    let mut stop_requested = false;
    let mut stop_started = None;
    let mut refinement_started = None;
    let mut refinement_stop_requested = false;
    let mut interrupt_stopped_capture = false;

    while !session.is_done() {
        let snapshot = session.snapshot();
        if !stop_requested
            && (cancel::is_cancelled()
                || duration_ms.is_some_and(|limit| limit <= started.elapsed().as_millis() as u64))
        {
            interrupt_stopped_capture = cancel::is_cancelled()
                && matches!(snapshot.phase, Phase::Starting | Phase::Running);
            session.stop();
            stop_requested = true;
            stop_started = Some(Instant::now());
        }

        if snapshot.phase == Phase::Refining {
            refinement_started.get_or_insert_with(Instant::now);
            if cancel::is_cancelled() && !interrupt_stopped_capture && !refinement_stop_requested {
                session.stop();
                refinement_stop_requested = true;
            }
            if !refinement_stop_requested
                && refinement_started.is_some_and(|at| at.elapsed() >= REFINE_TIMEOUT)
            {
                errln!(
                    out,
                    "Efterbearbetningen tog för lång tid; avbryter efterbearbetningen och behåller live-texten."
                );
                session.stop();
                refinement_stop_requested = true;
            }
        }

        if stop_started.is_some_and(|at| at.elapsed() >= DRAIN_TIMEOUT)
            && matches!(snapshot.phase, Phase::Starting | Phase::Draining)
        {
            session.shutdown();
            let drained = wait_until_done(session, DRAIN_TIMEOUT);
            let detail = if drained {
                "Live meeting drain exceeded its time limit."
            } else {
                "Live meeting shutdown did not finish within its time limit."
            };
            return Err(DictationError::TranscriptionFailed(detail.into()));
        }

        emit_events(out, args.jsonl, reducer.update(&snapshot))?;
        if session.is_done() {
            break;
        }
        thread::sleep(POLL_INTERVAL);
    }

    let mut last = reducer.update(&session.snapshot());
    emit_events(out, args.jsonl, std::mem::take(&mut last))?;
    finish_session(session, args, out)
}

fn emit_events(out: &Out, jsonl: bool, events: Vec<LiveEvent>) -> Result<(), DictationError> {
    for event in events {
        if jsonl {
            out.jsonl(&event)?;
        } else {
            match event {
                LiveEvent::Status {
                    phase,
                    refinement,
                    elapsed_ms,
                    sources,
                    error,
                    ..
                } => {
                    errln!(
                        out,
                        "Status: {} ({:.1}s)",
                        phase_label(phase),
                        elapsed_ms as f64 / 1000.0
                    );
                    for source in sources {
                        if let Some(source_error) = source.error {
                            errln!(
                                out,
                                "  {}: fel: {source_error}",
                                source_label(source.source)
                            );
                        }
                    }
                    if let Some(error) = error {
                        errln!(out, "  fel: {error}");
                    }
                    if let Some(refinement) = refinement {
                        if phase == Phase::Refining {
                            errln!(out, "  Efterbearbetning: {}%", refinement.progress_percent);
                        }
                        if let Some(error) = refinement.error {
                            errln!(out, "  efterbearbetning: fel: {error}");
                        }
                    }
                }
                LiveEvent::Segment {
                    source,
                    text,
                    status,
                    ..
                } => {
                    outln!(
                        out,
                        "{} [{}] {}",
                        source_label(source),
                        status_label(status),
                        text
                    )?;
                }
                LiveEvent::TranscriptReset { .. } => {
                    outln!(out, "Slutlig text ersätter live-utskriften.")?;
                }
                LiveEvent::Gap {
                    source,
                    start_ms,
                    end_ms,
                    reason,
                } => {
                    errln!(
                        out,
                        "  Lucka {}–{} ms ({}): {}",
                        source_label(source),
                        start_ms,
                        end_ms,
                        reason
                    );
                }
            }
        }
    }
    Ok(())
}

fn finish_session(
    session: &Arc<Session>,
    args: &LiveArgs,
    out: &Out,
) -> Result<(), DictationError> {
    let interactive = io::stdin().is_terminal() && !args.jsonl;

    if interactive {
        if let Some(path) = args.text_output.as_ref() {
            export_text_interactive(session, path.clone(), out)?;
        } else {
            match prompt_path(out, "Spara text till sökväg (tomt för att hoppa över): ")? {
                Prompt::Value(path) => export_text_interactive(session, PathBuf::from(path), out)?,
                Prompt::Blank => {}
                Prompt::Eof => return Err(input_ended(out)),
            }
        }
        if let Some(path) = args.audio_output.as_ref() {
            export_audio_interactive(session, path.clone(), out)?;
        } else if session.snapshot().audio_available {
            match prompt_path(out, "Spara ljud till sökväg (tomt för att hoppa över): ")? {
                Prompt::Value(path) => export_audio_interactive(session, PathBuf::from(path), out)?,
                Prompt::Blank => {}
                Prompt::Eof => return Err(input_ended(out)),
            }
        }
        loop {
            match confirm_discard(out)? {
                LifecycleDecision::Discard => {
                    session
                        .discard()
                        .map_err(DictationError::TranscriptionFailed)?;
                    break;
                }
                LifecycleDecision::Continue => {
                    errln!(out, "Mötet behålls. Bekräfta kassering när du är klar.");
                }
                LifecycleDecision::Eof => return Err(input_ended(out)),
            }
        }
        if let Some(error) = terminal_failure(session) {
            return Err(DictationError::TranscriptionFailed(error));
        }
    } else {
        if let Some(path) = args.text_output.as_ref() {
            export_text(session, path, out)?;
        }
        if let Some(path) = args.audio_output.as_ref() {
            export_audio(session, path, out)?;
        }
        if args.text_output.is_none() && args.audio_output.is_none() {
            errln!(out, "Mötet är klart; inget explicit exportmål angavs.");
        }
        if let Err(error) = session.discard() {
            return Err(DictationError::TranscriptionFailed(error));
        }
        if let Some(error) = terminal_failure(session) {
            return Err(DictationError::TranscriptionFailed(error));
        }
    }
    Ok(())
}

fn export_text(session: &Arc<Session>, path: &Path, out: &Out) -> Result<(), DictationError> {
    session
        .export_text(path, MeetingExportFormat::Plain)
        .map(|_| errln!(out, "Text sparad."))
        .map_err(|error| {
            errln!(
                out,
                "Text kunde inte sparas: {error}. Tillfälligt möte kan inte överleva processen."
            );
            DictationError::TranscriptionFailed(error)
        })
}

fn export_audio(session: &Arc<Session>, path: &Path, out: &Out) -> Result<(), DictationError> {
    session
        .export_audio(path)
        .map(|_| errln!(out, "Ljud sparat."))
        .map_err(|error| {
            errln!(
                out,
                "Ljud kunde inte sparas: {error}. Tillfälligt möte kan inte överleva processen."
            );
            DictationError::TranscriptionFailed(error)
        })
}

fn export_text_interactive(
    session: &Arc<Session>,
    mut path: PathBuf,
    out: &Out,
) -> Result<(), DictationError> {
    loop {
        match session.export_text(&path, MeetingExportFormat::Plain) {
            Ok(_) => {
                errln!(out, "Text sparad.");
                return Ok(());
            }
            Err(error) => {
                errln!(out, "Text kunde inte sparas: {error}");
                match prompt_path(out, "Ny textsökväg (tomt för att hoppa över): ")? {
                    Prompt::Value(next) => path = PathBuf::from(next),
                    Prompt::Blank => return Ok(()),
                    Prompt::Eof => return Err(input_ended(out)),
                }
            }
        }
    }
}

fn export_audio_interactive(
    session: &Arc<Session>,
    mut path: PathBuf,
    out: &Out,
) -> Result<(), DictationError> {
    loop {
        match session.export_audio(&path) {
            Ok(_) => {
                errln!(out, "Ljud sparat.");
                return Ok(());
            }
            Err(error) => {
                errln!(out, "Ljud kunde inte sparas: {error}");
                match prompt_path(out, "Ny ljudsökväg (tomt för att hoppa över): ")? {
                    Prompt::Value(next) => path = PathBuf::from(next),
                    Prompt::Blank => return Ok(()),
                    Prompt::Eof => return Err(input_ended(out)),
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Prompt {
    Value(String),
    Blank,
    Eof,
}

fn prompt_path(out: &Out, prompt: &str) -> Result<Prompt, DictationError> {
    out.err_text(prompt);
    let mut input = String::new();
    let bytes = io::stdin()
        .read_line(&mut input)
        .map_err(|error| DictationError::TranscriptionFailed(error.to_string()))?;
    if bytes == 0 {
        return Ok(Prompt::Eof);
    }
    let path = input.trim();
    Ok(if path.is_empty() {
        Prompt::Blank
    } else {
        Prompt::Value(path.to_owned())
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LifecycleDecision {
    Discard,
    Continue,
    Eof,
}

fn confirm_discard(out: &Out) -> Result<LifecycleDecision, DictationError> {
    let prompt = prompt_path(
        out,
        "Kasta den tillfälliga inspelningen? skriv 'ja' för att bekräfta: ",
    )?;
    Ok(match prompt {
        Prompt::Eof => LifecycleDecision::Eof,
        Prompt::Blank => LifecycleDecision::Continue,
        Prompt::Value(value) => lifecycle_decision(Some(value.as_str())),
    })
}

fn lifecycle_decision(value: Option<&str>) -> LifecycleDecision {
    match value {
        None => LifecycleDecision::Eof,
        Some(value)
            if matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "y" | "yes" | "j" | "ja"
            ) =>
        {
            LifecycleDecision::Discard
        }
        Some(_) => LifecycleDecision::Continue,
    }
}

fn input_ended(out: &Out) -> DictationError {
    errln!(
        out,
        "Inmatningen avslutades; det tillfälliga mötet går förlorat när processen avslutas."
    );
    DictationError::TranscriptionFailed(
        "Interactive input ended before the temporary meeting was explicitly discarded.".into(),
    )
}

fn terminal_failure(session: &Session) -> Option<String> {
    let snapshot = session.snapshot();
    (snapshot.phase == Phase::Failed).then(|| {
        snapshot
            .error
            .unwrap_or_else(|| "Live meeting failed.".into())
    })
}

fn wait_until_done(session: &Session, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while !session.is_done() && Instant::now() < deadline {
        thread::sleep(POLL_INTERVAL);
    }
    session.is_done()
}

fn source_label(source: Source) -> &'static str {
    match source {
        Source::Microphone => "Jag · mikrofon",
        Source::System => "Mötet · systemljud",
    }
}

fn source_rank(source: Source) -> u8 {
    match source {
        Source::Microphone => 0,
        Source::System => 1,
    }
}

fn status_label(status: SegmentStatus) -> &'static str {
    match status {
        SegmentStatus::Preview => "förhandsvisning",
        SegmentStatus::Final => "slutlig",
    }
}

fn phase_label(phase: Phase) -> &'static str {
    match phase {
        Phase::Starting => "startar",
        Phase::Running => "pågår",
        Phase::Draining => "slutför",
        Phase::Refining => "förfinar",
        Phase::Completed => "klar",
        Phase::Cancelled => "avbruten",
        Phase::Failed => "misslyckad",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        lifecycle_decision, validate_duration, EventReducer, LifecycleDecision, LiveEvent,
    };
    use sagascript_core::meeting::live::document::{Gap, Source};
    use sagascript_core::meeting::live::state::{Phase, Refinement, RefinementStatus, Snapshot};

    fn snapshot(segments: Vec<sagascript_core::meeting::live::document::LiveSegment>) -> Snapshot {
        Snapshot {
            id: "session".into(),
            revision: 1,
            phase: Phase::Running,
            refinement: None,
            elapsed_ms: 100,
            sources: Vec::new(),
            segments,
            gaps: Vec::<Gap>::new(),
            error: None,
            audio_available: false,
        }
    }

    fn segment(text: &str, revision: u64) -> sagascript_core::meeting::live::document::LiveSegment {
        sagascript_core::meeting::live::document::LiveSegment {
            id: "1".into(),
            source: Source::Microphone,
            start_ms: 0,
            end_ms: 100,
            text: text.into(),
            status: sagascript_core::meeting::live::document::SegmentStatus::Preview,
            revision,
        }
    }

    #[test]
    fn duration_is_strictly_bounded() {
        assert!(validate_duration("0").is_err());
        assert!(validate_duration("NaN").is_err());
        assert!(validate_duration("14400").is_ok());
        assert!(validate_duration("14400.1").is_err());
    }

    #[test]
    fn reducer_replaces_same_source_preview_without_repeating_unchanged_text() {
        let mut reducer = EventReducer::default();
        assert_eq!(
            reducer.update(&snapshot(vec![segment("första", 1)])).len(),
            2
        );
        assert!(reducer
            .update(&snapshot(vec![segment("första", 1)]))
            .is_empty());
        let changed = reducer.update(&snapshot(vec![segment("andra", 2)]));
        assert!(changed
            .iter()
            .any(|event| matches!(event, LiveEvent::Segment { text, .. } if text == "andra")));
    }

    #[test]
    fn reducer_emits_each_gap_once() {
        let mut reducer = EventReducer::default();
        let mut with_gap = snapshot(Vec::new());
        with_gap.gaps.push(Gap {
            source: Source::System,
            start_ms: 100,
            end_ms: 200,
            reason: "dropped audio".into(),
        });
        assert!(reducer.update(&with_gap).iter().any(
            |event| matches!(event, LiveEvent::Gap { reason, .. } if reason == "dropped audio")
        ));
        assert!(reducer.update(&with_gap).is_empty());
    }

    #[test]
    fn reducer_emits_refinement_progress_as_status_changes() {
        let mut reducer = EventReducer::default();
        let mut running = snapshot(Vec::new());
        running.refinement = Some(Refinement {
            status: RefinementStatus::Running,
            progress_percent: 25,
            error: None,
        });
        assert!(reducer.update(&running).iter().any(|event| matches!(
            event,
            LiveEvent::Status {
                refinement: Some(Refinement {
                    progress_percent: 25,
                    ..
                }),
                ..
            }
        )));

        running.refinement.as_mut().unwrap().progress_percent = 50;
        assert!(reducer.update(&running).iter().any(|event| matches!(
            event,
            LiveEvent::Status {
                refinement: Some(Refinement {
                    progress_percent: 50,
                    ..
                }),
                ..
            }
        )));
    }

    #[test]
    fn reducer_resets_transcript_once_before_completed_segments() {
        let mut reducer = EventReducer::default();
        assert!(!reducer
            .update(&snapshot(vec![segment("live", 1)]))
            .is_empty());

        let mut completed = snapshot(vec![segment("final", 2)]);
        completed.phase = Phase::Completed;
        completed.refinement = Some(Refinement {
            status: RefinementStatus::Completed,
            progress_percent: 100,
            error: None,
        });
        let events = reducer.update(&completed);
        let reset_index = events
            .iter()
            .position(|event| matches!(event, LiveEvent::TranscriptReset { .. }))
            .expect("transcript reset");
        let segment_index = events
            .iter()
            .position(|event| matches!(event, LiveEvent::Segment { text, .. } if text == "final"))
            .expect("final segment");
        assert!(reset_index < segment_index);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, LiveEvent::TranscriptReset { .. }))
                .count(),
            1
        );
        assert!(reducer.update(&completed).is_empty());
    }

    #[test]
    fn refining_phase_has_swedish_label() {
        assert_eq!(super::phase_label(Phase::Refining), "förfinar");
    }

    #[test]
    fn lifecycle_requires_explicit_discard_and_reports_eof() {
        assert_eq!(lifecycle_decision(Some("nej")), LifecycleDecision::Continue);
        assert_eq!(lifecycle_decision(Some("ja")), LifecycleDecision::Discard);
        assert_eq!(lifecycle_decision(None), LifecycleDecision::Eof);
    }
}
