# FlowDictate Competitive Landscape — Local FOSS Speech-to-Text

**Date:** 2026-02-18
**Status:** Research complete

---

## Executive Summary

No free, open-source tool combines **high-quality Swedish dictation** (KB-Whisper models), **real-time inline dictation**, and **ease of use** for non-technical users. FlowDictate fills a genuine gap.

---

## 1. FOSS Real-Time Dictation Tools

| Tool | Platforms | Engine | Maturity | Swedish? |
|---|---|---|---|---|
| **Handy** | macOS/Win/Linux | Whisper + Parakeet, Tauri | Young (2025) | Standard Whisper only (~12% WER) |
| **Amical** | macOS/Win | Whisper + LLM post-processing | Very early (v0.1.x) | Standard Whisper only |
| **OpenWhispr** | macOS/Win/Linux | Whisper + Parakeet, Electron | Young (2025) | Standard Whisper only |
| **Whispering** | macOS/Win/Linux + browser | whisper.cpp | Young | Standard Whisper only |
| **open-wispr** | macOS only | whisper.cpp + Metal | Very new (Feb 2026) | Standard Whisper only |
| **OpenSuperWhisper** | macOS only | WhisperKit/whisper.cpp | Low activity | Standard Whisper only |
| **OmniDictate** | Windows only | faster-whisper | Active | CC BY-NC (not FOSS) |
| **Scribe** | Windows/Linux | Vosk | Active | Vosk accuracy is poor |

**None use KB-Whisper models for Swedish.**

---

## 2. FOSS File Transcription Tools

| Tool | Platforms | Maturity | Notes |
|---|---|---|---|
| **Buzz** (17.8k stars) | macOS/Win/Linux | Mature | Best FOSS transcription tool. No inline dictation. |
| **Vibe** | macOS/Win/Linux | Maturing | Modern Rust/Tauri. File transcription + batch. No dictation. |
| **Whishper** | Docker (self-hosted) | Active | Web UI. Technical users only. |
| **Scriberr** | Docker (self-hosted) | Active | Diarization + summarization. Technical users only. |

---

## 3. Paid Competitors (for context)

| App | Price | Platforms | Local? | Swedish Quality |
|---|---|---|---|---|
| **Wispr Flow** | $15/month | macOS | No (cloud) | Decent (cloud model) |
| **Superwhisper** | $250 lifetime | macOS | Yes | Standard Whisper (~12% WER) |
| **MacWhisper** | $69 Pro | macOS | Yes | Standard Whisper |
| **VoiceInk** | $25-49 | macOS | Yes | Standard Whisper |
| macOS built-in dictation | Free | macOS | Yes (on-device) | Decent but limited |

---

## 4. Non-Whisper Local STT

| Engine | Status | Notes |
|---|---|---|
| **Vosk** | Active but outclassed | ~85-95% accuracy, no punctuation, all lowercase. Whisper made it largely obsolete for desktop. |
| **Mozilla DeepSpeech** | Dead (archived) | Superseded by Whisper. |
| **NVIDIA Parakeet** | Active | Good English accuracy. Used by Handy. No Swedish-optimized models. |

---

## 5. Swedish-Specific Analysis

**KB-Whisper** (KBLab/Kungliga Biblioteket) models offer the best available Swedish speech recognition:

| Model | WER (Swedish) | Size |
|---|---|---|
| kb-whisper-tiny | ~15% | 39 MB |
| kb-whisper-base | ~9.1% | 74 MB |
| kb-whisper-small | ~6.2% | 244 MB |
| Standard Whisper (large-v3) | ~12% | 1.5 GB |

**No existing FOSS tool integrates KB-Whisper.** All tools that support Swedish use standard OpenAI Whisper models, which have roughly twice the error rate of KB-Whisper for Swedish.

---

## 6. Gap Analysis

### What exists
- File transcription: well served by Buzz and Vibe
- English real-time dictation: multiple young FOSS projects competing
- Paid dictation: Wispr Flow and Superwhisper are polished but expensive and macOS-only

### What does NOT exist
- **Free, local, high-quality Swedish dictation** (KB-Whisper + real-time inline dictation)
- **Combined dictation + file transcription** in one polished FOSS tool
- **Cross-platform (macOS + Windows)** FOSS dictation with native performance

### FlowDictate's unique position
1. **Best Swedish accuracy** — KB-Whisper models (~6-9% WER vs ~12% standard Whisper)
2. **100% local** — privacy-first, no cloud dependency
3. **Real-time inline dictation** — text appears in active app, not a separate window
4. **Free and open source** — no subscriptions, no license restrictions
5. **Native performance** — Swift/AppKit (not Electron), WhisperKit for ANE acceleration
6. **Potential cross-platform** — Rust rewrite feasibility confirmed (see `rust-rewrite-feasibility.md`)

### Closest competitors to watch
1. **Amical** — most ambitious FOSS dictation project, but very early (v0.1.x)
2. **Handy** — best cross-platform FOSS dictation architecture (Tauri), actively maintained
3. **Buzz** — if they add inline dictation, strong competitor for file transcription niche

---

## Sources

- [Handy](https://github.com/cjpais/Handy)
- [Buzz](https://github.com/chidiwilliams/buzz)
- [Vibe](https://github.com/thewh1teagle/vibe)
- [OpenWhispr](https://github.com/OpenWhispr/openwhispr)
- [Amical](https://github.com/amicalhq/amical)
- [Whispering](https://www.producthunt.com/products/whispering)
- [open-wispr (Medium)](https://medium.com/@ammonx9/i-built-a-free-open-source-voice-dictation-app-for-mac-because-the-alternatives-are-absurd-43ab9ca74ae9)
- [KB-Whisper (HuggingFace)](https://huggingface.co/collections/KBLab/kb-whisper-67af9eafb24da903b63cc4aa)
- [KB-Whisper blog](https://kb-labb.github.io/posts/2025-03-07-welcome-KB-Whisper/)
