# #519 verification and review

The endpoint remains merge after review, with no release or installation. Essential native acceptance is pending; this is not a ready-to-merge claim.

## Candidate review

The initial local candidate was `e4f3856e68015d8fc31fb9a649984abd7c5a95a6`, based on `a996b291cdeda4588e210b16c3a6221de01202d4`. Independent read-only review used a native subagent configured for `gpt-5.6-sol` / `xhigh`. Runtime model/effort metadata was unavailable. A suitably restricted different-provider CLI route could not be established; the skill-authorized native fallback was used.

Three findings were accepted:

- P1: a handwritten Core Audio timestamp had the wrong SMPTETime size and flags offset. Use SDK-generated `coreaudio-sys` bindings. A byte-layout callback regression failed before the fix and passed afterwards; other callback layouts are checked against SDK bindings too.
- P1: the shipped native build setup omitted the selected Clang compiler runtime required by release linking on Xcode 27. The local manual linker workaround was insufficient delivery evidence. The helper now resolves and validates the selected compiler archive, then uses a Rust compiler shim to preserve Cargo flags and existing wrappers. Its 11 tests pass, including a real link against a local archive whose path contains spaces and shell metacharacters. The release guard passed all four tests using the generated helper (no manual RUSTFLAGS workaround), including `release_build_ignores_the_real_environment`; macOS 13.0 remained the deployment target.
- P2: a system tap that stops callbacks during silence left a stale signal level visible. Clear the idle signal without inventing fresh audio or changing connection state. The regression failed before the fix and passed afterwards.

Review usefulness: pass (three grounded defects). The bounded build-helper leaf used a native Luna/high configuration (runtime identity unavailable); usefulness partial because root review required corrections to argument handling and wrapper composition. Root inspected the final patch, reran its real-link regression, and limited POSIX execution fixtures to compatible hosts while keeping the non-macOS rejection test platform-independent. No review substitutes for native audio acceptance.

## Deterministic evidence

After the timestamp and idle-signal fixes: all 71 live-core tests passed; whole-workspace tests reported 2,490 passing and 3 ignored, with zero failures; workspace Clippy, both Rust format checks and lean CLI dependency checks passed. The native record feature also compiled on Rust 1.88.

After the build-helper correction, format, lint, Node CI and code-health checks also passed. The final platform-fixture adjustment passed its 11 focused tests plus ESLint/Prettier.

The initial candidate additionally passed 673 frontend tests, isolated browser flows (including the new live view with inspected screenshots), Svelte checks, formatting/lint, Node CI/code-health checks, dependency/license policy, Swift tests and engine-host conformance. The local default verification run's initial failures were diagnosed and rechecked individually; do not describe that first run as entirely green. A verbatim upstream Abseil license retains its original trailing blank line and hash.

An ad-hoc signed QA app from the initial candidate passed package identity, entitlements, usage-string and system-only dynamic-library checks, plus CoreML/ANE conformance with synthetic silence. It was not launched for audio capture and must be rebuilt from the corrected revision before native testing. Ad-hoc signing does not establish Developer ID distribution behavior.

## Outstanding gates

See [NATIVE-ACCEPTANCE.md](NATIVE-ACCEPTANCE.md): exact candidate test authorization, actual permissions and routes, headphones and speakers, echo/doubletalk, export/stop, at least 30 minutes of measured Swedish latency/quality/resource behavior, and Magnus's real-meeting acceptance. Provider checks must also pass on the published candidate before merge.

## First provider CI and corrections

Draft PR #521 published candidate `27ef58be00399a4360f80136baa5c7eb0423e5ea`. Rereview found all three original defects fixed and no new findings (same configured reviewer, runtime identity unavailable). Both provider MSRV jobs passed. Provider CI exposed two additional build-environment gaps: Windows Git checkout converted the hash-verified legal files to CRLF, and Clippy requires the compiler shim's basename to be `rustc`.

Both failures were reproduced in local regressions before fixes. A `-text` attribute preserves all nine original legal files; a Windows-style checkout test verifies their hashes. The compiler shim now lives at `compiler/rustc`, outside PATH, and the native smoke includes actual Clippy through the existing wrapper chain. The 27 focused helper/license tests, formatting/lint/Node CI, whole-workspace Clippy through the newly generated helper, and release guard pass. The source runtime and UI are unchanged by these CI corrections. Updated provider checks and rereview are still required.

Delegation evidence: a native Luna/high leaf reproduced 0/9 then 9/9 matching hashes in isolated Git fixtures (runtime model/effort unknown, usefulness pass); root implemented and verified the fixes.

## Linux policy correction

Candidate `e486afb85546ab44042d57b6aa67e4045359053b` passed both required provider checks (`check-macos`, `check-windows`), both release builds and both MSRV jobs. Linux's ordinary Cargo gates passed, but its separate production Clippy policy rejected ten new findings: one `unwrap_used` in segment history and nine `expect_used` in timeline initialization/draining. The normal macOS verification matrix does not measure this Linux baseline. A direct production-core policy pass on macOS reproduced all ten findings.

The correction handles optional timestamp state explicitly and drains each front audio chunk through one checked borrow and bounded iterators. Valid sample placement, gaps and retention accounting must remain unchanged. Neither lint suppression nor a baseline increase is permitted for this repair. The runtime change requires renewed candidate verification and a rebuilt QA artifact before native acceptance; the earlier `27ef58be` QA app is not the current candidate.

After the correction, the same direct production-core pass reports zero policy findings in live code (ten before). All 71 focused live tests, workspace Clippy and Rust formatting pass. The complete workspace tests pass again: 2,490 passed, three ignored, no failures. The first local attempt failed because the sandbox denied loopback HTTP fixtures; the unchanged suite passed with that fixture capability approved. This macOS evidence does not replace the pending Linux baseline run.

The bounded implementation leaf used native `gpt-5.6-luna`/high (runtime identity unavailable). Usefulness: partial; root requested a simpler borrow structure, inspected the resulting patch and independently ran policy, Clippy and workspace tests. No lint baseline or acceptance gate changed.

## Native QA corrections

Published candidate `498c9706a6c06e3f47df9edab74f98ace7056313` passed all 11 jobs in provider CI run `40245741205` and both Windows package architectures in run `40245741220`. These receipts apply to that revision only.

The owner's approved native audio test established short-session two-source transcription, background progress, stop and text/stereo-audio export. It also reproduced startup buffer overflow and a Cmd+Q exit-guard bypass. See [NATIVE-ACCEPTANCE.md](NATIVE-ACCEPTANCE.md) for bounded evidence and remaining gates. No captured personal speech or transcript is published.

The capture correction prepares AEC before opening native streams and uses a shared activation epoch to ignore pre-start callbacks, including trimming buffers that straddle activation. Three regressions fail without the gate/trim and pass with it; real post-start overflow remains an error. A bounded native `gpt-5.6-luna`/high worker implemented this leaf (actual runtime identity unknown); root required a stronger valid-buffer regression, inspected the patch and integrated it. Usefulness: partial. A second bounded read-only Luna/high leaf traced Tauri/Muda's native Quit selector and checked the replacement menu path (actual runtime identity unknown; usefulness pass). Root implemented the native menu correction. Native green evidence remains required; code inspection alone is insufficient.

Root verification of both corrections: workspace tests 2,493 passed, three ignored, zero failures; workspace Clippy passed; direct production-core policy reported zero findings in live code. Rust formatting and diff whitespace checks passed. These checks do not replace the pending native regression run or fresh provider CI.
