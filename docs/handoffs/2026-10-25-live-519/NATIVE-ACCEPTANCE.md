# #519 native acceptance — partial, native regressions passed

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

The candidate was stopped after these findings. They led to the rebuilt, reviewed candidate and renewed owner authorization described below. Deterministic checks and browser screenshots alone did not establish native acceptance.

## Corrected candidate, 2026-10-26

Magnus authorized candidate `14b7abd5ae43ed9146ac4c459a9929063f183aaa` for at most 45 minutes from test start. The isolated QA app and CoreML host matched that clean revision and passed package checks. Hardware: MacBook Air M4, 32 GB, macOS 27.0.1 (26A434), built-in microphone and speakers. Both provider CI and Windows package jobs passed on this revision.

The first GUI start reported microphone permission pending without a visible dialog. An explicit retry advanced to Starting; Magnus then accepted an OS prompt and capture became active. No TCC reset or permission bypass was used. The short GUI session passed two-source capture, stop and zero-loss checks. Cmd+Q retained both active and completed sessions; app-menu Quit retained a failed session; after Discard, Cmd+Q exited the QA app. The ordinary installed app remained untouched. These results cover visible in-app Quit actions, not external AppKit termination, Force Quit or crash recovery.

The subsequent CLI run lasted **807.62 seconds (13.46 minutes)**. Magnus asked to stop the annoying repeated playback, so playback and capture were stopped promptly. CLI exit 130 records the requested interrupt; the session reached Completed and exported a valid stereo PCM16/16 kHz WAV and text before cleanup. Both sources reported zero lost frames and zero gaps. This is **not** a completed 30-minute test, and repeated speaker playback must not resume without a new explicit request.

Aggregate measurements from this bounded run:

- First nonempty preview relative to segment start: median 1.86 s, p95 4.60 s, maximum 5.20 s across 25 segments. Before local speech was added, the repeated synthetic system fixture was approximately 1.8–1.9 s. These are segment-relative estimates, not manually aligned phonetic onset measurements.
- Stable final text relative to segment end: median 0.45 s, p95 0.53 s, maximum 0.54 s. The running-status epoch estimator had a 0.32 s anchor spread; avoid treating these as exact speech-end latencies.
- Twenty-one repeats of the 54-word synthetic reference averaged 10.93% normalized word edit distance. This small repeated fixture does not establish general meeting accuracy.
- Magnus spoke the supplied doubletalk phrase during playback. The closest final microphone segment differed by one word from the 18-word reference while a nonempty system segment overlapped. This establishes one successful local/system overlap case, not broad doubletalk quality.
- Twenty-six resource samples: app mean 20.4% of one CPU core, RSS 47.2→42.3 MiB; engine mean 3.3% of one core, RSS 245.3→77.3 MiB. RSS does not include all accelerator/shared memory. Queued-audio maxima (3.89 s microphone, 22.85 s system) describe pending audio-window duration, not the time spent waiting; no increasing backlog or capture loss was observed.

All captured audio, transcripts and raw logs were deleted after aggregate verification, as authorized. Only aggregate metrics, synthetic reference material and package/test receipts remain locally. No personal speech or transcript is published. No capture or playback remains running.

The run exposed empty ASR finals as blank source/time rows. A presentation/export correction filters whitespace-only rows before UI pagination and skips blank cues in human-readable exports. Raw JSON/JSONL segment state remains available so an empty final can replace an earlier preview. Capture, AEC, scheduling and ASR are unchanged by this follow-up; native evidence above remains explicitly tied to `14b7abd5`.

Outstanding acceptance: headphones, route changes, a representative longer meeting/lecture test, the incomplete 30-minute check, and Magnus's real-meeting trust/quality assessment. Do not merge on the strength of the short fixture alone.

## Updated quality workflow and acceptance

Magnus requested automatic local retranscription of both retained tracks after Stop (2026-10-26). Live text may be approximate; final quality must be evaluated separately. The new pass must visibly stop capture before processing, preserve live text/audio on cancellation or error, and export the chosen final result without mixed source attribution. A measurable improvement is not yet established. Capture loss, route failures and source separation remain independent acceptance gates.

Further native tests require a fixed user script displayed separately from system playback references, exact spoken cues/timing, and Magnus's explicit readiness immediately before start. Do not automatically replay a fixture. The completed scripted AirPods test on a0fa8a0e reached Completed with zero reported loss/gaps and valid stereo/text export; B matched15/15words and system2/87word edits. A was invalid for the intended comparison because Magnus read the system reference. These aggregate results neither approve the new post-stop pass nor prove distinct simultaneous speech attribution. Captured raw data was deleted after checking.
