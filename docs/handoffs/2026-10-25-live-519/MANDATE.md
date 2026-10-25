# #519 delivery mandate

Owner authorization: 2026-10-25 conversation, "sen ship-it", clarified endpoint "Merge till main efter granskning"; text AND audio must be optionally saved after the meeting; open-source reuse explicitly allowed; preserve Apple Neural Engine advantage.

Repository: Magnus-Gille/sagascript. Base: a996b291cdeda4588e210b16c3a6221de01202d4. Branch: codex/issue-519-live-meetings. Endpoint: reviewed implementation merged to main with fresh required checks; no release/tag/deploy. Main push prebuild workflow does not publish; no version bump planned. Required provider checks to re-read before publication/merge.

Scope: Mac live microphone plus system-audio transcription, source labels, larger in-app view, background operation, visible source/ASR failures, bounded latency/quality measured, headphones AND speakers, temporary two-source audio retention, explicit post-meeting text/audio save or discard, CLI parity. Pianissimo/CoreML/ANE first candidate. No full remote speaker identification, cloud, summary, Windows native capture expansion, automatic paste, or permanent automatic meeting archive.

Checks: red/green behavior tests, project verify gates, cross-model independent review, native/visual evidence, two-source >=30 min acceptance and speaker-echo/doubletalk quality. Do not claim mocks establish hardware acceptance. Owner audio capture/playback still needs explicit test-time authorization. Do not merge with unproven essential acceptance.

Recovery: local changes on task/leaf worktrees only; preserve primary checkout and its untracked :memory:.ses. Publication reversible through follow-up/revert PR; no branch deletion or destructive reset. No release trigger authorized.

Open implementation choices: upstream AEC/capture components currently being checked for exact license/API/platform support; measured latency/quality balance rather than an invented fixed target. Full audio temporary retention is required by the owner's save-audio answer.

Owner clarification 2026-10-25: Mac default microphone and all system audio are sufficient for v1, signal status after Start. Per-app/device selection and pre-start signal testing are not required for this candidate.
