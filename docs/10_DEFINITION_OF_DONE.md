# Definition of Done (DoD)

This is the checklist for ending the Ralph loop.

The completion promise is:
**FLOWDICTATE_COMPLETE**

Only output it when all items below are true.

## Product functionality
- [ ] Menu bar app launches and stays resident
- [ ] Global hotkey works system-wide
- [ ] Dictation starts immediately on trigger
- [ ] Visual indicator clearly shows “active dictation”
- [ ] Dictation stops reliably (push-to-talk release or toggle)
- [ ] Transcript is produced in English and Swedish
- [ ] Transcript is pasted into the active application reliably

## Backends
- [ ] Local backend works (WhisperKit)
- [ ] Remote backend exists (OpenAI transcription) and can be enabled
- [ ] API key stored in Keychain and never logged

## Performance
- [ ] Basic benchmark instrumentation exists
- [ ] `docs/BENCHMARKS.md` recorded at least once on target-ish hardware

## Quality
- [ ] Unit tests exist for key services
- [ ] CI configured on macOS and passing
- [ ] Docs updated:
  - [ ] PRD
  - [ ] Architecture
  - [ ] NFRs
  - [ ] Security/Privacy
  - [ ] Test Plan
  - [ ] Status
  - [ ] Decisions

## Safety
- [ ] No secrets committed
- [ ] Permissions are requested only when needed and explained
