# Sagascript

[![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/Magnus-Gille/sagascript/actions/workflows/ci.yml/badge.svg)](https://github.com/Magnus-Gille/sagascript/actions/workflows/ci.yml)

Dictate anywhere. Privately. A lightweight menu bar app for macOS. Press a
hotkey, speak, and text appears in any application. Audio and transcripts stay
on your Mac. Sagascript connects to the internet only when you choose a
language that needs its speech engine, download another model, or check for an
update.

## Features

- **Push-to-talk dictation** -- hold a global hotkey, speak, release to transcribe and paste into any app
- **Local transcription** -- audio and transcripts are processed on-device with Metal/Core ML; they are not uploaded
- **Nordic-grade accuracy** -- Swedish and Norwegian use [KB-Whisper](https://huggingface.co/KBLab) (Swedish National Library) and [NB-Whisper](https://huggingface.co/NbAiLab) (Norwegian National Library), fine-tuned on 50,000+ hours of Nordic speech with 47% fewer errors than generic Whisper
- **Privacy by default** -- no telemetry, cloud transcription, or transcript upload; network access is limited to explicit speech-engine/model downloads and update checks
- **No telemetry or tracking** -- no analytics, no usage sharing, no data collection of any kind
- **Multi-language** -- English, Swedish, and Norwegian with dedicated models; additional languages supported via generic Whisper models
- **Language shortcuts** -- assign different global hotkeys to different languages and switch without opening Settings
- **CLI + GUI** -- full CLI for scripting and automation, menu bar app for everyday use
- **File transcription** -- transcribe audio and video files (MP3, WAV, M4A, FLAC, MP4, MKV, OGG, and more)
- **Configurable** -- choose your model, language, hotkey, and output behavior
- **macOS v1** -- official releases are signed and notarized for macOS 13+ on Apple Silicon; Intel Macs are not supported by the v1 binary release
- **Windows preview** -- the Windows port remains available for build-from-source testing; no official Windows binaries are published yet

## Building from source

### Prerequisites

- **macOS**: macOS 13.0+ on Apple Silicon (Intel Macs are not supported by the v1 binary release)
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

# Load the model once and transcribe several files or a directory
sagascript transcribe one.wav two.mp3 recordings/ --recursive

# Stream one result or error per source (JSON Lines)
sagascript transcribe recordings/ --recursive --jsonl

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
sagascript config path

# Manage the external personal dictionary
sagascript glossary path
sagascript glossary add OpenRouter --alias 'open router'

# Use one shortcut for English and another for Swedish
sagascript config profiles create swedish --name Swedish --hotkey 'Option+Space' --language sv
sagascript config profiles list

# Generate shell completions
sagascript completions zsh > ~/.zfunc/_sagascript

# Generate man pages
sagascript manpages --dir /usr/local/share/man/man1
```

Run `sagascript --help` for the full list of commands.

Default CLI diagnostics retain Sagascript warnings and errors while suppressing
routine native Whisper/GGML chatter, so machine-readable stdout (including
`--json`) remains safe to capture. To opt in to verbose native diagnostics for
troubleshooting, set an explicit filter, for example
`RUST_LOG=whisper_rs=info sagascript transcribe recording.mp3`.

For files of at least one minute, `--language auto` samples up to 60
speech-rich windows for sustained language changes. JSON includes
`language_regions` with language probabilities and a `mixed_language_audio`
warning when two supported languages remain stable across multiple windows.
The v1 behavior warns instead of silently switching the decoder; split the
recording or transcribe each part with an explicit language for best accuracy.
Explicit `en`, `sv`, and `no` keep the single-language fast path. Batch mode
runs language detection, VAD, repetition checks, and diagnostics
independently for every source file while loading the selected model only once.

Batch directory discovery accepts WAV, MP3, M4A, AAC, MP4, MOV, QTA, OGG,
WebM, and FLAC (case-insensitive), sorted by path. Explicit inputs retain their
given order and duplicates are processed once. By default an invalid or corrupt
item is reported while later items continue; the command still exits non-zero.
Use `--fail-fast` to stop immediately. For machine consumers, multi-input
`--json` returns an array of `{source,status,result|error}` objects and
`--jsonl` emits the same objects one per line. Single-file `--json` retains its
existing object shape.

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
- [Windows release track](docs/windows-release.md) -- unsigned internal candidates, zero-cost Store path, and acceptance gates
- [Configuration files](docs/configuration.md) -- XDG paths, dotfiles, migration, and personal dictionaries
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
