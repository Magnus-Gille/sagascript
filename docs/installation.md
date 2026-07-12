# Installation

## macOS

### System requirements

- macOS 13.0 (Ventura) or later
- Apple Silicon (M1+) is required for the v1 binary release. Intel Macs are not
  supported by the v1 installer.
- ~200 MB disk space (plus Whisper model files)

### Download

Download the latest `.dmg` from the [Releases page](https://github.com/Magnus-Gille/sagascript/releases).

### Install

1. Open the `.dmg` file
2. Drag Sagascript to your Applications folder
3. Launch Sagascript -- it will appear in your menu bar
4. Grant permissions when prompted:
   - **Microphone** -- for recording audio
   - **Accessibility** -- for pasting transcriptions into the active app

To make the app's CLI available in your shell, create this link once:

```bash
sudo mkdir -p /usr/local/bin
sudo ln -sfn /Applications/Sagascript.app/Contents/MacOS/sagascript /usr/local/bin/sagascript
sagascript --version
```

The version output includes the release's Git revision and build date so a
stale installation is immediately visible.

### Upgrade

1. Quit Sagascript completely.
2. Open the new DMG and drag Sagascript to Applications.
3. Choose **Replace** when Finder asks; do not merge or retain the old bundle.
4. Run `sagascript --version` and confirm it reports the new release revision.

The `/usr/local/bin/sagascript` link above points into the app bundle, so it
automatically reaches the replacement executable. If it points elsewhere,
repeat the `ln -sfn` command before testing the upgraded CLI.

### Accessibility onboarding release check

Release acceptance must exercise the case where System Settings is already
open on an unrelated pane:

1. Install and launch the signed `/Applications/Sagascript.app`; do not use an
   unsigned development bundle for this check.
2. Leave System Settings open on Wi-Fi (or another unrelated pane).
3. Reset only the release app's grant with
   `tccutil reset Accessibility ai.gille.sagascript`.
4. In onboarding, click **Open System Settings**.
5. Verify System Settings comes forward on **Privacy & Security >
   Accessibility**, enable the exact installed Sagascript row, and confirm
   onboarding changes to **Accessibility granted** without an app relaunch.
6. Confirm **I'll paste manually** remains available while permission is not
   granted.

### Homebrew (planned)

```
brew install --cask sagascript
```

## Windows

> **Build-from-source preview:** Sagascript v1 publishes official binaries for
> macOS only. The project does not publish or endorse unsigned Windows
> installers. Windows users can inspect and build the current preview from
> source.

### System requirements

- Windows 10 version 1803 or later, or Windows 11
- x86_64 architecture (ARM64 not yet supported)
- ~200 MB disk space (plus Whisper model files)
- Edge WebView2 Runtime (automatically installed if missing)

Follow the build-from-source instructions below. If Windows warns about a
binary, do not bypass SmartScreen; verify the source and build it yourself.

## Building from source

### Prerequisites

**All platforms:**

- Rust 1.75+ (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh` or [rustup.rs](https://rustup.rs))
- Node.js 20+ (`brew install node` on macOS, or [nodejs.org](https://nodejs.org) on Windows)
- Tauri CLI (`cargo install tauri-cli`)

**macOS additional:**

- Xcode Command Line Tools (`xcode-select --install`)

**Windows additional:**

- [Visual Studio Build Tools 2022](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the **"Desktop development with C++"** workload

### Build and run

```bash
git clone https://github.com/Magnus-Gille/sagascript.git
cd sagascript
npm install
cargo tauri dev
```

### Build a release binary

```bash
cargo tauri build
```

- **macOS:** The `.app` bundle will be in `src-tauri/target/release/bundle/macos/`
- **Windows preview:** Local source builds produce an NSIS installer in
  `src-tauri/target/release/bundle/nsis/` and an MSI in
  `src-tauri/target/release/bundle/msi/`. These locally built packages are not
  official Sagascript v1 artifacts.
