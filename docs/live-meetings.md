# Live meetings (#519)

## Implementation choice

Keep SagaScript's existing Pianissimo engine host for Swedish ASR. On Apple Silicon its Core ML compute setting defaults to CPU and Neural Engine. This retains the existing local model, model verification, process supervision and ANE execution route. It does not imply that every operation runs on ANE. No cloud backend or new model download is introduced.

The two labels describe **audio sources**, not identified speakers: `Jag · mikrofon` and `Mötet · systemljud`. Several remote participants share the system label. Source overlap is retained. Saved audio contains raw microphone on the left and system audio on the right; the microphone transcription uses echo-cancelled audio.

Magnus confirmed on 2026-10-25 that v1 uses the Mac-selected default microphone and all system audio, with signal status after Start. Per-app/device selectors and pre-start signal testing are deferred.

Use a private Core Audio process tap (macOS 14.2+) for system audio and CPAL for the default microphone. The aggregate contains only the tap, so a headset's physical microphone cannot accidentally become part of the system track. Both callbacks copy finite PCM into bounded channels. The capture owner checks device changes, aligns native timestamps, converts to 16 kHz and writes temporary audio off the callbacks.

Use `webrtc-audio-processing` **2.1.0** (BSD-3-Clause), with its bundled WebRTC AEC3, for microphone echo cancellation. It receives the aligned system track as its far-end reference in 10 ms frames. This is CPU audio processing; it leaves Pianissimo's Core ML/ANE inference route intact. Amplitude/correlation suppression alone is unsuitable for simultaneous local and remote speech.

### References checked

- [Apple: Core Audio taps](https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps): process capture, macOS 14.2 and `NSAudioCaptureUsageDescription`.
- [Chromium tap adapter](https://chromium.googlesource.com/chromium/src/media/+/e2e9a25be98ac39e38a79287ea705f2be71f7226/audio/mac/catap_audio_input_stream.mm): tap-only aggregate and a permission probe after IOProc registration. Useful implementation evidence, not proof of our hardware support.
- [WebRTC Rust wrapper](https://github.com/tonarino/webrtc-audio-processing): reused AEC implementation, exact Cargo lock and bundled-source build.
- [Wisp](https://github.com/ppXD/Wisp), inspected at `311f778ff15610f12351941129504c77a778ef08`: MIT reference for near/far capture and AEC; no claim that its complete integration is verified.
- [Muesli](https://github.com/Muesli-HQ/muesli): MIT reference for Core Audio capture; its claimed device coverage is not our acceptance evidence.
- Existing draft PRs #300/#306 informed the source split. Their native timing, queue and echo limitations prevent treating them as a ready implementation.

## Runtime and UI

`meeting/live/session.rs` owns one session for both desktop and CLI. A dedicated capture thread owns native start/stop. A serialized ASR worker uses bounded windows and one persistent Pianissimo client. Preview jobs for the same utterance coalesce; final jobs are retained or produce an explicit overload failure. An error stops capture and retains exportable partial results where possible.

Current tuning is a candidate for measurement: 250 ms capture holdback; 200 ms pre-roll; preview roughly every second; utterance finalization after 600 ms of silence; a 30-second window cap with overlap. Signal detection currently uses the existing RMS threshold rather than a separate speech classifier. These values are not an accepted Swedish quality/latency target.

The live document has its own versioned schema. It retains source, timestamps, revisions, preview/final state and explicit gaps. Only final segments are exported; omission of pending previews makes the exported result incomplete. Timestamp-based word trimming handles overlap when ASR supplies words. Model output without word timing still needs quality evaluation around forced cuts.

The larger Settings live tab polls serialized snapshots every 500 ms, shows source health, signal, queue and measured last text delay, and pages older transcript rows. Tab switching leaves the backend session running. Stop drains queued final speech. Starting can be cancelled. Export and discard remain available after a terminal failure. Quitting with a pending session reveals the live tab instead of silently deleting it.

The first implementation is bounded to four hours, 20,000 segments, 16 MiB transcript text and a 60-second ASR pending PCM queue. Native timestamp discontinuities, microphone stalls, capture overflow, route changes and excessive clock drift fail visibly instead of fabricating continuity. A system source with no samples is not proof of silence or successful permission; native permission and real-device tests remain essential.

## Temporary data and export

Audio is held in an unnamed temporary PCM WAV file, not a permanent meeting archive. It is deleted when the session is discarded or the process exits. Text lives in the session. Crash recovery of unsaved meetings is not promised. Choose text and/or audio export after stopping, then discard the temporary session. Exports use staged, no-replace publication; failed saves preserve the temporary recording for another destination.

The desktop offers Markdown text and two-track WAV. The core also supports plain text, JSON, SRT and VTT. CLI `sagascript live` streams human-readable previews/finals; `--jsonl` streams status, changed segments and gaps. Interactive mode asks for export paths and explicit discard. In noninteractive mode, only `--text-output` and `--audio-output` request saving; an export failure is reported before the process exits and loses its temporary copy.

## Build and verification

WebRTC's bundled build needs Meson 1.12.1, Ninja 1.13.2, clang, pkg-config and the selected Rust toolchain's llvm-tools component. `scripts/live-aec-build-requirements.txt` pins binary-wheel hashes. `scripts/prepare-live-aec.mjs` verifies already-downloaded Abseil 20240722.0 source/patch hashes, creates task-local wrappers, and prevents ambient Homebrew Abseil selection. It performs no downloads or package installation. The macOS CI setup prepares those inputs before Cargo.

Do not distribute a binary linked to `/opt/homebrew` or another developer's native libraries. Check `otool -L` on the built application and engine host. Build the native component with macOS deployment target 13.0 to preserve the app's existing lower OS floor; live capture itself remains gated at 14.2 and Apple Silicon. The new path must also compile on the repository's Rust 1.88 MSRV.

### Acceptance still required before merge

Deterministic component tests are not native meeting acceptance. Required evidence includes a signed application, microphone and system permission denial/recovery, headphones and speakers, simultaneous local/remote speech, silence, device changes, export retry, stop/drain and at least 30 minutes without growing backlog or material meeting impact. Measure p50/p95/worst time to first and stable text, rewrites and Swedish accuracy on the same representative audio and machine. The short FLEURS Swedish fixture is only a smoke test; it is not conversational meeting ground truth.

Owner approval of the exact live capture/playback test is required before recording on the owner's Mac. Magnus's real-meeting trial and judgment of “good enough” remain final acceptance gates. No production release or automatic installation is included in the merge mandate.
