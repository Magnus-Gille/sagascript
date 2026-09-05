# Windows dictation benchmark

The CLI benchmark measures the local Whisper inference path used by live
dictation. Build the CLI and run it against a supplied fixture:

```powershell
cargo build -p sagascript-cli --release
.\target\release\sagascript.exe benchmark-dictation test-audio\english-jfk.wav --language en
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
same decoded PCM buffer.

This is an inference benchmark. It excludes microphone capture, hotkey
handling, UI state changes, and paste. The GUI's phase log records those phases
for an end-to-end measurement: compare `capture`, `model_acquisition`,
`inference`, and `clipboard_focus_paste` timestamps when assessing the complete
dictation path.

The checked-in Windows gate currently uses the public English fixture to catch
model reuse and transcription regressions. A Swedish fixture and any
cross-language latency claim remain deferred until representative audio is
available.
