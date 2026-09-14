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
   - **Accessibility** -- for pasting transcriptions into the active app and
     for bare F13–F24 shortcuts

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

> **Unsigned beta:** Sagascript provides a [Windows beta prerelease](https://github.com/Magnus-Gille/sagascript/releases/tag/windows-beta-20260905)
> for Windows 11 on x64 and ARM64. It is a public preview, not a signed
> or stable release. Verify the matching checksum before running it and do not
> bypass SmartScreen.

### System requirements

- Windows 11
- x64 or ARM64 architecture
- ~200 MB disk space (plus Whisper model files)
- Edge WebView2 Runtime (automatically installed if missing)

### Download and install

1. Open the [Windows beta prerelease](https://github.com/Magnus-Gille/sagascript/releases/tag/windows-beta-20260905).
2. Download `Sagascript-Windows-x64-Setup.exe` on an Intel/AMD PC or
   `Sagascript-Windows-arm64-Setup.exe` on a native ARM64 PC.
3. Download the matching `SHA256SUMS-Windows-<architecture>` file and verify
   the installer checksum before running it.
4. Complete the installer, then launch Sagascript from the Start menu.

The beta is unsigned, so Windows may show a SmartScreen warning. Follow your
device's security policy. If you need a signed stable installation, wait for
the signed Windows release; building from source does not provide a publisher
signature.

The prerelease also contains matching MSI, portable desktop, and CLI files.
The CLI is separate from the desktop executable and will not be automatically
added to `PATH` by this beta.

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
- **Windows:** Local source builds produce an NSIS installer in
  `src-tauri/target/release/bundle/nsis/` and an MSI in
  `src-tauri/target/release/bundle/msi/`. These packages are suitable for
  development and are separate from the beta artifacts.
