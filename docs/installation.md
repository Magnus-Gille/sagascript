# Installation

## macOS

### System requirements

- macOS 13.0 (Ventura) or later
- Apple Silicon (M1+) or Intel x86_64
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
   - **Input Monitoring** -- for the global hotkey

### Homebrew (planned)

```
brew install --cask sagascript
```

## Windows

### System requirements

- Windows 10 version 1803 or later, or Windows 11
- x86_64 architecture (ARM64 not yet supported)
- ~200 MB disk space (plus Whisper model files)
- Edge WebView2 Runtime (automatically installed if missing)

### Download

Download the latest `Sagascript_x.x.x_x64-setup.exe` from the [Releases page](https://github.com/Magnus-Gille/sagascript/releases).

### Install

1. Run the installer
2. If Windows SmartScreen warns about an unrecognized app, click **"More info"** then **"Run anyway"** (this will not appear once the app is code-signed)
3. Sagascript will appear in your system tray
4. Allow microphone access if prompted by Windows

### MSI (enterprise)

An `.msi` installer is also available on the [Releases page](https://github.com/Magnus-Gille/sagascript/releases) for IT deployment via Group Policy or other management tools.

### winget (planned)

```
winget install Sagascript.Sagascript
```

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
- **Windows:** The NSIS installer will be in `src-tauri/target/release/bundle/nsis/` and the MSI in `src-tauri/target/release/bundle/msi/`
