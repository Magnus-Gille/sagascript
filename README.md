# Sagascript — Claude Code Autopilot Starter (Ralph Loop)

This repository is a **starter bundle of instructions** (Markdown-first) for running **Claude Code** autonomously using the **Ralph (Ralph Wiggum) loop** to build a **Wispr Flow–style dictation app for macOS**:
- **Push-to-talk** via a configurable global hotkey / button
- **English + Swedish** transcription
- **Ultra low latency + high performance** focus
- **Paste into the currently active app** (like Wispr Flow)
- Runs as a **minimal menu-bar app** with a clear **visual indicator** while dictation is active
- Supports **local models** (preferred) and **remote pay-as-you-go APIs** (optional)

> This bundle is designed so you can unzip it into a cloud sandbox and start Claude Code with minimal fuss.
> After you start the Ralph loop, it should keep iterating until completion without further user input.

---

## What you do (minimal manual steps)

### 1) Open Claude Code in this folder
However you normally run Claude Code (terminal CLI, or the Claude Code web environment), open **this folder**.

### 2) Install/enable the Ralph loop plugin (once)
In the Claude Code REPL:

```
/plugin install ralph-loop@claude-plugins-official
```

### 3) Start the autonomous run
Copy/paste this single command:

```
/ralph-loop:ralph-loop "Read PROMPT.md and execute it exactly. Re-read PROMPT.md at the start of every iteration. Do not ask the user questions. Make reasonable assumptions and document them." --max-iterations 80 --completion-promise "SAGASCRIPT_COMPLETE"
```

That’s it. Walk away.

### 4) If you need to stop it
```
/ralph-loop:cancel-ralph
```

---

## What the agent will produce

The prompt requires Claude to create (and keep updated):

- A full **PRD** (product requirements doc)
- A detailed **architecture** (with diagrams)
- A full **test plan** and runnable tests where possible
- A complete macOS app implementation (MVP → polish)
- CI + release automation (GitHub Actions on macOS runners)
- Security + privacy documentation, and sane defaults

See the `docs/` folder for the seed docs and templates.

---

## Sandboxing / Safety

This starter is explicitly written to run **inside a sandboxed environment**.

The best sandbox is: **run Claude Code in a cloud workspace** (Codespaces / cloud VM / Claude Code web sandbox), not on your host OS.

See:
- `docs/00_START_HERE.md`
- `docs/08_SECURITY_PRIVACY_PLAN.md`

---

## Repo layout

- `PROMPT.md` — the “master prompt” for the autonomous run
- `CLAUDE.md` — persistent project context for Claude Code
- `.claude/` — project-scoped Claude Code configuration and optional subagents
- `docs/` — requirements, architecture seed, NFRs, test plan seed, etc.

---

## License

If you want to publish the resulting codebase, pick a license once the app skeleton exists.
This starter bundle is intended as scaffolding/instructions, not a final product.
