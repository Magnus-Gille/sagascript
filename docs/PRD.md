# PRD — FlowDictate

## 1. Overview

FlowDictate is a macOS menu bar application that provides low-latency, privacy-first voice dictation. Inspired by Wispr Flow, it enables users to transcribe speech directly into any application using a global hotkey.

**Target Platform:** macOS (Apple Silicon — M1/M2/M3/M4)
**Primary Languages:** English and Swedish
**Privacy Model:** Local-first transcription with optional remote backend

## 2. Goals

1. **Instant dictation** — Press a hotkey, speak, release, and see text appear in the active application within 1-2 seconds.
2. **Privacy-first** — All transcription happens on-device by default using WhisperKit.
3. **Minimal UI** — Menu bar app with simple settings; no intrusive windows.
4. **Clear feedback** — Users always know when dictation is active via visual indicators.
5. **Multi-language** — Support English and Swedish transcription.

## 3. Non-goals

1. **Real-time streaming transcription UI** — Text appears after utterance ends, not live.
2. **Voice commands / assistant features** — This is pure dictation, not Siri-like.
3. **Custom wake words** — Dictation is hotkey-triggered only.
4. **Windows/Linux support** — macOS only for MVP.
5. **Audio recording/playback** — No audio is persisted by default.

## 4. User Stories

### US-1: Quick dictation
**As a** knowledge worker
**I want to** press a hotkey and speak to type text
**So that** I can compose messages/documents faster than typing

**Acceptance criteria:**
- Global hotkey (default: Option+Space) activates dictation from any app
- Audio capture begins within 100ms of hotkey press
- Releasing hotkey (or second press for toggle mode) stops recording
- Transcription completes within 2 seconds for typical utterances
- Transcribed text is pasted into the currently focused text field

### US-2: Visual feedback
**As a** user
**I want to** clearly see when dictation is active
**So that** I know the app is listening and I can speak confidently

**Acceptance criteria:**
- Menu bar icon changes state during recording (e.g., fills in red)
- Optional floating HUD overlay shows "Recording..." indicator
- Overlay is non-intrusive and auto-hides on stop

### US-3: Language selection
**As a** bilingual user
**I want to** choose my dictation language (English or Swedish)
**So that** transcription accuracy is optimized for my speech

**Acceptance criteria:**
- Settings allows selecting language: English, Swedish, or Auto-detect
- Auto-detect uses model's language detection (may be less accurate)
- Language preference persists across app restarts

### US-4: Local-first privacy
**As a** privacy-conscious user
**I want to** transcribe entirely on-device
**So that** my voice data never leaves my computer

**Acceptance criteria:**
- Local backend (WhisperKit) is the default
- No network requests are made during transcription by default
- Audio is processed in memory and discarded after transcription

### US-5: Optional cloud transcription
**As a** user who wants maximum accuracy
**I want to** optionally use OpenAI's transcription API
**So that** I can trade privacy for better accuracy when I choose

**Acceptance criteria:**
- Settings toggle to enable remote transcription
- Clear disclosure that audio will be sent to OpenAI
- API key stored securely in macOS Keychain
- API key is never logged or displayed after entry

### US-6: Menu bar presence
**As a** user
**I want** the app to run unobtrusively in the menu bar
**So that** it's always available without cluttering my screen

**Acceptance criteria:**
- App launches as a menu bar app (no Dock icon)
- Clicking menu bar icon shows dropdown with settings and quit options
- App persists in background until explicitly quit

## 5. UX Flows

### Flow 1: First Launch

```
1. User launches FlowDictate
2. App requests Microphone permission (system dialog)
3. App requests Accessibility permission (for paste simulation)
4. App appears in menu bar with idle icon
5. User can access Settings from menu bar dropdown
```

### Flow 2: Dictation (Push-to-talk mode)

```
1. User holds Option+Space
2. Menu bar icon turns red; optional HUD appears
3. User speaks
4. User releases Option+Space
5. Audio is transcribed locally (or remotely if enabled)
6. Transcribed text is pasted into active app
7. Icon returns to idle; HUD dismisses
```

### Flow 3: Dictation (Toggle mode)

```
1. User presses Option+Space (first press)
2. Recording starts; visual indicators activate
3. User speaks freely
4. User presses Option+Space again (second press)
5. Recording stops; transcription begins
6. Text is pasted; indicators deactivate
```

### Flow 4: Settings

```
1. User clicks menu bar icon
2. Dropdown appears: [Settings...] [Quit]
3. User clicks Settings
4. Settings window opens:
   - Hotkey configuration
   - Language selection (English / Swedish / Auto)
   - Transcription backend (Local / Remote)
   - If Remote: API key field + security notice
   - Hotkey mode (Push-to-talk / Toggle)
5. Changes save automatically
```

## 6. Acceptance Criteria (MVP)

| Feature | Criterion |
|---------|-----------|
| Hotkey | Global hotkey works in any application |
| Recording | Audio capture starts within 100ms |
| Indicator | Menu bar icon + optional HUD clearly show recording state |
| Transcription | English and Swedish supported with selectable language |
| Paste | Text pasted into active app via clipboard + Cmd+V |
| Local backend | WhisperKit works offline with reasonable accuracy |
| Remote backend | OpenAI API works when enabled and configured |
| API key security | Key stored in Keychain, never logged |
| Settings | All options accessible via clean UI |
| Performance | Typical utterance transcribed in under 2 seconds |

## 7. Telemetry & Privacy

### Data handling principles
1. **No audio persistence** — Audio buffers are discarded after transcription
2. **No telemetry by default** — App does not phone home
3. **No analytics** — No usage tracking in MVP
4. **Transparent data flow** — Remote transcription clearly disclosed when enabled

### Permissions required
| Permission | Why needed | When requested |
|------------|------------|----------------|
| Microphone | Audio capture for transcription | First launch or first dictation |
| Accessibility | Simulate Cmd+V paste into active app | First launch or first dictation |

### If remote transcription enabled
- Audio sent to OpenAI API over TLS
- Subject to OpenAI's data retention policies
- Clear toggle + disclosure in settings
