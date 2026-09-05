# Windows dictation benchmark

The CLI benchmark measures the local Whisper inference path used by live
dictation. Build the CLI and run it against a supplied fixture:

```powershell
cargo build --manifest-path src-tauri/Cargo.toml -p sagascript-cli --release
.\src-tauri\target\release\sagascript.exe benchmark-dictation test-audio\english-jfk.wav --language en
```

The recommended model for the selected language must already be present. The
command never downloads models and never reads or writes the saved settings.
Use `sagascript download-model base.en`, `kb-whisper-base`, or
`nb-whisper-base` beforehand when the corresponding fixture requires it.

`--iterations` controls the warm sample count and accepts 2 through 30
(default 5). `--max-warm-ms` turns the run into a gate: every warm sample's
inference total must be at or below that limit. `--expect-word WORD` checks
that every cold and warm transcript contains the expected token while the JSON
output still contains no transcript text or input path.

The JSON report includes the build version, selected language and model, audio
duration, one decode duration, cold model and inference timings, warm p50/p95
model/inference/total timings, and nonempty text counts. The model is loaded
by the first cold call; all warm calls use the same in-process backend and the
same decoded PCM buffer. These percentiles use linear interpolation between
sorted samples; the offline `latency-report` command uses nearest rank, so do
not compare the two formats as if their percentile estimators were identical.

This is an inference benchmark. It excludes microphone capture, hotkey
handling, UI state changes, and paste. The GUI writes a
`dictation_session_finished` event with `recording_finalization`, `conversion`,
`model_acquisition`, `inference`, `postprocessing`, and `clipboard_focus_paste`
durations in `phases_ms`. `key_up_to_completion_ms` starts when the shortcut's
release reaches the application and ends after the paste operation returns
(or inference completes when auto-paste is off). It excludes OS/input dispatch
before the application's stop handler; it cannot prove when another
application's editor renders the text. A timeout ends the application's wait,
not necessarily the native paste operation; check the editor before retrying.

The installed 604a9c1 candidate only logs session starts, so historical
push-to-talk latency cannot be reconstructed from that log. Keep the source
revision with every new baseline. Use `scripts/summarize-windows-dictation.ps1`
with `-LogPath`, `-OutputPath`, and optionally `-Since` to export aggregate
durations and hardware metadata without dictated content.

That PowerShell summary is a legacy exploratory diagnostic: its overall and
per-language distributions can combine different builds, models and utterance
lengths. They are not a configuration-specific regression baseline. For new
R1 builds use `sagascript latency-report --input <copied.jsonl>` for separate
configuration/length/outcome cohorts and an explicit successful-paste budget;
see [the metric and privacy boundaries](latency-measurement.md). The published
September 5 beta predates that CLI command, so use the new R1 CLI for reports.

The checked-in Windows gate uses the public English fixture to catch model
reuse and transcription regressions. Historical subjective Swedish feedback
is not a controlled Windows baseline. Roadmap #198 includes #184 in R1;
physical Windows cold/warm measurements and any evidence-backed optimization
remain separate acceptance work. No model, decoder-quality or latency-budget
default is changed here.
