# FlowDictate (macOS dictation) — Claude Code Project Context

## What this repo is
This repo is a **Claude Code autopilot project** to build a low-latency macOS dictation app (Wispr Flow–style) named **FlowDictate**.

The master prompt is in `PROMPT.md`. Follow it.

## Golden rules
- Do not ask the user questions; make assumptions and document them in `docs/DECISIONS.md`.
- Optimize for **latency** and **perceived speed**.
- Default to **local transcription** (privacy-first). Remote is opt-in.
- Keep UI minimal (menu bar + settings + indicator).

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
- Swift: prefer clarity over cleverness
- Use Swift Concurrency where appropriate; avoid doing heavy work on the main thread
- Keep modules small and testable
- No hardcoded secrets
- Add comments for:
  - hotkey implementation details
  - audio buffering/segmentation
  - paste/AX permission flow

## Local commands
- `swift build` — Build the app
- `swift test` — Run 29 unit tests
- `.build/debug/FlowDictate` — Run the app
- `tail -f ~/Library/Logs/FlowDictate/flowdictate.log` — Watch logs

## Current status (2026-01-23)
App is **feature-complete** with performance optimizations:
- WhisperKit with prewarming, greedy decoding, optimized compute options
- User-selectable models (tinyEn, tiny, baseEn, base)
- RTF logging for performance monitoring
See `docs/STATUS.md` for full details.

## Subagents
Project-specific subagents may be defined in `.claude/agents/`.
Use them for:
- product/PRD work
- security review
- performance review
- QA/test creation
