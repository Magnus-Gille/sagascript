# Security & privacy plan (local-first)

## Data classification

- Microphone audio: **highly sensitive**
- Transcripts: sensitive (may contain personal data)
- API keys: secret

## Principles

- Local-first transcription by default
- No audio persistence unless user explicitly enables debug
- Least-privilege permissions
- Secure secret storage (Keychain)

## Threat model (high level)

### Threats
- Accidental logging of audio/transcripts
- Leaked API keys
- Abuse of Accessibility permission
- Remote service receives sensitive audio without clear consent

### Mitigations
- Never log raw audio buffers
- Never log API keys
- Keep “remote transcription” opt-in and clearly labeled
- Keychain-based storage for secrets
- Minimal code surface for event injection; prefer clipboard restore logic
- Document required permissions and why

## Permissions UX

- Microphone:
  - request on first dictation attempt
  - show UI explaining why

- Accessibility:
  - required to paste text into other apps (simulate ⌘V)
  - request only when user enables “paste into active app”
  - provide “how to enable” instructions in-app

## Remote transcription safety (OpenAI)

- Only send audio when remote backend is enabled
- Use TLS
- Provide per-request cancellation
- Allow “local only” mode always

## Sandboxing for the development agent

This repo is meant to be developed in a cloud sandbox.
If run locally:
- use a container / dev environment
- restrict Claude Code permissions to this folder
- avoid running as admin/root

