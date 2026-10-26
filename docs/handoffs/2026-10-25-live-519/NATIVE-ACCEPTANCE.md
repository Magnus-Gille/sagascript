# #519 native acceptance — partial, corrections pending

The delivery endpoint is merge after review. This checklist does not authorize microphone capture, system-audio capture, playback, installation, or release. Ask Magnus before starting the concrete audio test. Use a candidate built from a recorded commit; never substitute browser mocks for native evidence.

## Candidate receipt

Record commit, dirty status, app path, bundle identifier, macOS version, Mac/chip, microphone/output routes, model identity and compute-unit configuration. Verify the bundled engine host identity, code signature and entitlements, audio usage descriptions, and absence of developer/Homebrew dynamic library dependencies. Keep the installed SagaScript unchanged. An ad-hoc signature is not evidence of Developer ID distribution behavior.

## Authorized test sequence

1. Start with a short, owner-approved test phrase and playback fixture. Check both source meters and source-labelled text. Cancel during model warm-up, then start again.
2. Exercise microphone and system-audio permission denial/recovery through normal macOS controls. Record actual prompts and errors. Do not reset unrelated TCC permissions.
3. Test headphones: local speech, remote speech, simultaneous speech, silence and short remote bursts. Confirm source attribution, timing and no invented speech during silence.
4. Repeat with computer speakers at normal meeting volume. Inspect remote-only leakage into the microphone transcript and preservation of local speech during doubletalk. This is a release-blocking quality question for AEC3, not proven by DSP unit tests.
5. Switch tabs and return; read older pages while new text arrives. Disconnect/change the selected route. Verify visible failure/retained partial results and that stopped sessions no longer capture.
6. Stop while speech is queued. Check finalization, incomplete/gap markers, export cancellation and retry, text export, stereo WAV (microphone left/system right), explicit discard, and the quit guard. Use only task-specific temporary output paths; do not overwrite existing files.
7. Run at least 30 minutes of representative Swedish conversation/lecture audio on the same machine. Record time to first preview and stable final text (p50/p95/worst), rewrite rate, intelligibility/accuracy against approved reference text, queue growth, CPU/memory and perceived meeting impact. No target is accepted solely from a short fixture or a mock.
8. Magnus uses the candidate in an approved real meeting and confirms that visible progress, quality and source separation are trustworthy. Record unresolved issues; essential acceptance failures block merge.

## Data and recovery

Capture stays local. Temporary audio exists during the session so that audio export remains possible afterwards. Text/audio export is opt-in; use Discard to release the test session. Do not attach captured speech or transcripts to an issue, PR or remote reviewer. Keep test evidence limited to timings, source states, synthetic/approved phrases and redacted diagnostics unless separately authorized. Stop the candidate on failure; no installation or production rollback is involved.

## Current evidence

On 2026-10-26 Magnus authorized a local, isolated audio test of candidate `498c9706a6c06e3f47df9edab74f98ace7056313`, bundle `ai.gille.sagascript.live519qa`, for at most 45 minutes (06:48:36–07:33:36 UTC). He explicitly allowed both macOS permission prompts. The installed app and its settings were preserved. The ad-hoc signed bundle passed identity, entitlements, usage-string and system-only dynamic-library checks; its bundled engine host matched the clean candidate.

Built-in MacBook Air microphone (48 kHz) and speakers (96 kHz) were selected. A short Swedish synthetic playback and local speech produced separate system/microphone transcript labels. Switching away from the Live tab and back preserved progress. The short session stopped normally with zero reported lost samples; text export and stereo PCM16 WAV export succeeded (16 kHz, 86.27 seconds). Cancelling text export preserved results. Displayed text delay was approximately 0.4–0.7 seconds, but this is not a measured end-to-end latency percentile. Raw recordings and personal transcript content remain local and are excluded from publication and review.

Two native failures block acceptance:

- The attempted 30-minute CLI run ended after about five seconds with microphone buffer overflow (98,816 dropped samples). It did not complete the endurance test. CPAL starts the macOS input stream during construction, before the system tap and consumer are ready. The correction gates both callbacks until a shared activation epoch and prepares AEC before capture; regression tests cover preactivation callbacks and timestamp trimming.
- Cmd+Q exited the GUI with a retained completed session, bypassing the existing `ExitRequested` guard. The correction replaces the native predefined Quit item with a regular Cmd+Q menu action routed through Tauri's vetoable exit request. Native retesting must prove refusal before Discard and normal exit afterwards.

The candidate was stopped after these findings. No capture continues. A rebuilt, reviewed candidate and a renewed concrete audio-test authorization are required for further capture. Still pending: native regression retests, headphones, measured speaker echo/doubletalk quality, route changes, 30-minute latency/quality/resource measurements, and Magnus's real-meeting acceptance. Deterministic checks and browser screenshots do not establish these items.
