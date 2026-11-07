//! `sagascript recordings`: list, inspect, export, recover and delete the
//! recording folders `sagascript record` and the app write (SAGA-564, design
//! §4.7). The library work lives in `sagascript_core::recording`; this module
//! is arguments, tables and JSON shapes.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;
use unicode_width::UnicodeWidthStr;

use sagascript_core::error::DictationError;
use sagascript_core::recording::{
    delete_recording_folder, export_recording, list_recordings, read_recording, recover_folder,
    summarize_folder, ExportFormat, ExportLayout, ExportOptions, Manifest, RecordingError,
    RecordingState, RecordingStore, RecordingSummary, RecoveryOutcome, Track,
};
use sagascript_core::settings::Settings;

use crate::output::{errln, outln, Out};
use crate::recording_text::{ended_reason_id, format_bytes, format_hms, quality_id, short_when};

#[derive(Args)]
pub struct RecordingsArgs {
    #[command(subcommand)]
    pub action: RecordingsAction,
}

/// Options every action takes.
#[derive(Args, Clone, Debug, Default)]
pub struct Common {
    /// Folder that holds the recordings (default: the recording_directory setting, else
    /// Sagascript in your Music folder)
    #[arg(long, value_name = "DIR")]
    pub dir: Option<PathBuf>,

    /// Machine-readable JSON on stdout
    #[arg(long)]
    pub json: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum FormatArg {
    Flac,
    Wav,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum LayoutArg {
    Tracks,
    Stereo,
    Mixdown,
}

#[derive(Subcommand)]
pub enum RecordingsAction {
    /// List the recordings, newest first
    #[command(
        long_about = "List the recordings in the recordings folder, newest first: ID, start time, title, length, quality, size and state. A recording whose state is 'needs recovery' was interrupted (a crash, a power loss); run 'sagascript recordings recover'. With --json the output is an array of recording summaries.",
        after_long_help = "EXAMPLES:\n  sagascript recordings list\n  sagascript recordings list --dir ~/Music/Takes --json"
    )]
    List {
        #[command(flatten)]
        common: Common,
    },

    /// Show one recording and its tracks
    #[command(
        long_about = "Show one recording: title, start time, state, length, size, gaps and each track's codec, sample rate, channels, frames and gaps. The recording is a folder name, a recording id (or a prefix of at least 6 characters) or the path of the folder. The input device is shown in the text output only; --json leaves it out.",
        after_long_help = "EXAMPLES:\n  sagascript recordings show a1b2c3d4\n  sagascript recordings show \"2026-11-05 14.02 Team meeting [a1b2c3]\" --json"
    )]
    Show {
        /// Folder name, recording id (prefix of 6 or more characters) or folder path
        #[arg(value_name = "ID_OR_FOLDER")]
        target: String,
        #[command(flatten)]
        common: Common,
    },

    /// Export a recording as FLAC or WAV
    #[command(
        long_about = "Export a finished recording.\n\n--layout tracks writes one file per track, unchanged in sample rate (FLAC is a byte copy of the stored file; WAV is 24-bit PCM) into the directory given by --output. stereo (track 1 left, track 2 right) and mixdown (the tracks added) write one file to the path given by --output at the highest sample rate of the tracks; the result is derived, the other rates are resampled, and a note says so on stderr and in the JSON output.\n\nAn existing output is never overwritten, and the output cannot be inside the recording folder. Prints the files written on stdout.",
        after_long_help = "EXAMPLES:\n  sagascript recordings export a1b2c3d4 --output ./takes\n  sagascript recordings export a1b2c3d4 --format wav --layout mixdown --output talk.wav\n  sagascript recordings export a1b2c3d4 --layout stereo --output talk.flac --json"
    )]
    Export {
        /// Folder name, recording id (prefix of 6 or more characters) or folder path
        #[arg(value_name = "ID_OR_FOLDER")]
        target: String,

        /// File format
        #[arg(long, value_enum, default_value = "flac")]
        format: FormatArg,

        /// tracks: one file per track into the --output directory; stereo, mixdown: one file
        #[arg(long, value_enum, default_value = "tracks")]
        layout: LayoutArg,

        /// Directory (layout tracks) or file (stereo, mixdown) to create
        #[arg(short, long, value_name = "PATH")]
        output: PathBuf,

        #[command(flatten)]
        common: Common,
    },

    /// Finish recordings that a crash or power loss interrupted
    #[command(
        long_about = "Finish interrupted recordings. A recording is interrupted when the process that wrote it died; its folder is still marked 'recording'. Recovery keeps every complete frame of audio, moves any bytes after the last complete frame to a '.tail' file instead of deleting them, and marks the recording complete and recovered.\n\nWithout an argument every interrupted recording in the folder is recovered. A recording that another process is still writing is never touched.",
        after_long_help = "EXAMPLES:\n  sagascript recordings recover\n  sagascript recordings recover a1b2c3d4 --json"
    )]
    Recover {
        /// Folder name, recording id (prefix of 6 or more characters) or folder path (default: all)
        #[arg(value_name = "ID_OR_FOLDER")]
        target: Option<String>,
        #[command(flatten)]
        common: Common,
    },

    /// Delete a recording folder
    #[command(
        long_about = "Delete a recording: its folder and everything in it is removed for good (the Sagascript app moves it to the Trash instead). Requires --yes. A recording that another process is still writing is refused.",
        after_long_help = "EXAMPLES:\n  sagascript recordings delete a1b2c3d4 --yes"
    )]
    Delete {
        /// Folder name, recording id (prefix of 6 or more characters) or folder path
        #[arg(value_name = "ID_OR_FOLDER")]
        target: String,

        /// Confirm the deletion
        #[arg(long)]
        yes: bool,

        #[command(flatten)]
        common: Common,
    },
}

impl RecordingsAction {
    /// Whether the run was asked for `--json`.
    pub fn json(&self) -> bool {
        match self {
            Self::List { common }
            | Self::Show { common, .. }
            | Self::Export { common, .. }
            | Self::Recover { common, .. }
            | Self::Delete { common, .. } => common.json,
        }
    }
}

/// The recordings folder and the recording a target names. A target with a
/// path separator is the folder itself (shell completion hands one over); its
/// parent is then the recordings folder and `--dir` is not consulted.
pub(crate) fn locate(
    common: &Common,
    target: &str,
    stored: &Settings,
) -> Result<(PathBuf, String), DictationError> {
    if target.contains(['/', '\\']) {
        let path = Path::new(target);
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| {
                DictationError::SettingsError("the recording path names no folder".into())
            })?;
        let root = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        };
        return Ok((root, name));
    }
    Ok((root_of(common, stored), target.to_string()))
}

fn root_of(common: &Common, stored: &Settings) -> PathBuf {
    common
        .dir
        .clone()
        .unwrap_or_else(|| stored.recordings_dir())
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

pub fn run(args: RecordingsArgs, out: &Out) -> Result<(), DictationError> {
    let stored = sagascript_core::machine::load();
    match args.action {
        RecordingsAction::List { common } => list(&root_of(&common, &stored), common.json, out),
        RecordingsAction::Show { target, common } => {
            let (root, name) = locate(&common, &target, &stored)?;
            show(&root, &name, common.json, out)
        }
        RecordingsAction::Export {
            target,
            format,
            layout,
            output,
            common,
        } => {
            let (root, name) = locate(&common, &target, &stored)?;
            export(&root, &name, format, layout, &output, common.json, out)
        }
        RecordingsAction::Recover { target, common } => {
            let (root, name) = match &target {
                Some(target) => {
                    let (root, name) = locate(&common, target, &stored)?;
                    (root, Some(name))
                }
                None => (root_of(&common, &stored), None),
            };
            recover(&root, name.as_deref(), common.json, out)
        }
        RecordingsAction::Delete {
            target,
            yes,
            common,
        } => {
            let (root, name) = locate(&common, &target, &stored)?;
            delete(&root, &name, yes, common.json, out)
        }
    }
}

// -- list ------------------------------------------------------------------

/// What the STATE column says.
pub(crate) fn state_label(summary: &RecordingSummary) -> &'static str {
    match (summary.state, summary.recovered, summary.needs_recovery) {
        (RecordingState::Complete, true, _) => "recovered",
        (RecordingState::Complete, false, _) => "complete",
        (RecordingState::Recording, _, true) => "needs recovery",
        (RecordingState::Recording, _, false) => "in progress",
    }
}

const HEADERS: [&str; 7] = ["ID", "WHEN", "TITLE", "LENGTH", "QUALITY", "SIZE", "STATE"];
const TITLE_WIDTH: usize = 40;

fn truncate(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    let mut kept = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        kept.push(c);
        used += w;
    }
    kept.push('…');
    kept
}

/// One row of the table, as cells.
fn row(summary: &RecordingSummary) -> [String; 7] {
    [
        summary.recording_id.chars().take(8).collect(),
        short_when(&summary.started_at),
        truncate(&summary.title.replace(['\n', '\r', '\t'], " "), TITLE_WIDTH),
        format_hms(summary.duration_seconds),
        quality_id(summary.quality).to_string(),
        format_bytes(summary.bytes),
        state_label(summary).to_string(),
    ]
}

/// The table, header first, columns as wide as their widest cell; the last
/// column is not padded.
pub(crate) fn table_lines(summaries: &[RecordingSummary]) -> Vec<String> {
    let rows: Vec<[String; 7]> = summaries.iter().map(row).collect();
    let widths: Vec<usize> = (0..HEADERS.len())
        .map(|column| {
            rows.iter()
                .map(|row| UnicodeWidthStr::width(row[column].as_str()))
                .chain([HEADERS[column].len()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let render = |cells: &[&str]| {
        let mut line = String::new();
        for (column, cell) in cells.iter().enumerate() {
            if column + 1 == cells.len() {
                line.push_str(cell);
            } else {
                let fill = widths[column].saturating_sub(UnicodeWidthStr::width(*cell));
                line.push_str(cell);
                line.push_str(&" ".repeat(fill + 2));
            }
        }
        line
    };
    let mut lines = vec![render(&HEADERS)];
    lines.extend(rows.iter().map(|row| {
        let cells: Vec<&str> = row.iter().map(String::as_str).collect();
        render(&cells)
    }));
    lines
}

fn list(root: &Path, json: bool, out: &Out) -> Result<(), DictationError> {
    let summaries = list_recordings(root);
    if json {
        out.json(&summaries)?;
        return Ok(());
    }
    if summaries.is_empty() {
        errln!(out, "No recordings in {}.", absolute(root).display());
        return Ok(());
    }
    for line in table_lines(&summaries) {
        outln!(out, "{line}")?;
    }
    if summaries.iter().any(|s| s.needs_recovery) {
        errln!(
            out,
            "Some recordings were interrupted; run 'sagascript recordings recover' to finish them."
        );
    }
    Ok(())
}

// -- show ------------------------------------------------------------------

/// One track in `show --json`. The input device is a local detail and is left
/// out, so the object can be shared.
#[derive(Debug, Serialize)]
pub(crate) struct TrackJson {
    id: String,
    role: String,
    file: String,
    codec: String,
    lossless: bool,
    sample_rate: u32,
    bits: u8,
    channels: u8,
    frames: u64,
    duration_seconds: f64,
    gap_count: usize,
    gaps: Vec<GapJson>,
    ended_reason: Option<String>,
    clipped_samples: u64,
    resampled_from: Option<u32>,
}

#[derive(Debug, Serialize)]
pub(crate) struct GapJson {
    at_frame: u64,
    length: u64,
    seconds: f64,
    reason: String,
}

fn track_json(track: &Track) -> TrackJson {
    let rate = f64::from(track.sample_rate.max(1));
    TrackJson {
        id: track.id.clone(),
        role: track.role.clone(),
        file: track.file.clone(),
        codec: track.codec.clone(),
        lossless: track.lossless,
        sample_rate: track.sample_rate,
        bits: track.bits,
        channels: track.channels,
        frames: track.frames,
        duration_seconds: track.frames as f64 / rate,
        gap_count: track.gaps.len(),
        gaps: track
            .gaps
            .iter()
            .map(|gap| GapJson {
                at_frame: gap.at_frame,
                length: gap.length,
                seconds: gap.length as f64 / rate,
                reason: String::from(gap.reason.clone()),
            })
            .collect(),
        ended_reason: track.ended_reason.as_ref().map(ended_reason_id),
        clipped_samples: track.clipped_samples,
        resampled_from: track.resampled_from,
    }
}

/// `show --json`: the summary's fields plus the absolute folder and the tracks.
pub(crate) fn show_json(
    summary: &RecordingSummary,
    folder: &Path,
    manifest: &Manifest,
) -> serde_json::Value {
    let mut value = serde_json::to_value(summary).unwrap_or(serde_json::Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.insert("folder".into(), folder.to_string_lossy().into());
        object.insert(
            "tracks".into(),
            serde_json::to_value(manifest.tracks.iter().map(track_json).collect::<Vec<_>>())
                .unwrap_or(serde_json::Value::Null),
        );
    }
    value
}

pub(crate) fn show_lines(
    summary: &RecordingSummary,
    folder: &Path,
    manifest: &Manifest,
) -> Vec<String> {
    let mut lines = vec![
        format!("Title:    {}", summary.title),
        format!("ID:       {}", summary.recording_id),
        format!("Folder:   {}", folder.display()),
        format!("Started:  {}", short_when(&summary.started_at)),
        format!("State:    {}", state_label(summary)),
        format!("Quality:  {}", quality_id(summary.quality)),
        format!("Length:   {}", format_hms(summary.duration_seconds)),
        format!("Size:     {}", format_bytes(summary.bytes)),
        format!(
            "Gaps:     {} ({:.1} s of silence)",
            summary.gap_count, summary.gap_seconds
        ),
    ];
    if let Some(reason) = &summary.ended_reason {
        lines.push(format!("Ended:    {reason}"));
    }
    lines.push(String::new());
    lines.push("Tracks:".to_string());
    for track in &manifest.tracks {
        let json = track_json(track);
        let mut line = format!(
            "  {} ({}): {} {} Hz, {}-bit, {} channel{}, {} frames ({}), {} gap{}",
            track.id,
            track.role,
            track.codec,
            track.sample_rate,
            track.bits,
            track.channels,
            if track.channels == 1 { "" } else { "s" },
            track.frames,
            format_hms(json.duration_seconds),
            json.gap_count,
            if json.gap_count == 1 { "" } else { "s" },
        );
        if let Some(from) = track.resampled_from {
            line.push_str(&format!(", resampled from {from} Hz"));
        }
        if let Some(device) = &track.device {
            line.push_str(&format!(", device: {device}"));
        }
        lines.push(line);
    }
    lines
}

fn show(root: &Path, name: &str, json: bool, out: &Out) -> Result<(), DictationError> {
    let (dir, manifest) = read_recording(root, name)?;
    let summary = summarize_folder(&dir)?;
    let folder = absolute(&dir);
    if json {
        out.json(&show_json(&summary, &folder, &manifest))?;
    } else {
        for line in show_lines(&summary, &folder, &manifest) {
            outln!(out, "{line}")?;
        }
    }
    Ok(())
}

// -- export ----------------------------------------------------------------

#[derive(Debug, Serialize)]
pub(crate) struct ExportJson {
    recording_id: String,
    files: Vec<PathBuf>,
    derived: bool,
    resampled_to: Option<u32>,
    note: Option<String>,
}

fn export(
    root: &Path,
    name: &str,
    format: FormatArg,
    layout: LayoutArg,
    output: &Path,
    json: bool,
    out: &Out,
) -> Result<(), DictationError> {
    let (dir, manifest) = read_recording(root, name)?;
    let report = export_recording(
        &dir,
        &ExportOptions {
            format: match format {
                FormatArg::Flac => ExportFormat::Flac,
                FormatArg::Wav => ExportFormat::Wav,
            },
            layout: match layout {
                LayoutArg::Tracks => ExportLayout::Tracks,
                LayoutArg::Stereo => ExportLayout::Stereo,
                LayoutArg::Mixdown => ExportLayout::Mixdown,
            },
            out: output.to_path_buf(),
        },
    )?;
    let files: Vec<PathBuf> = report.files.iter().map(|file| absolute(file)).collect();
    errln!(
        out,
        "Exported {} file{}.",
        files.len(),
        if files.len() == 1 { "" } else { "s" }
    );
    if let Some(note) = report.note.as_deref().filter(|_| report.derived) {
        errln!(out, "Note: {note}");
    }
    if json {
        out.json(&ExportJson {
            recording_id: manifest.recording_id,
            files,
            derived: report.derived,
            resampled_to: report.resampled_to,
            note: report.note,
        })?;
    } else {
        for file in &files {
            outln!(out, "{}", file.display())?;
        }
    }
    Ok(())
}

// -- recover ---------------------------------------------------------------

/// One folder's result, in the vocabulary of the app's recovery notes.
#[derive(Debug, Serialize, PartialEq)]
pub(crate) struct RecoveryNote {
    folder_name: String,
    title: String,
    /// `recovered`, `already_complete`, `skipped_locked` or `failed`.
    outcome: &'static str,
    frames_kept: u64,
    tail_bytes: u64,
    duration_seconds: f64,
    message: String,
}

impl RecoveryNote {
    fn bare(folder_name: &str, title: &str, outcome: &'static str, message: String) -> Self {
        Self {
            folder_name: folder_name.to_string(),
            title: title.to_string(),
            outcome,
            frames_kept: 0,
            tail_bytes: 0,
            duration_seconds: 0.0,
            message,
        }
    }
}

pub(crate) fn note_for(folder_name: &str, title: &str, outcome: &RecoveryOutcome) -> RecoveryNote {
    match outcome {
        RecoveryOutcome::Recovered(recovered) => {
            let frames_kept = recovered.tracks.iter().map(|track| track.frames_kept).sum();
            let tail_bytes: u64 = recovered.tracks.iter().map(|track| track.tail_bytes).sum();
            let duration_seconds = recovered
                .manifest
                .tracks
                .iter()
                .zip(&recovered.tracks)
                .map(|(track, kept)| kept.frames_kept as f64 / f64::from(track.sample_rate.max(1)))
                .fold(0.0, f64::max);
            let mut message = format!("kept {} of audio", format_hms(duration_seconds));
            if tail_bytes > 0 {
                message.push_str(&format!(
                    "; {tail_bytes} bytes after the last complete frame were set aside in a .tail file, not deleted"
                ));
            }
            RecoveryNote {
                folder_name: folder_name.to_string(),
                title: title.to_string(),
                outcome: "recovered",
                frames_kept,
                tail_bytes,
                duration_seconds,
                message,
            }
        }
        RecoveryOutcome::SkippedNotRecording => RecoveryNote::bare(
            folder_name,
            title,
            "already_complete",
            "the recording is already complete".into(),
        ),
        RecoveryOutcome::SkippedLocked => RecoveryNote::bare(
            folder_name,
            title,
            "skipped_locked",
            "another process is still writing it; left alone".into(),
        ),
        RecoveryOutcome::Failed { code } => RecoveryNote::bare(
            folder_name,
            title,
            "failed",
            format!("could not be recovered ({code}); nothing was changed"),
        ),
    }
}

fn outcome_of(
    result: Result<sagascript_core::recording::RecoveredRecording, RecordingError>,
) -> RecoveryOutcome {
    match result {
        Ok(recovered) => RecoveryOutcome::Recovered(Box::new(recovered)),
        Err(RecordingError::Locked) => RecoveryOutcome::SkippedLocked,
        Err(RecordingError::NotRecording) => RecoveryOutcome::SkippedNotRecording,
        Err(error) => RecoveryOutcome::Failed { code: error.code() },
    }
}

fn recover(root: &Path, name: Option<&str>, json: bool, out: &Out) -> Result<(), DictationError> {
    let title_of = |folder: &str| {
        read_recording(root, folder)
            .map(|(_, manifest)| manifest.title)
            .unwrap_or_default()
    };
    let notes: Vec<RecoveryNote> = match name {
        Some(name) => {
            let (dir, manifest) = read_recording(root, name)?;
            let folder_name = dir
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            vec![note_for(
                &folder_name,
                &manifest.title,
                &outcome_of(recover_folder(&dir)),
            )]
        }
        None => RecordingStore::new(root)
            .recover_pending()?
            .into_iter()
            // A finished recording needs nothing; only what was looked at counts.
            .filter(|report| !matches!(report.outcome, RecoveryOutcome::SkippedNotRecording))
            .map(|report| {
                let title = match &report.outcome {
                    RecoveryOutcome::Recovered(recovered) => recovered.manifest.title.clone(),
                    _ => title_of(&report.folder),
                };
                note_for(&report.folder, &title, &report.outcome)
            })
            .collect(),
    };

    if json {
        out.json(&notes)?;
    } else {
        for note in &notes {
            outln!(
                out,
                "{}: {} ({})",
                note.folder_name,
                note.outcome,
                note.message
            )?;
        }
    }
    if notes.is_empty() {
        errln!(out, "Nothing to recover.");
    }
    let failed = notes.iter().filter(|note| note.outcome == "failed").count();
    if failed > 0 {
        return Err(DictationError::TaskFailed(format!(
            "{failed} recording{} could not be recovered; see the report above",
            if failed == 1 { "" } else { "s" }
        )));
    }
    Ok(())
}

// -- delete ----------------------------------------------------------------

fn delete(root: &Path, name: &str, yes: bool, json: bool, out: &Out) -> Result<(), DictationError> {
    if !yes {
        return Err(DictationError::SettingsError(format!(
            "refusing to delete '{name}' without --yes: the folder and its audio are removed for good; run the command again with --yes to confirm"
        )));
    }
    // The folder name and id for the report; a recording whose manifest has
    // gone bad can still be deleted by its folder name.
    let known = read_recording(root, name).ok();
    delete_recording_folder(root, name)?;
    let folder_name = known
        .as_ref()
        .and_then(|(dir, _)| dir.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string());
    errln!(out, "Deleted {folder_name}.");
    if json {
        out.json(&serde_json::json!({
            "deleted": true,
            "folder_name": folder_name,
            "recording_id": known.map(|(_, manifest)| manifest.recording_id),
        }))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Cli;
    use clap::Parser;
    use sagascript_core::recording::Quality;

    fn summary(
        title: &str,
        state: RecordingState,
        recovered: bool,
        needs: bool,
    ) -> RecordingSummary {
        RecordingSummary {
            folder_name: format!("2026-11-05 14.02 {title} [a1b2c3]"),
            recording_id: "a1b2c3d4-0000-4000-8000-000000000000".into(),
            title: title.into(),
            state,
            recovered,
            started_at: "2026-11-05T14:02:11.482+01:00".into(),
            quality: Quality::Compact,
            duration_seconds: 3723.0,
            bytes: 1_500_000,
            track_count: 1,
            gap_count: 0,
            gap_seconds: 0.0,
            ended_reason: Some("stopped".into()),
            codec: "flac".into(),
            sample_rate: 16_000,
            channels: 1,
            needs_recovery: needs,
            playable: state == RecordingState::Complete,
        }
    }

    fn parse(args: &[&str]) -> RecordingsAction {
        let mut argv = vec!["sagascript", "recordings"];
        argv.extend_from_slice(args);
        match Cli::try_parse_from(argv).unwrap().command.unwrap() {
            crate::Command::Recordings(args) => args.action,
            _ => panic!("expected recordings"),
        }
    }

    #[test]
    fn every_action_parses_with_dir_and_json() {
        assert!(matches!(
            parse(&["list", "--dir", "d", "--json"]),
            RecordingsAction::List { common } if common.json && common.dir == Some("d".into())
        ));
        assert!(matches!(
            parse(&["show", "a1b2c3", "--json"]),
            RecordingsAction::Show { target, common } if target == "a1b2c3" && common.json
        ));
        assert!(matches!(
            parse(&["recover"]),
            RecordingsAction::Recover { target: None, .. }
        ));
        assert!(matches!(
            parse(&["delete", "x", "--yes"]),
            RecordingsAction::Delete { yes: true, .. }
        ));
        match parse(&[
            "export", "x", "--format", "wav", "--layout", "mixdown", "-o", "o.wav",
        ]) {
            RecordingsAction::Export {
                format,
                layout,
                output,
                ..
            } => {
                assert_eq!(format, FormatArg::Wav);
                assert_eq!(layout, LayoutArg::Mixdown);
                assert_eq!(output, PathBuf::from("o.wav"));
            }
            _ => panic!("expected export"),
        }
        assert!(parse(&["list", "--json"]).json());
        assert!(!parse(&["list"]).json());
    }

    #[test]
    fn export_needs_an_output_and_known_choices() {
        for argv in [
            &["recordings", "export", "x"][..],
            &["recordings", "export", "x", "-o", "o", "--format", "mp3"],
            &[
                "recordings",
                "export",
                "x",
                "-o",
                "o",
                "--layout",
                "surround",
            ],
            &["recordings", "delete"],
        ] {
            let mut full = vec!["sagascript"];
            full.extend_from_slice(argv);
            assert!(Cli::try_parse_from(full).is_err(), "{argv:?}");
        }
    }

    #[test]
    fn a_path_target_names_the_folder_and_its_parent_is_the_root() {
        let stored = Settings::default();
        let common = Common {
            dir: Some("ignored".into()),
            json: false,
        };
        assert_eq!(
            locate(&common, "/rec/2026 Talk [a1b2c3]/", &stored).unwrap(),
            (PathBuf::from("/rec"), "2026 Talk [a1b2c3]".to_string())
        );
        assert_eq!(
            locate(&common, "./Talk", &stored).unwrap(),
            (PathBuf::from("."), "Talk".to_string())
        );
        assert_eq!(
            locate(&common, "a1b2c3", &stored).unwrap(),
            (PathBuf::from("ignored"), "a1b2c3".to_string())
        );
        let custom = Settings {
            recording_directory: "/from/settings".into(),
            ..Settings::default()
        };
        assert_eq!(
            locate(&Common::default(), "x", &custom).unwrap().0,
            PathBuf::from("/from/settings")
        );
        assert!(locate(&common, "/", &stored).is_err());
    }

    #[test]
    fn state_labels_cover_every_case() {
        use RecordingState::{Complete, Recording};
        assert_eq!(
            state_label(&summary("a", Complete, false, false)),
            "complete"
        );
        assert_eq!(
            state_label(&summary("a", Complete, true, false)),
            "recovered"
        );
        assert_eq!(
            state_label(&summary("a", Recording, false, true)),
            "needs recovery"
        );
        assert_eq!(
            state_label(&summary("a", Recording, false, false)),
            "in progress"
        );
    }

    #[test]
    fn the_table_aligns_columns_and_leaves_the_last_unpadded() {
        let lines = table_lines(&[
            summary("Standup", RecordingState::Complete, false, false),
            summary(
                "Ärende längre titel",
                RecordingState::Recording,
                false,
                true,
            ),
        ]);
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[0],
            "ID        WHEN              TITLE                LENGTH   QUALITY  SIZE    STATE"
        );
        assert_eq!(
            lines[1],
            "a1b2c3d4  2026-11-05 14:02  Standup              1:02:03  compact  1.5 MB  complete"
        );
        assert!(lines[2].ends_with("needs recovery"));
        assert!(lines.iter().all(|line| !line.ends_with(' ')));
    }

    #[test]
    fn long_titles_are_cut_with_an_ellipsis_by_display_width() {
        let long = "x".repeat(60);
        let cut = truncate(&long, TITLE_WIDTH);
        assert_eq!(UnicodeWidthStr::width(cut.as_str()), TITLE_WIDTH);
        assert!(cut.ends_with('…'));
        assert_eq!(truncate("short", TITLE_WIDTH), "short");
        let wide = "漢".repeat(30);
        assert!(UnicodeWidthStr::width(truncate(&wide, TITLE_WIDTH).as_str()) <= TITLE_WIDTH);
    }

    fn manifest_with_device() -> Manifest {
        use sagascript_core::recording::{Client, Clock, EndedReason, Gap, GapReason};
        Manifest {
            schema_version: 1,
            min_reader_version: 1,
            recording_id: "a1b2c3d4-0000-4000-8000-000000000000".into(),
            title: "Talk".into(),
            state: RecordingState::Complete,
            recovered: false,
            started_at: "2026-11-05T14:02:11.482+01:00".into(),
            client: Client {
                kind: "cli".into(),
                app_version: "1".into(),
                build: "x".into(),
                extra: Default::default(),
            },
            quality: Quality::Lossless,
            tracks: vec![Track {
                id: "mic".into(),
                role: "mic".into(),
                file: "mic.flac".into(),
                codec: "flac".into(),
                lossless: true,
                sample_rate: 48_000,
                bits: 24,
                channels: 1,
                frames: 96_000,
                audio_sha256: None,
                clock: Clock::new("mach_continuous", 1),
                gaps: vec![Gap::new(48_000, 4_800, GapReason::Overrun)],
                ended_reason: Some(EndedReason::Stopped),
                clipped_samples: 0,
                resampled_from: None,
                device: Some("Studio Mic".into()),
                extra: Default::default(),
            }],
            extra: Default::default(),
        }
    }

    #[test]
    fn show_json_omits_the_device_and_adds_folder_and_tracks() {
        let manifest = manifest_with_device();
        let value = show_json(
            &summary("Talk", RecordingState::Complete, false, false),
            Path::new("/rec/Talk"),
            &manifest,
        );
        assert_eq!(value["folder"], "/rec/Talk");
        assert_eq!(value["title"], "Talk");
        let track = &value["tracks"][0];
        assert_eq!(track["codec"], "flac");
        assert_eq!(track["sample_rate"], 48_000);
        assert_eq!(track["frames"], 96_000);
        assert_eq!(track["gap_count"], 1);
        assert_eq!(track["gaps"][0]["reason"], "overrun");
        assert!((track["gaps"][0]["seconds"].as_f64().unwrap() - 0.1).abs() < 1e-9);
        assert_eq!(track["ended_reason"], "stopped");
        assert!(
            !value.to_string().contains("Studio Mic"),
            "the device name must not be in the JSON"
        );
    }

    #[test]
    fn show_text_lists_tracks_with_their_device() {
        let lines = show_lines(
            &summary("Talk", RecordingState::Complete, false, false),
            Path::new("/rec/Talk"),
            &manifest_with_device(),
        );
        let text = lines.join("\n");
        assert!(text.contains("Folder:   /rec/Talk"));
        assert!(text.contains(
            "mic (mic): flac 48000 Hz, 24-bit, 1 channel, 96000 frames (0:00:02), 1 gap"
        ));
        assert!(text.contains("device: Studio Mic"));
    }

    #[test]
    fn recovery_notes_use_the_apps_vocabulary() {
        let locked = note_for("f", "T", &RecoveryOutcome::SkippedLocked);
        assert_eq!(locked.outcome, "skipped_locked");
        assert!(locked.message.contains("left alone"));
        let done = note_for("f", "T", &RecoveryOutcome::SkippedNotRecording);
        assert_eq!(done.outcome, "already_complete");
        let failed = note_for(
            "f",
            "T",
            &RecoveryOutcome::Failed {
                code: "recording_corrupt_track",
            },
        );
        assert_eq!(failed.outcome, "failed");
        assert!(failed.message.contains("recording_corrupt_track"));
        let json = serde_json::to_value(&failed).unwrap();
        assert_eq!(json["folder_name"], "f");
        assert_eq!(json["frames_kept"], 0);
    }

    #[test]
    fn delete_without_yes_is_refused_before_anything_is_looked_up() {
        let (out, captured) = Out::capture();
        let error = delete(Path::new("/does/not/exist"), "x", false, false, &out).unwrap_err();
        assert_eq!(error.code(), "settings_error");
        assert!(error.to_string().contains("--yes"));
        assert!(captured.stdout().is_empty());
    }
}
