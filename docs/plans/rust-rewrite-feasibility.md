# FlowDictate Rust/Cross-Platform Rewrite — Feasibility Spike

**Date:** 2026-02-18
**Status:** Research complete, awaiting decision

---

## Executive Summary

A Rust + Tauri rewrite is **feasible** for macOS + Windows. The main risks are around ANE acceleration on Mac (whisper.cpp CoreML is slower than WhisperKit) and Linux/Wayland support (fundamental security model conflicts). Estimated effort: **3-5 weeks** for macOS + Windows parity.

---

## 1. whisper.cpp CoreML/ANE vs WhisperKit

**Verdict: whisper.cpp CoreML works but is meaningfully slower than WhisperKit.**

| | WhisperKit | whisper.cpp + CoreML |
|---|---|---|
| What runs on ANE | Encoder + Decoder | Encoder only (decoder on CPU) |
| Speedup vs CPU | 5-10x | ~3x |
| First-run penalty | None (pre-compiled models) | ANE compilation: minutes for small models, **hours for large** |
| Model format | CoreML bundle (pre-built) | GGML .bin + separate .mlmodelc (both needed) |
| Streaming | Native, optimized | Sliding window + VAD (functional) |
| KB-Whisper CoreML | Not available (would need conversion) | Not available (would need conversion) |

**Key finding:** whisper.cpp CoreML only accelerates the encoder on ANE. The decoder stays on CPU. WhisperKit runs both on ANE, which is why it's faster. For a dictation app doing short utterances, the decoder is a significant portion of total time.

**For small models (base/small):** The gap is tolerable. whisper.cpp CoreML on a base model still transcribes ~5s audio in well under 1 second on Apple Silicon. For dictation use cases, this is fast enough.

---

## 2. whisper-rs (Rust Bindings)

**Verdict: Mature enough. GO.**

- **Version:** 0.15.1, ~152k downloads, 930 GitHub stars
- **CoreML feature flag:** `features = ["coreml"]` — compiles whisper.cpp with CoreML support
- **Metal flag:** `features = ["metal"]` — GPU acceleration (can combine with CoreML)
- **CUDA flag:** `features = ["cuda"]` — for Windows Nvidia GPUs
- **API:** Clean. `WhisperContext` (model) + `WhisperState` (inference) separation allows concurrent use

**Concern:** Repo moved to Codeberg (GitHub archived July 2025). Single maintainer. But whisper.cpp itself is stable and the Rust bindings are a thin FFI layer — low risk of breaking changes.

**Streaming gap:** The `new_segment_callback` is an unsafe C callback, not a Rust-safe abstraction. For dictation, a chunked-submission approach (submit audio chunks, poll for results) is more practical than the callback path.

---

## 3. Tauri v2 Capabilities

**Verdict: Strong fit for macOS + Windows. GO.**

| Capability | macOS | Windows | How |
|---|---|---|---|
| System tray (no dock icon) | Yes | Yes | `TrayIconBuilder` + `ActivationPolicy::Accessory` |
| Global hotkeys | Yes | Yes | `tauri-plugin-global-shortcut` |
| Paste simulation | Yes (needs AX permission) | Yes | `enigo` crate from Rust backend |
| Microphone access | Yes | Yes | `cpal` crate (16kHz mono capture) |
| Keychain / Credential Manager | Yes | Yes | `tauri-plugin-keyring` or `keyring` crate |
| Launch at login | Yes | Yes | `tauri-plugin-autostart` |
| App bundle size | ~5-10 MB | ~5-10 MB | vs ~100 MB for Electron |

**Limitations:**
- Fn-key / modifier-only hotkeys need custom `CGEventTap` code on macOS (same as current Swift app)
- Paste simulation on macOS crashes without Accessibility permission — need to check/prompt first
- All transcription logic stays in the Rust backend; only UI text goes to the webview frontend

---

## 4. KB-Whisper CoreML Models

**Verdict: No pre-built CoreML models exist. Conversion is doable but adds a build step.**

- KBLab ships: safetensors, GGML, ONNX, ctranslate2. **No CoreML.**
- KB-Whisper uses standard Whisper architecture (just different weights) — conversion should work
- Conversion tool: `whisper.cpp/models/generate-coreml-model.sh -h5 base ./kb-whisper-base`
- Time: ~1 min (tiny), ~5 min (base), ~15 min (small). One-time dev machine step.
- Need to host the converted `.mlmodelc` files (HuggingFace or CDN)
- At runtime: both GGML .bin AND .mlmodelc needed side-by-side (~63% more storage for base)

**Note:** Even without CoreML conversion, KB-Whisper GGML files work on CPU through whisper-rs. CoreML is an optimization, not a requirement.

---

## 5. Rust Platform Crate Ecosystem

| Need | Crate | macOS | Windows | Maturity |
|---|---|---|---|---|
| Audio capture | `cpal` 0.17 | CoreAudio | WASAPI | **Mature** (10M downloads) |
| Global hotkeys | `global-hotkey` | Yes | Yes | **Mature** |
| Paste simulation | `enigo` | Yes (AX perm) | Yes (SendInput) | **Usable** |
| Clipboard | `arboard` (1Password) | Yes | Yes | **Mature** |
| System tray | `tray-icon` | Yes | Yes | **Usable** |
| Credentials | `keyring` | Keychain | Cred Manager | **Mature** |
| Launch at login | `auto-launch` | Launch Agent | Registry | **Usable** |
| Overlay indicator | `winit` or Tauri window | Yes | Yes | **Usable** |

**All critical needs are covered for macOS + Windows.** Linux/Wayland has gaps in hotkeys and paste simulation (Wayland intentionally blocks these).

---

## Architecture Proposal

```
┌─────────────────────────────────────────────────┐
│              Tauri v2 Shell                      │
│   HTML/CSS UI (settings, overlay indicator)      │
├─────────────────────────────────────────────────┤
│              Rust Core                           │
│                                                  │
│  ┌───────────┐  ┌──────────┐  ┌──────────────┐ │
│  │   cpal    │  │ whisper  │  │   enigo      │ │
│  │  (audio)  │→ │   -rs    │→ │ (paste sim)  │ │
│  └───────────┘  │          │  └──────────────┘ │
│                 ├──────────┤  ┌──────────────┐ │
│                 │ macOS:   │  │  arboard     │ │
│                 │ CoreML+  │  │ (clipboard)  │ │
│                 │ Metal    │  └──────────────┘ │
│                 │ Windows: │  ┌──────────────┐ │
│                 │ CUDA /   │  │  keyring     │ │
│                 │ CPU      │  │ (secrets)    │ │
│                 └──────────┘  └──────────────┘ │
└─────────────────────────────────────────────────┘
```

---

## Effort Estimate

| Phase | Work | Time |
|---|---|---|
| 1. Scaffold Tauri app + tray icon | Boilerplate, menu, settings window | 2-3 days |
| 2. Audio capture → whisper-rs | cpal capture, resampling to 16kHz, whisper-rs integration | 3-4 days |
| 3. Hotkey + paste pipeline | global-hotkey registration, clipboard write, enigo paste | 2-3 days |
| 4. Model management | Download GGML files, cache, model selection UI | 2-3 days |
| 5. KB-Whisper CoreML | Convert models, host files, conditional CoreML loading on macOS | 2-3 days |
| 6. Settings + polish | Language/model/backend picker, overlay indicator, launch at login | 3-4 days |
| 7. Windows testing + fixes | Platform-specific bug fixes, CUDA testing | 3-4 days |
| **Total** | | **~3-5 weeks** |

---

## Go / No-Go Risks

| Risk | Severity | Mitigation |
|---|---|---|
| whisper.cpp CoreML slower than WhisperKit | Medium | Acceptable for base/small models. Users won't notice for short dictation. |
| KB-Whisper CoreML conversion fails | Low | Standard Whisper architecture — should work. Test tiny first. |
| whisper-rs single maintainer | Low | Thin FFI layer, whisper.cpp itself is very active. Could fork if needed. |
| Tauri doesn't support Fn hotkeys | Low | Same as current Swift app — need CGEventTap FFI. Known solution. |
| Linux/Wayland gaps | Medium | Defer Linux support to v2. Focus on macOS + Windows first. |
| macOS ANE first-run compilation | Low | Only for base/small models = minutes, not hours. Cache after first run. |

---

## Recommendation

**GO for macOS + Windows.** The ecosystem is mature enough, and the architecture (Rust core + Tauri shell + whisper-rs) is sound. The main trade-off vs the current Swift app is ~2-3x slower transcription on Mac (whisper.cpp CoreML vs WhisperKit), but this is still sub-second for dictation-length audio.

**Defer Linux** until Wayland's input simulation story improves.

**Alternative considered:** Keep Swift for macOS, build separate C#/WPF app for Windows. More work to maintain two codebases, but each would have the best native experience. The Rust path is a bet on "one codebase, 90% shared."
