# Sagascript — Windows Port Plan

This document outlines every work item required to ship Sagascript on Windows,
from code changes through CI/CD, installer generation, code signing, documentation,
and website distribution.

---

## Table of Contents

1. [Current State Assessment](#1-current-state-assessment)
2. [Rust Backend Changes](#2-rust-backend-changes)
3. [Frontend Changes](#3-frontend-changes)
4. [Tauri Configuration](#4-tauri-configuration)
5. [CI/CD Pipeline](#5-cicd-pipeline)
6. [Code Signing & Installer](#6-code-signing--installer)
7. [Documentation Updates](#7-documentation-updates)
8. [Website / Distribution](#8-website--distribution)
9. [Work Item Checklist](#9-work-item-checklist) (each phase includes its testing gate)

---

## 1. Current State Assessment

### Already working on Windows (no changes needed)

| Component | File(s) | Notes |
|---|---|---|
| Audio capture | `src-tauri/src/audio/capture.rs` | Uses `cpal` (cross-platform) |
| Audio decoding | `src-tauri/src/audio/decoder.rs` | Uses `symphonia` (cross-platform) |
| Audio resampling | `src-tauri/src/audio/resample.rs` | Pure Rust math |
| Clipboard | `src-tauri/src/paste/service.rs` | Uses `arboard`; already has `#[cfg(target_os = "windows")]` branch for `Key::Control` |
| Keyboard simulation | `src-tauri/src/paste/service.rs` | Uses `enigo` (cross-platform) |
| Global hotkey | `src-tauri/src/hotkey/` | Uses `tauri-plugin-global-shortcut` |
| Settings storage | `src-tauri/src/settings/` | JSON file via `tauri-plugin-store` |
| Credentials | `src-tauri/src/credentials/` | Uses `keyring` crate (wraps Windows Credential Manager) |
| Logging paths | `src-tauri/src/logging/` | Already has `#[cfg(target_os = "windows")]` branch → `%LOCALAPPDATA%\Sagascript\Logs\` |
| CLI commands | `src-tauri/src/cli/` | All platform-agnostic; includes PowerShell completions |
| Whisper (CPU) | `src-tauri/Cargo.toml` | `whisper-rs = "0.15"` already declared under `[target.'cfg(target_os = "windows")'.dependencies]` |
| Tray icon | `src-tauri/src/main.rs` | Tauri system tray is cross-platform |
| Window hide-on-close | `src-tauri/src/main.rs` | Standard Tauri event handling |
| Windows icon | `src-tauri/icons/icon.ico` | Already exists, along with Square*.png and StoreLogo.png |
| Console suppression | `src-tauri/src/main.rs:1` | `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` already present |
| Platform detection | `src-tauri/src/commands.rs` | `get_platform()` command already returns `"windows"` on Windows |

### Needs changes for Windows

| Component | Effort | Details |
|---|---|---|
| Platform module (`windows.rs`) | Medium | Currently empty stub — needs accessibility/permissions equivalent |
| Overlay window | Medium | macOS-specific NSWindow APIs need Win32 equivalent |
| `notify` crate | Low | Currently only enables `macos_kqueue` feature |
| Autostart plugin | Low | Currently hardcoded to `MacosLauncher::LaunchAgent` |
| Whisper GPU acceleration | Optional | CPU works; CUDA support is a nice-to-have |
| CI/CD | Medium | No Windows runner in current pipeline |
| Tauri bundle config | Low | No `windows` section in `tauri.conf.json` |
| Documentation | Medium | README, install docs, website all macOS-only |

---

## 2. Rust Backend Changes

### 2.1 Platform module — `src-tauri/src/platform/windows.rs`

**Current state:** Empty file with two comment lines.

**Required implementation:**

```rust
// src-tauri/src/platform/windows.rs

/// Windows does not have a separate accessibility permission gate like macOS.
/// Input simulation via SendInput works without explicit user grants.
pub fn is_accessibility_trusted() -> bool {
    true // No macOS-style AX permission on Windows
}

/// No-op on Windows — accessibility permissions aren't needed.
pub fn request_accessibility_permission() {
    // Nothing to do
}
```

On macOS, `set_activation_policy_accessory()` hides the app from the Dock. On
Windows the equivalent is handled by Tauri's `skip_taskbar(true)` on the overlay
and not creating a visible main window. No additional Win32 call is needed
because Tauri already suppresses the taskbar entry when there are no visible
windows — the app lives in the system tray.

### 2.2 Overlay window — `src-tauri/src/overlay.rs`

**Current state:** `create_overlay()` is cross-platform, but `configure_macos_window()`
and `macos_order_front()` are macOS-only (`#[cfg(target_os = "macos")]`).

**Required changes:**

Add a Windows equivalent that uses the Tauri window API (which maps to Win32
internally):

```rust
#[cfg(target_os = "windows")]
fn configure_windows_overlay(window: &tauri::WebviewWindow) {
    // Tauri's `.always_on_top(true)` already sets WS_EX_TOPMOST.
    // Tauri's `.skip_taskbar(true)` already hides from taskbar.
    // Tauri's `.decorations(false)` + `.transparent(true)` already handle chrome.
    //
    // For click-through, use the Win32 extended style WS_EX_TRANSPARENT:
    use windows::Win32::UI::WindowsAndMessaging::*;
    let hwnd = window.hwnd().unwrap();
    unsafe {
        let style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        SetWindowLongW(hwnd, GWL_EXSTYLE, style | WS_EX_TRANSPARENT.0 as i32 | WS_EX_LAYERED.0 as i32);
    }
}
```

**Dependencies to add (Windows-only):**
```toml
[target.'cfg(target_os = "windows")'.dependencies]
windows = { version = "0.58", features = ["Win32_UI_WindowsAndMessaging"] }
```

Alternatively, evaluate if Tauri v2's `.ignore_cursor_events(true)` (available
since Tauri 2.1) covers the click-through requirement without raw Win32 calls.
If it does, no additional dependency is needed.

### 2.3 `notify` crate — `src-tauri/Cargo.toml`

**Current state:**
```toml
notify = { version = "7", default-features = false, features = ["macos_kqueue"] }
```

This won't compile on Windows because `macos_kqueue` is the only enabled backend.

**Fix — use conditional features:**

```toml
# Shared (no platform-specific features)
notify = { version = "7", default-features = false }

# macOS: use kqueue
[target.'cfg(target_os = "macos")'.dependencies]
notify = { version = "7", default-features = false, features = ["macos_kqueue"] }

# Windows: use ReadDirectoryChangesW (default backend, no feature flag needed)
[target.'cfg(target_os = "windows")'.dependencies]
notify = { version = "7" }
```

Alternatively, simply use `default-features = true` everywhere, which auto-selects
the correct backend per platform. The `macos_kqueue` feature was likely chosen
for performance on macOS, and the default Windows backend (`ReadDirectoryChangesW`)
is the correct choice.

### 2.4 Autostart plugin — `src-tauri/src/main.rs:104-107`

**Current state:**
```rust
.plugin(tauri_plugin_autostart::init(
    tauri_plugin_autostart::MacosLauncher::LaunchAgent,
    None,
))
```

**Fix — conditional compilation:**

```rust
#[cfg(target_os = "macos")]
let autostart = tauri_plugin_autostart::init(
    tauri_plugin_autostart::MacosLauncher::LaunchAgent,
    None,
);

#[cfg(target_os = "windows")]
let autostart = tauri_plugin_autostart::init(
    tauri_plugin_autostart::MacosLauncher::LaunchAgent, // ignored on Windows
    None,
);
```

`tauri-plugin-autostart` v2 automatically uses the Windows Registry
(`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`) on Windows regardless of
the `MacosLauncher` parameter. The macOS launcher enum is only consulted on
macOS. Verify this by checking the plugin source, but no code change is
likely needed beyond testing.

### 2.5 Whisper GPU acceleration (optional, post-launch)

**Current state:** Windows builds use CPU-only `whisper-rs`.

**Options for GPU on Windows:**

| Backend | Pros | Cons |
|---|---|---|
| CUDA (NVIDIA) | Best perf, most tested | Requires CUDA toolkit in CI, NVIDIA-only |
| Vulkan | Cross-vendor (NVIDIA, AMD, Intel) | Less mature in whisper.cpp |
| DirectML | Native Windows, all GPUs | whisper.cpp support is experimental |
| CPU only | Zero complexity | Slower on large models |

**Recommendation:** Ship v1 with CPU-only. Add CUDA as an opt-in feature
flag in a follow-up release:

```toml
[target.'cfg(target_os = "windows")'.dependencies]
whisper-rs = { version = "0.15", features = ["cuda"] }  # future
```

### 2.6 Commands — `src-tauri/src/commands.rs`

Verify that `check_accessibility_permission` and `request_accessibility_permission`
compile on Windows. They likely call into `platform::macos` behind `#[cfg]`
guards. If not, add Windows branches that return `true` / no-op.

### 2.7 `enigo` threading note

On macOS, `enigo` must run on the main thread (TIS/HIToolbox APIs). On Windows,
`SendInput` works from any thread. The existing `run_on_main_thread()` dispatch
in `main.rs:444` is harmless on Windows but adds unnecessary latency. Consider:

```rust
#[cfg(target_os = "macos")]
{
    app_handle.run_on_main_thread(move || { /* paste */ });
}
#[cfg(not(target_os = "macos"))]
{
    // On Windows, paste directly from the async context
    let paste_svc = PasteService::new();
    paste_svc.paste(&text)?;
}
```

---

## 3. Frontend Changes

### 3.1 Onboarding — `src/lib/Onboarding.svelte`

**Current state:** Already handles Windows correctly. `getSteps()` returns
`["welcome", "ready"]` when `platform !== "macos"`, skipping the macOS-only
microphone and accessibility permission steps.

**One text fix:** Line 186 says "nothing leaves your Mac" — change to
platform-aware wording:

```svelte
All audio is processed locally on your device.
```

### 3.2 Settings — `src/lib/Settings.svelte`

Verify that:
- Hotkey display shows `Ctrl+Shift+Space` (not `Cmd`)
- File dialog works via `tauri-plugin-dialog`
- Model download progress renders correctly

### 3.3 Overlay — `src/lib/Overlay.svelte`

No changes expected — it's purely CSS/HTML.

---

## 4. Tauri Configuration

### 4.1 `src-tauri/tauri.conf.json` — Add Windows bundle config

```json
{
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": [
      "icons/32x32.png",
      "icons/128x128.png",
      "icons/128x128@2x.png",
      "icons/icon.icns",
      "icons/icon.ico"
    ],
    "macOS": {
      "minimumSystemVersion": "13.0"
    },
    "windows": {
      "certificateThumbprint": null,
      "digestAlgorithm": "sha256",
      "timestampUrl": "http://timestamp.digicert.com",
      "webviewInstallMode": {
        "type": "downloadBootstrapper"
      },
      "allowDowngrades": false
    }
  }
}
```

**Key decisions:**
- **MSI vs NSIS:** Tauri v2 supports both. NSIS is recommended for user-facing
  installers (per-user install, custom UI, auto-update support). MSI is better
  for enterprise/IT deployment.
  → **Ship both.** Set `"targets": ["nsis", "msi"]` under the Windows section,
  or keep `"all"` to generate both.
- **WebView2:** Windows requires Edge WebView2 Runtime. The `downloadBootstrapper`
  mode bundles a small (~1.8 MB) bootstrapper that downloads WebView2 if missing.
  This is the recommended approach.

### 4.2 Windows-specific Tauri features

The `macOSPrivateApi` flag in `app` config is ignored on Windows, so no change needed.

---

## 5. CI/CD Pipeline

### 5.1 Add Windows build job — `.github/workflows/ci.yml`

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

jobs:
  check-macos:
    runs-on: macos-14
    steps:
      - name: Checkout
        uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy

      - name: Install Node.js
        uses: actions/setup-node@v4
        with:
          node-version: 20
          cache: npm

      - name: Cache Cargo
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            src-tauri/target
          key: macos-cargo-${{ hashFiles('src-tauri/Cargo.lock') }}
          restore-keys: macos-cargo-

      - name: Install npm dependencies
        run: npm ci

      - name: Build frontend
        run: npm run build

      - name: Cargo check
        working-directory: src-tauri
        run: cargo check

      - name: Cargo test
        working-directory: src-tauri
        run: cargo test

      - name: Cargo clippy
        working-directory: src-tauri
        run: cargo clippy -- -D warnings

      - name: Svelte check
        run: npx svelte-check --tsconfig ./tsconfig.json

      - name: Build Tauri app
        run: npx tauri build

  check-windows:
    runs-on: windows-latest
    steps:
      - name: Checkout
        uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy

      - name: Install Node.js
        uses: actions/setup-node@v4
        with:
          node-version: 20
          cache: npm

      - name: Cache Cargo
        uses: actions/cache@v4
        with:
          path: |
            ~\.cargo\registry
            ~\.cargo\git
            src-tauri\target
          key: windows-cargo-${{ hashFiles('src-tauri/Cargo.lock') }}
          restore-keys: windows-cargo-

      - name: Install npm dependencies
        run: npm ci

      - name: Build frontend
        run: npm run build

      - name: Cargo check
        working-directory: src-tauri
        run: cargo check

      - name: Cargo test
        working-directory: src-tauri
        run: cargo test

      - name: Cargo clippy
        working-directory: src-tauri
        run: cargo clippy -- -D warnings

      - name: Build Tauri app
        run: npx tauri build
```

### 5.2 Release workflow — `.github/workflows/release.yml` (new)

Create a dedicated release workflow triggered by tags:

```yaml
name: Release

on:
  push:
    tags: ["v*"]

permissions:
  contents: write

jobs:
  build-macos:
    runs-on: macos-14
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: actions/setup-node@v4
        with: { node-version: 20, cache: npm }
      - run: npm ci
      - run: npm run build
      - run: npx tauri build
      - name: Upload macOS artifacts
        uses: actions/upload-artifact@v4
        with:
          name: macos-bundle
          path: |
            src-tauri/target/release/bundle/dmg/*.dmg
            src-tauri/target/release/bundle/macos/*.app.tar.gz

  build-windows:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: actions/setup-node@v4
        with: { node-version: 20, cache: npm }
      - run: npm ci
      - run: npm run build
      - run: npx tauri build
      - name: Upload Windows artifacts
        uses: actions/upload-artifact@v4
        with:
          name: windows-bundle
          path: |
            src-tauri/target/release/bundle/nsis/*.exe
            src-tauri/target/release/bundle/msi/*.msi

  publish-release:
    needs: [build-macos, build-windows]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/download-artifact@v4
        with:
          path: artifacts

      - name: Create GitHub Release
        uses: softprops/action-gh-release@v2
        with:
          draft: true
          generate_release_notes: true
          files: |
            artifacts/macos-bundle/**/*
            artifacts/windows-bundle/**/*
```

### 5.3 CI considerations

- **Build time:** Windows Rust builds are significantly slower than macOS. Expect
  15-25 min for a cold build. Cargo caching helps a lot for incremental builds.
- **whisper-rs compilation:** `whisper-rs` compiles `whisper.cpp` from source.
  On Windows this requires a C/C++ compiler. The `windows-latest` runner includes
  MSVC (Visual Studio Build Tools), so this should work out of the box.
- **Svelte check:** Only needs to run once (platform-independent). Consider
  running it only in the macOS job to avoid redundancy.

---

## 6. Code Signing & Installer

### 6.1 Windows code signing

Unsigned Windows executables trigger SmartScreen warnings ("Windows protected
your PC"). For a production release, code signing is essential.

**Options:**

| Provider | Cost | Notes |
|---|---|---|
| SSL.com EV code signing | ~$240/yr | Hardware token or cloud signing; instant SmartScreen trust |
| DigiCert EV | ~$500/yr | Industry standard |
| Certum Open Source | ~$50/yr | For open-source projects; OV cert, needs reputation |
| Self-signed | Free | SmartScreen will block; development only |

**Recommended approach:**
1. Purchase an EV code signing certificate (SSL.com is cost-effective)
2. Store the certificate and password as GitHub Actions secrets
3. Configure Tauri's `certificateThumbprint` in `tauri.conf.json`
4. Sign during the release workflow using `signtool.exe`

**Tauri signing configuration:**

```json
{
  "bundle": {
    "windows": {
      "certificateThumbprint": "<THUMBPRINT>",
      "digestAlgorithm": "sha256",
      "timestampUrl": "http://timestamp.digicert.com"
    }
  }
}
```

In CI, set the `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`
environment variables.

### 6.2 Installer types

Tauri v2 generates these Windows installer formats:

| Format | File | Use case |
|---|---|---|
| **NSIS** | `Sagascript_0.1.0_x64-setup.exe` | Consumer install: per-user, auto-update, uninstaller in Add/Remove Programs |
| **MSI** | `Sagascript_0.1.0_x64_en-US.msi` | Enterprise/IT: Group Policy deployment, per-machine install |

**Recommendation:** Generate both. The NSIS `.exe` is the primary download for
end users. The MSI is available for enterprise users.

### 6.3 Tauri auto-updater (optional, post-launch)

Tauri v2 has a built-in updater that checks a JSON endpoint for new versions.
This can be configured later to provide automatic updates on Windows.

---

## 7. Documentation Updates

### 7.1 README.md

Update the README to reflect Windows support:

**Title/description:**
```
Low-latency, privacy-first dictation for macOS and Windows.
```

**Prerequisites section — add Windows:**
```markdown
### Windows

- Windows 10 (version 1803+) or Windows 11
- Rust 1.75+ (install via [rustup](https://rustup.rs))
- Node.js 20+ (install via [nodejs.org](https://nodejs.org))
- Tauri CLI (`cargo install tauri-cli`)
- Visual Studio Build Tools 2022 (with "Desktop development with C++")
```

**Build output — add Windows:**
```markdown
### Windows
The installer will be in `src-tauri/target/release/bundle/nsis/Sagascript_x.x.x_x64-setup.exe`.
```

**Permissions section — add Windows note:**
```markdown
### Windows permissions
Sagascript works without special permissions on Windows. Microphone access
is granted via the standard Windows privacy settings (Settings → Privacy →
Microphone).
```

### 7.2 CONTRIBUTING.md

Add Windows development setup instructions:
- Install Visual Studio Build Tools 2022
- Enable "Desktop development with C++" workload
- Note that `cargo test` and `cargo clippy` should pass on both platforms

### 7.3 Installation guide — `docs/installation.md` (new)

Create a dedicated installation guide covering:

```markdown
# Installation

## macOS

### Download
Download the latest `.dmg` from the [Releases page](https://github.com/Magnus-Gille/sagascript/releases).

### Install
1. Open the `.dmg` file
2. Drag Sagascript to your Applications folder
3. Launch Sagascript — it will appear in your menu bar
4. Grant Microphone and Accessibility permissions when prompted

### Homebrew (planned)
```
brew install --cask sagascript
```

## Windows

### Download
Download the latest `Sagascript_x.x.x_x64-setup.exe` from the [Releases page](https://github.com/Magnus-Gille/sagascript/releases).

### Install
1. Run the installer
2. If SmartScreen warns about an unrecognized app, click "More info" → "Run anyway"
   (this will not appear once the app is code-signed)
3. Sagascript will appear in your system tray
4. Allow microphone access if prompted by Windows

### MSI (enterprise)
An MSI installer is also available for IT deployment via Group Policy.

### System Requirements
- Windows 10 version 1803 or later / Windows 11
- x64 architecture (ARM64 not yet supported)
- ~200 MB disk space (plus model files)
- Edge WebView2 Runtime (automatically installed if missing)

## Building from source
...
```

### 7.4 Windows-specific documentation — `docs/windows-notes.md` (new)

```markdown
# Windows-Specific Notes

## Differences from macOS

| Feature | macOS | Windows |
|---|---|---|
| Transcription backend | Metal + Core ML GPU | CPU (CUDA planned) |
| Permissions | Accessibility + Microphone prompts | Microphone only |
| Tray behavior | Menu bar icon | System tray icon |
| Hotkey | Ctrl+Shift+Space | Ctrl+Shift+Space |
| Paste shortcut | Cmd+V | Ctrl+V |
| Settings path | ~/Library/Application Support/... | %APPDATA%\... |
| Log path | ~/Library/Logs/Sagascript/ | %LOCALAPPDATA%\Sagascript\Logs\ |
| Model path | ~/.sagascript/models/ | %USERPROFILE%\.sagascript\models\ |

## Known limitations (v1)

- GPU acceleration is not available on Windows (CPU only). Large models
  (large, large-v3) will be significantly slower than on macOS with Metal.
  Recommend using base or small models on Windows.
- Auto-updater is not yet configured for Windows.
- ARM64 (Snapdragon/Copilot+ PCs) is not yet tested.

## Troubleshooting

### "Windows protected your PC" SmartScreen warning
This appears for unsigned applications. Click "More info" → "Run anyway".
This warning will be removed once the app is code-signed.

### Microphone not working
Go to Windows Settings → Privacy & Security → Microphone, and ensure
Sagascript is allowed to access the microphone.

### Hotkey not registering
Some hotkey combinations may conflict with other applications or Windows
shortcuts. Try changing the hotkey in Sagascript Settings.
```

---

## 8. Website / Distribution

### 8.1 Download page

The website (or GitHub Releases page) should offer platform-specific downloads:

```
┌─────────────────────────────────────────────┐
│            Download Sagascript               │
│                                              │
│  ┌──────────────┐  ┌──────────────────────┐  │
│  │   macOS       │  │   Windows            │  │
│  │   .dmg        │  │   .exe installer     │  │
│  │   Intel/Apple │  │   64-bit             │  │
│  │   Silicon     │  │                      │  │
│  │  [Download]   │  │  [Download]          │  │
│  └──────────────┘  └──────────────────────┘  │
│                                              │
│  Other formats: .msi (enterprise)            │
└─────────────────────────────────────────────┘
```

### 8.2 GitHub Releases

Each release tag should produce:

| File | Platform | Notes |
|---|---|---|
| `Sagascript_x.x.x_x64.dmg` | macOS | Universal binary (Intel + Apple Silicon) |
| `Sagascript_x.x.x_x64-setup.exe` | Windows | NSIS installer, includes WebView2 bootstrapper |
| `Sagascript_x.x.x_x64_en-US.msi` | Windows | MSI for enterprise deployment |
| `latest.json` | Both | Auto-updater manifest (future) |

### 8.3 Website updates

If/when a website exists:
- Add Windows to the hero section and feature list
- Add Windows screenshots (tray icon, settings window, overlay)
- Platform-detection JavaScript to auto-select the correct download button
- Add Windows installation instructions

### 8.4 Package managers (post-launch)

| Manager | Platform | Priority |
|---|---|---|
| Homebrew Cask | macOS | High |
| winget | Windows | High |
| Chocolatey | Windows | Medium |
| Scoop | Windows | Low |

**winget submission:**
1. Create a manifest file following [winget-pkgs](https://github.com/microsoft/winget-pkgs) format
2. Submit PR to the winget-pkgs repository
3. Users can then install via: `winget install Sagascript.Sagascript`

---

## 9. Work Item Checklist

Each phase includes its own verification/testing gate — nothing advances
to the next phase until the tests in the current phase pass.

### Phase 1: Compile & run on Windows

**Code changes:**
- [ ] Fix `notify` crate features for cross-platform compilation
- [ ] Implement `platform::windows` module (accessibility stubs)
- [ ] Add Windows overlay configuration (click-through, always-on-top)
- [ ] Fix autostart plugin initialization for cross-platform
- [ ] Verify `commands.rs` accessibility commands compile on Windows
- [ ] Add `windows` crate dependency (if needed for overlay)
- [ ] Fix "nothing leaves your Mac" text in Onboarding.svelte

**Testing gate — phase is not done until all pass:**
- [ ] `cargo check --target x86_64-pc-windows-msvc` succeeds
- [ ] `cargo test` passes on a Windows machine
- [ ] `cargo clippy -- -D warnings` passes on Windows
- [ ] App launches and shows system tray icon
- [ ] Tray menu works (Open Settings, Transcribe File, Quit)
- [ ] Settings window opens and renders correctly
- [ ] Onboarding flow works (Welcome → Ready, skipping macOS-only steps)
- [ ] Hotkey registration works (Ctrl+Shift+Space)
- [ ] Push-to-talk recording works
- [ ] Toggle recording mode works
- [ ] Transcription completes (CPU, base model)
- [ ] Auto-paste works (Ctrl+V simulation)
- [ ] Clipboard fallback works when auto-paste is disabled
- [ ] Recording overlay appears and disappears correctly
- [ ] Overlay is click-through
- [ ] File transcription works (file dialog)
- [ ] Model download works
- [ ] Settings persist across restarts
- [ ] CLI commands work from PowerShell and Command Prompt

### Phase 2: CI/CD

**Code changes:**
- [ ] Add `check-windows` job to `.github/workflows/ci.yml`
- [ ] Create `.github/workflows/release.yml` with dual-platform builds
- [ ] Configure artifact upload for Windows bundles

**Testing gate:**
- [ ] CI passes on both macOS and Windows runners (cargo check, test, clippy, build)
- [ ] Release workflow produces NSIS `.exe` and `.msi` artifacts
- [ ] macOS CI is not broken by the changes

### Phase 3: Installer & Tauri config

**Code changes:**
- [ ] Add `windows` section to `tauri.conf.json` bundle config
- [ ] Configure NSIS installer settings (install path, shortcuts, uninstaller)

**Testing gate:**
- [ ] Fresh install on clean Windows 10 (version 1803 minimum)
- [ ] Fresh install on clean Windows 11
- [ ] Upgrade install over previous version
- [ ] Uninstaller: clean removal via Add/Remove Programs
- [ ] WebView2 bootstrapper installs runtime if missing
- [ ] Icon renders correctly in taskbar, tray, Start menu, and installer
- [ ] Autostart works (toggle in settings, verify in Task Manager → Startup)
- [ ] Test on HiDPI display (150%, 200% scaling) — overlay positioning correct
- [ ] Test with non-English Windows locale (path handling, UI rendering)

### Phase 4: Code signing

**Code changes:**
- [ ] Obtain EV code signing certificate
- [ ] Configure certificate in GitHub Actions secrets
- [ ] Add signing step to release workflow

**Testing gate:**
- [ ] Signed installer does not trigger SmartScreen warning
- [ ] Certificate details show correct publisher name in file properties
- [ ] Timestamp server is used (signature remains valid after cert expiry)

### Phase 5: Documentation & distribution

**Code changes:**
- [ ] Update README.md with Windows support
- [ ] Update CONTRIBUTING.md with Windows dev setup
- [ ] Create `docs/installation.md`
- [ ] Create `docs/windows-notes.md`
- [ ] Update website download page (if exists)

**Testing gate:**
- [ ] GitHub Release created with both macOS and Windows artifacts
- [ ] Download links work and files are not corrupted
- [ ] Installation instructions are accurate (follow them on a clean machine)
- [ ] Submit to winget package manager and verify `winget install` works

### Phase 6: Post-launch improvements

- [ ] Add CUDA support for NVIDIA GPU acceleration
- [ ] Configure Tauri auto-updater for Windows
- [ ] Test on ARM64 Windows (Snapdragon)
- [ ] Add Chocolatey package
- [ ] Performance profiling: CPU transcription benchmarks vs macOS Metal
- [ ] Low-RAM testing (4 GB) — verify model loading doesn't OOM

---

## Architecture Diagram: Windows Build Pipeline

```
  git push tag v*
        │
        ├──────────────────────────┐
        │                          │
  ┌─────▼─────┐            ┌──────▼──────┐
  │  macOS 14  │            │  Windows    │
  │  Runner    │            │  Latest     │
  ├────────────┤            ├─────────────┤
  │ npm ci     │            │ npm ci      │
  │ npm build  │            │ npm build   │
  │ tauri build│            │ tauri build │
  ├────────────┤            ├─────────────┤
  │ Output:    │            │ Output:     │
  │  .dmg      │            │  .exe (NSIS)│
  │  .app.gz   │            │  .msi       │
  └─────┬──────┘            └──────┬──────┘
        │                          │
        ├──────────────────────────┘
        │
  ┌─────▼──────────┐
  │ GitHub Release  │
  │ (draft)         │
  │                 │
  │ - .dmg          │
  │ - .exe          │
  │ - .msi          │
  └─────────────────┘
```
