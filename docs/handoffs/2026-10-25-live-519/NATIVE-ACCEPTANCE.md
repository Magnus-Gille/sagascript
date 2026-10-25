# #519 native acceptance — not yet executed

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

Pending: candidate package verification, all native capture/permission/quality/resource tests, and Magnus's real-meeting acceptance. Deterministic checks and isolated browser screenshots are tracked separately and do not satisfy these items.
