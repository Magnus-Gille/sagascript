# Sagascript

[![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/Magnus-Gille/sagascript/actions/workflows/ci.yml/badge.svg)](https://github.com/Magnus-Gille/sagascript/actions/workflows/ci.yml)

Dictate anywhere. Privately. A lightweight menu bar app for macOS. Press a
hotkey, speak, and text appears in any application. Audio and transcripts stay
on your Mac; an internet connection is used only when you choose to download a
speech or diarization model.

## Features

- **Push-to-talk dictation** -- hold a global hotkey, speak, release to transcribe and paste into any app
- **Local transcription** -- audio and transcripts are processed on-device with Metal/Core ML; they are not uploaded
- **Nordic-grade accuracy** -- Swedish and Norwegian use [KB-Whisper](https://huggingface.co/KBLab) (Swedish National Library) and [NB-Whisper](https://huggingface.co/NbAiLab) (Norwegian National Library), fine-tuned on 50,000+ hours of Nordic speech with 47% fewer errors than generic Whisper
- **Privacy by default** -- no telemetry, cloud transcription, or transcript upload; network access is limited to model downloads you initiate
- **No telemetry or tracking** -- no analytics, no usage sharing, no data collection of any kind
- **Multi-language** -- English, Swedish, and Norwegian with dedicated models; additional languages supported via generic Whisper models
- **CLI + GUI** -- full CLI for scripting and automation, menu bar app for everyday use
- **File transcription** -- transcribe audio and video files (MP3, WAV, M4A, FLAC, MP4, MKV, OGG, and more)
- **Configurable** -- choose your model, language, hotkey, and output behavior
- **macOS v1** -- official releases are signed and notarized for macOS 13+; Apple Silicon is the tested launch platform, while the universal build's Intel slice still requires hardware acceptance
- **Windows preview** -- the Windows port remains available for build-from-source testing; no official Windows binaries are published yet

## Building from source

### Prerequisites

- **macOS**: macOS 13.0+ (Apple Silicon tested; the universal build's Intel slice requires hardware acceptance before support is claimed)
- **Windows preview**: Windows 10+ (build from source; not an official v1 release)
- **Linux** (experimental): X11 session; GTK/WebKit dev libraries + `xdotool` — see [Linux notes](docs/linux-notes.md)
- Rust 1.75+ (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
- Node.js 20+ (`brew install node` on macOS, or download from [nodejs.org](https://nodejs.org) on Windows)
- Tauri CLI (`cargo install tauri-cli`)

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

On macOS the `.app` bundle will be in `src-tauri/target/release/bundle/macos/`.
Source builds can also produce Windows or experimental Linux packages; these
are not official v1 release artifacts. See the platform notes below.

## CLI usage

Sagascript includes a full CLI. The desktop binary itself accepts every CLI subcommand, and a headless CLI-only binary (no GUI dependencies) can be built with `cargo build -p sagascript-cli --release` from `src-tauri/`. Either way the binary lands at `src-tauri/target/release/sagascript` — it is whichever was built last (or use the app bundle).

```bash
# Transcribe an audio/video file
sagascript transcribe recording.mp3

# Record from microphone and transcribe
sagascript record

# List available Whisper models
sagascript list-models

# Download a model
sagascript download-model ggml-base.en

# Manage settings
sagascript config list
sagascript config set language sv
sagascript config get hotkey

# Generate shell completions
sagascript completions zsh > ~/.zfunc/_sagascript

# Generate man pages
sagascript manpages --dir /usr/local/share/man/man1
```

Run `sagascript --help` for the full list of commands.

## Permissions

### macOS

Sagascript needs the following permissions (macOS will prompt you on first use):

- **Microphone** -- for recording audio
- **Accessibility** -- for pasting transcriptions into the active app
Official macOS releases are Developer ID signed and notarized. If a downloaded
release asks you to bypass Gatekeeper, do not run it; report the artifact.

### Windows

Windows is currently a build-from-source preview. It needs microphone access
for recording audio. Do not install an unsigned binary from an untrusted party.

## Documentation

- [Installation guide](docs/installation.md) -- detailed install instructions for macOS and Windows
- [Linux notes](docs/linux-notes.md) -- experimental Linux build, prerequisites, and known limitations
- [Windows-specific notes](docs/windows-notes.md) -- feature comparison, known limitations, and troubleshooting
- [Third-party notices](THIRD_PARTY_NOTICES.md) -- dependency and downloadable-model licenses
- [Model sources and integrity manifest](docs/model-sources.md) -- pinned revisions, licenses, sizes, and SHA-256 checksums

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, code style, and how to submit changes.

## Acknowledgments

- [whisper.cpp](https://github.com/ggerganov/whisper.cpp) by Georgi Gerganov -- the inference engine that makes local transcription fast
- [whisper-rs](https://github.com/tazz4843/whisper-rs) -- Rust bindings for whisper.cpp
- [Tauri](https://tauri.app/) -- the framework powering the native app shell
- [OpenAI Whisper](https://github.com/openai/whisper) -- the original speech recognition model
- [KB (Kungliga biblioteket / National Library of Sweden)](https://www.kb.se/) -- Swedish-optimized [KB-Whisper](https://huggingface.co/KBLab) models (tiny, base, small, medium, large) by KBLab, used for Swedish transcription
- [NB (Nasjonalbiblioteket / National Library of Norway)](https://www.nb.no/) -- Norwegian-optimized [NB-Whisper](https://huggingface.co/NbAiLab) models (tiny, base, small, medium, large) by NbAiLab, used for Norwegian transcription
- [NbAiLab/NPSC](https://huggingface.co/datasets/NbAiLab/NPSC) -- Norwegian test audio (CC0, Norwegian National Library)

## License

[MIT](LICENSE)
