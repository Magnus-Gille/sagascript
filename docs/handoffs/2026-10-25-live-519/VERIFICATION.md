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
