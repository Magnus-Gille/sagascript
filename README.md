# Sagascript

[![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/Magnus-Gille/sagascript/actions/workflows/ci.yml/badge.svg)](https://github.com/Magnus-Gille/sagascript/actions/workflows/ci.yml)

Low-latency, privacy-first dictation for macOS. Push-to-talk transcription with local Whisper models.

![Sagascript](sagascript-screenshot.png)

## Features

- **Push-to-talk dictation** -- hold a global hotkey, speak, release to transcribe and paste
- **100% local transcription** -- runs Whisper models on-device via Metal/Core ML (no data leaves your Mac)
- **Multi-language** -- English, Swedish, Norwegian, and 90+ other languages
- **CLI + GUI** -- full CLI for scripting and automation, menu bar app for everyday use
- **File transcription** -- transcribe audio and video files (MP3, WAV, M4A, FLAC, MP4, MKV, OGG, and more)
- **Configurable** -- choose your model, language, hotkey, and output behavior

## Building from source

### Prerequisites

- macOS 13.0+
- Rust 1.75+ (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
- Node.js 20+ (`brew install node`)
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

The `.app` bundle will be in `src-tauri/target/release/bundle/macos/`.

## CLI usage

Sagascript includes a full CLI. After building, the binary is at `src-tauri/target/release/sagascript` (or use the app bundle).

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

## macOS permissions

Sagascript needs the following permissions (macOS will prompt you on first use):

- **Microphone** -- for recording audio
- **Accessibility** -- for pasting transcriptions into the active app
- **Input Monitoring** -- for the global hotkey

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, code style, and how to submit changes.

## Acknowledgments

- [whisper.cpp](https://github.com/ggerganov/whisper.cpp) by Georgi Gerganov -- the inference engine that makes local transcription fast
- [whisper-rs](https://github.com/tazz4843/whisper-rs) -- Rust bindings for whisper.cpp
- [Tauri](https://tauri.app/) -- the framework powering the native app shell
- [OpenAI Whisper](https://github.com/openai/whisper) -- the original speech recognition model
- [NbAiLab/NPSC](https://huggingface.co/datasets/NbAiLab/NPSC) -- Norwegian test audio (CC0, Norwegian National Library)

## License

[MIT](LICENSE)
