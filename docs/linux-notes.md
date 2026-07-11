# Linux-Specific Notes

> **Status: experimental.** The Linux GUI build is community-contributed (issue
> #44) and verified on Ubuntu / X11 / GNOME. Other distros, desktop
> environments, and Wayland have not been tested. The headless CLI build
> (`--no-default-features`) is the well-trodden path on Linux; the GUI build is
> newer.

## Differences from macOS

| Feature | macOS | Linux |
|---|---|---|
| Transcription backend | Metal + Core ML (GPU) | CPU only (Vulkan is a broken upstream opt-in) |
| Permissions required | Microphone, Accessibility | None (no TCC-style gate) |
| Tray behavior | Menu bar icon | System tray via libayatana-appindicator |
| Default hotkey | Ctrl+Shift+Space | Ctrl+Shift+Space |
| Paste shortcut | Cmd+V (enigo) | Ctrl+V (via `xdotool`) |
| Recording overlay | Shown | **Disabled** (see limitations) |
| Settings path | `~/Library/Application Support/ai.gille.sagascript/` | `~/.local/share/ai.gille.sagascript/` (`$XDG_DATA_HOME`) |
| Model path | `~/Library/Application Support/Sagascript/Models/` | `~/.local/share/Sagascript/Models/` (`$XDG_DATA_HOME`) |
| Installer format | `.dmg` | `.deb` / `.rpm` / `.AppImage` |

## Prerequisites

System libraries for the Tauri GUI (Debian/Ubuntu names):

```bash
sudo apt-get install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

Auto-paste shells out to `xdotool` (X11):

```bash
sudo apt-get install xdotool
```

Then the usual Rust + Node toolchain (see the main README) and:

```bash
npm install
cargo tauri build      # GUI app (.deb/.rpm/.AppImage in src-tauri/target/release/bundle/)
```

For a **headless CLI** build (no GUI/Tauri), build the `sagascript-cli` crate
(from `src-tauri/`; the binary is still `target/release/sagascript`):

```bash
cargo build --release -p sagascript-cli                     # transcribe / record (needs ALSA)
cargo build --release -p sagascript-cli --features diarization   # + speaker diarization
```

For a **pure batch-transcription** build with no audio-capture stack at all —
no `record` subcommand, no cpal, and **no ALSA system dependency**:

```bash
cargo build --release -p sagascript-cli --no-default-features
cargo build --release -p sagascript-cli --no-default-features --features diarization
```

## Known limitations

- **CPU-only transcription.** No Metal/Core ML. Prefer `base` or `small` models;
  `large` will be slow. (whisper-rs's `vulkan` feature is currently broken
  upstream.)
- **Recording overlay disabled.** Creating the transparent, always-on-top
  overlay window triggers an X11 window-lifecycle crash that terminates the app
  on several compositors, so the visual recording indicator is suppressed.
  Transcription and auto-paste are unaffected — watch the tray tooltip/title for
  state instead.
- **Auto-paste requires `xdotool` and X11.** enigo's X11 backend leaves the
  Control modifier unmapped, so paste is simulated via `xdotool key ctrl+v`.
  **Wayland is not supported** for auto-paste yet (it would need `ydotool` and
  `wl-clipboard`). On Wayland you can still disable auto-paste and paste manually
  from the clipboard.
- **No auto-updater.** Check the [Releases page](https://github.com/Magnus-Gille/sagascript/releases).

## Troubleshooting

### Auto-paste does nothing

Confirm `xdotool` is installed and you're on an X11 session
(`echo $XDG_SESSION_TYPE` should print `x11`). On Wayland, disable auto-paste in
settings and paste manually — the transcription is always copied to the
clipboard.

### Tray icon missing

Install `libayatana-appindicator3-dev` (build) / the matching runtime package,
and ensure your desktop environment has an app-indicator/system-tray extension
enabled (GNOME needs the AppIndicator extension).
