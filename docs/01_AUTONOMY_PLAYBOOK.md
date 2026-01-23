# Autonomy playbook (how the agent should work)

This playbook is for the Claude Code agent running the Ralph loop.

## Core loop

Every iteration:

1) **Orient**
- Read: `PROMPT.md`, `CLAUDE.md`, `docs/STATUS.md`, `docs/10_DEFINITION_OF_DONE.md`
- Identify what “done” means and what the next smallest objective is.

2) **Plan**
- Update `docs/STATUS.md` with:
  - objective for this iteration
  - risks
  - verification plan (how will you know it worked?)

3) **Execute**
- Prefer small commits and incremental progress.
- Write tests early when possible.

4) **Verify**
- Run tests or ensure CI covers verification.
- If something cannot run in this environment (e.g., macOS-specific build), set up macOS CI.

5) **Commit**
- Commit each meaningful step:
  - “Add PRD skeleton”
  - “Implement hotkey service + unit tests”
  - “Add WhisperKit backend adapter”
  - etc.

6) **Reflect**
- Document decisions and trade-offs in `docs/DECISIONS.md`.
- Add blockers to `docs/BLOCKERS.md`.

## “No user input” strategy

If you need a decision:
- Choose a sensible default that matches the requirements.
- Record the assumption in `docs/DECISIONS.md` (with rationale).
- Proceed.

Examples:
- Default hotkey combo
- Whether dictation is push-to-talk or toggle
- Default local model size
- Whether to auto-detect language or require a toggle

## Use subagents to keep context clean

Recommended custom subagents (project-scoped):
- Product Manager: refine PRD, scope, acceptance criteria
- Architect: propose module design and boundaries
- QA Engineer: generate test plan + test cases
- Security Reviewer: threat model + privacy review
- Performance Engineer: latency budget + profiling plan
- GitHub/DevOps: CI, release automation

Subagents can be defined in `.claude/agents/*.md`.

## Use hooks to enforce quality (optional but recommended)

Once basic build/test commands exist, configure Claude Code hooks to:
- run formatting/lint after file edits
- run test suite before `git commit`
- run a quick build in “Stop” hook (if acceptable)

Keep hooks non-flaky and fast. Prefer:
- small unit test suite locally
- full suite in CI

## Avoid “fake completion”

In a Ralph loop, outputting the completion promise ends the loop.
Only output the promise when ALL definition-of-done items are true.

If stuck for multiple iterations:
- reduce scope to MVP first
- make the system observable (logs, tests)
- then add polish
