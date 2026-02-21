# Sagascript (macOS dictation) — Claude Code Project Context

## What this repo is
This repo is a **Claude Code autopilot project** to build a low-latency macOS dictation app (Wispr Flow–style) named **Sagascript**.

The master prompt is in `PROMPT.md`. Follow it.

## Golden rules
- Do not ask the user questions; make assumptions and document them in `docs/DECISIONS.md`.
- Optimize for **latency** and **perceived speed**.
- Default to **local transcription** (privacy-first). Remote is opt-in.
- Keep UI minimal (menu bar + settings + indicator).
- **CLI-first design**: Every feature must have a CLI equivalent. The GUI is a convenience layer on top of CLI commands. When adding any new feature or capability, always implement the CLI subcommand first (or alongside), even if the user only asks for a GUI change. The CLI commands in `src-tauri/src/cli/` are the source of truth for what the app can do.

## Primary docs (always keep updated)
- `docs/PRD.md`
- `docs/ARCHITECTURE.md`
- `docs/NFRS.md`
- `docs/SECURITY_PRIVACY.md`
- `docs/TEST_PLAN.md`
- `docs/STATUS.md`
- `docs/DECISIONS.md`

## Recommended workflow
1) Update/confirm plan in `docs/STATUS.md`
2) Make one small change at a time
3) Run tests/build (or ensure CI covers it)
4) Commit with a descriptive message

## Code style
- Rust: prefer clarity over cleverness, keep modules small and testable
- Svelte 5: use runes (`$state`, `$effect`), not legacy stores
- No hardcoded secrets
- macOS threading: `enigo` and other TIS/HIToolbox APIs MUST run on the main thread — use `app_handle.run_on_main_thread()` from async contexts
- Add comments for:
  - hotkey implementation details
  - audio buffering/segmentation
  - paste/AX permission flow

## Local commands
- `cargo tauri dev` — Build and run the app in dev mode (from `flowdictate-tauri/`)
- `cargo check` — Type-check Rust (from `flowdictate-tauri/src-tauri/`)
- `cargo test` — Run Rust unit tests (from `flowdictate-tauri/src-tauri/`)
- `npx svelte-check --tsconfig ./tsconfig.json` — Type-check Svelte/TS (from `flowdictate-tauri/`)
- `tail -f ~/Library/Logs/Sagascript/sagascript.log` — Watch logs

## CLI subcommands (source of truth for app capabilities)
- `sagascript transcribe <file>` — Transcribe audio/video file
- `sagascript record` — Record from mic and transcribe
- `sagascript list-models` — List available whisper models
- `sagascript download-model <id>` — Download a model
- `sagascript config list|get|set|reset|path` — Manage settings
- `sagascript formats` — List supported audio formats

## Current status (2026-01-28)
App is **feature-complete**. Accuracy improvement work (Phase 1) is done on feature branches, not yet merged to main.

### Accuracy Improvement — Phase 1 COMPLETE (3 branches, ready to merge)

| Branch | Strategy | Tests | Commit | Key Changes |
|--------|----------|-------|--------|-------------|
| `feature/accuracy-1-model-upgrade` | Add small.en + large-v3-turbo models | 72 pass | `8a0cb2f` | 2 new WhisperModel cases, English default → smallEn |
| `feature/accuracy-2-custom-vocabulary` | Custom vocabulary + prompt conditioning | 80 pass | `051e7f2` | PromptBuilder, customVocabulary setting, promptTokens wired into WhisperKit |
| `feature/accuracy-3-vad` | VAD + audio normalization + silence trimming | 83 pass | `f002fc6` | AudioProcessor (Accelerate), trim silence before inference, noSpeechThreshold=0.6 |

**All 3 branches modify `WhisperKitBackend.swift` differently — merge conflicts expected. Merge in order: #1, #3, #2 (or resolve conflicts).**

### Accuracy Improvement — Phase 2 NOT STARTED (depends on Phase 1 merge)
- Strategy #4: LLM post-processing (branch: `feature/accuracy-4-llm-correction`)
- Strategy #6: Feedback loop (branch: `feature/accuracy-6-feedback-loop`)

### Accuracy Improvement — Phase 3 NOT STARTED (depends on Phase 2)
- Strategy #5: LoRA voice fine-tuning (branch: `feature/accuracy-5-lora-finetuning`)

### Accuracy Improvement — Phase 4 DEFERRED
- Strategy #7: Apple SpeechAnalyzer (macOS 26+)

### Full plan details
- `docs/plans/accuracy-1-model-upgrade.md` — on branch feature/accuracy-1-model-upgrade
- `docs/plans/accuracy-2-custom-vocabulary.md` — on branch feature/accuracy-2-custom-vocabulary
- `docs/plans/accuracy-3-vad.md` — on branch feature/accuracy-3-vad
- `docs/INSTANCE_LOG.md` — parallel instance coordination log

See `docs/STATUS.md` for full details.

## Subagents
Project-specific subagents may be defined in `.claude/agents/`.
Use them for:
- product/PRD work
- security review
- performance review
- QA/test creation
