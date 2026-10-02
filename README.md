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
- **macOS v1** -- official releases are signed and notarized for macOS 13+ on Apple Silicon (Pianissimo requires macOS 14+); Intel Macs are not supported by the v1 binary release
- **Windows beta** -- an unsigned Windows 11 preview for x64 and ARM64 is available from the [GitHub prerelease](https://github.com/Magnus-Gille/sagascript/releases/tag/windows-beta-20260905)

## Pianissimo (Swedish, Core ML / ONNX)

Pianissimo runs on the Apple Neural Engine through a Core ML engine host bundled
in the app (`Sagascript.app/Contents/Resources/EngineHost/sagascript-engine-host`).
On Windows on ARM (Snapdragon) it runs on the CPU through an ONNX Runtime host
bundled in `engine-host\` beside the app. It requires **macOS 14+ on Apple Silicon
or Windows on ARM (Snapdragon)**. In **Dictate**, select
**Pianissimo** for a Swedish profile, then download its model if
prompted. Each profile owns its language, shortcut, dictionary, and model;
another Swedish profile can keep Whisper. The file-transcription choice stays
independent. No Python installation is needed.

CLI equivalents:

```bash
sagascript download-model pianissimo-sv
sagascript record --language sv --model pianissimo-sv
sagascript config profiles update default --language sv --model pianissimo-sv
# Restore the Swedish recommended Whisper model for this profile:
sagascript config profiles update default --model auto
# Inspect or check the engine host end to end:
sagascript engine status --json
sagascript engine doctor
```

The engine host starts on demand, stays loaded while you dictate, and unloads
after an idle period (configurable in Settings). It supports dictionary
replacements after transcription, but not Whisper decoder hints. Explicit
`--hint`/`--hint-file` options are rejected with this model. File **Auto** uses
the language's recommended model (Pianissimo for Swedish where supported) when it
is downloaded, otherwise the best compatible model you already have.

### Test results (Swedish, Mac)

What we measured, how, and on which build. Full details and raw data:
[benchmark report](docs/benchmarks/pianissimo-coreml-2026-09.md).

| | |
| --- | --- |
| **Model under test** | Klang AI's Pianissimo weights, **our own Core ML conversion** [`magnusgille/pianissimo-sv-coreml`](https://huggingface.co/magnusgille/pianissimo-sv-coreml) r1 (`6e33b64e`): int8 encoder weights, fp16 compute, fixed 15 s windows with 2 s overlap. This is not an official Klang AI build |
| **Runtime** | Sagascript's Swift Core ML engine host on the Apple Neural Engine, in Sagascript **1.4.0** (`af1f5af`, the published, signed release; 1.4.1 has the same Mac engine) |
| **Test audio** | [FLEURS](https://huggingface.co/datasets/google/fleurs) Swedish test split (`sv_se`, CC BY 4.0): the first reading of each distinct sentence, **307 read sentences by many speakers**, joined with 0.5 s of silence into **one 60 min 11 s file** (6,293 reference words). It is one long file, **not a continuous conversation** |
| **Command** | `sagascript transcribe --model pianissimo-sv --language sv fleurs-sv-distinct-60min.wav` |
| **Timing** | Wall time of the whole command, process start to text (model already downloaded and compiled), 2 runs |
| **Accuracy** | Word error rate (WER) against the FLEURS reference text: lowercase, punctuation removed, whitespace collapsed, no number normalization (the same normalization Klang AI uses) |
| **Hardware** | MacBook Air 13" M4 (base, 10-core CPU, 16-core Neural Engine, 32 GB), fanless, on AC power, macOS 27.0 |
| **Date** | 2026-09-30 |

| Model and runtime (same 60 min file) | Time | WER |
| --- | ---: | ---: |
| **Pianissimo, our Core ML conversion r1, Neural Engine** (Sagascript 1.4.0) | **11.2–11.8 s** | **7.06%** |
| Pianissimo, 8-bit GGUF on the CPU via NeMo-Speech.cpp (Sagascript 1.3.2) | 174 s | 6.67% |
| Pianissimo, Klang AI's official MLX 8-bit build on the GPU (outside Sagascript; transcription time only, model load excluded) | 106 s | 6.53% |
| KB-Whisper Large via whisper.cpp (Sagascript 1.4) | about 12 min (estimated from a 15 min run: 174 s, 5.99% WER) | – |

### Questions about these results

#### Does accuracy drop on long recordings?

No, neither on FLEURS nor on a real 73-minute parliament debate (see *How does it do on a real, continuous one-hour recording?*). On FLEURS, WER is 6.6% when the 759 FLEURS test clips are
transcribed one by one, and 7.0%, 6.6%, 6.1% and 7.1% on the 5, 15, 30 and
60 min files. Sagascript cuts long audio into overlapping 15 s windows and stitches
the text back together, so the model never has to handle an hour at once. Klang
AI describes an extra fine-tune for long recordings in its training write-up. See the next question for what this set does and does not cover.

#### Is the test data a continuous hour of speech?

No. It is **one 60-minute file** made of **307 separate read sentences** (about
11 s each) by many different speakers, taken from Wikipedia-style text and joined
with 0.5 s of silence. It shows that a full hour goes through without dropped or
invented passages (6,262 words out against 6,293 in the reference; the longest
run of dropped words is 2). It is **not** a continuous conversation: there is no
long monologue, no interruptions, no overlapping speakers and no background
noise, so real meetings will score worse. For a real continuous recording, see the parliament debate below.

#### How does it do on a real, continuous one-hour recording?

We ran a real Riksdag (Swedish parliament) debate: *Skyldighet att betala för
tandvård – nya regler för vissa utlänningar* (SoU40, 12 Aug 2026, protocol
HD09156, speeches 172–192). It is **73 minutes of continuous speech** with 21
speeches, 7 speakers and back-and-forth replies, from Riksdagen's open data
([audio](https://mhdownload.riksdagen.se/VOD1/HD/2442608120057218521_aud.mp3),
SHA-256 `2cba7817151c53038413bfd8eb502374720685b6d2f0deb3a0c0ab93b0c5ace6`). The
reference is the **official protocol** (8,553 words). The protocol is edited: the
Speaker's "thank you, the floor goes to …" and words like *och*, *så*, *ju* and
repetitions are left out, although they are in the audio. Every engine that writes
them down is charged for them, so the total WER is inflated. The fairer column
counts only substituted and dropped words.

| Model and runtime (Sagascript 1.4.1 on a MacBook Air M4, 2026-10-01) | Time | WER, total | Substituted + dropped only | Names right |
| --- | ---: | ---: | ---: | ---: |
| **Pianissimo, our Core ML conversion r1, Neural Engine** | **14.6 s** | 23.4% | 10.9% | 93% |
| Pianissimo, 30 s-window candidate (not released; engine host only) | 21.6 s | 24.6% | 11.5% | 92% |
| KB-Whisper Large via whisper.cpp | about 25 min | 19.7% | 11.7% | 98.5% |
| Pianissimo, Klang AI's MLX 8-bit build (outside Sagascript) | 152 s | 24.7% | 10.6% | 92.5% |

**No decline over the hour.** WER by 10-minute block for Pianissimo r1: 26.8,
33.7, 23.4, 28.3, 19.3, 16.6, 17.2 and 20.7% (minutes 0–73). The other engines
follow the same pattern, worst around minutes 10–20 and best around 50–70. That
tracks the speakers and the content, not how far into the file the model is. The
longest run of dropped words was 8 for Pianissimo and 22 for KB-Whisper, which
skipped five short passages
([#273](https://github.com/Magnus-Gille/sagascript/issues/273)).

KB-Whisper's lower total WER comes mostly from leaving out fillers, as the
protocol does. On substituted plus dropped words the four engines are within one
point of each other (10.6–11.7%). KB-Whisper is clearly better on names here
(98.5% against about 93%). The 30 s-window candidate, which was better on FLEURS,
was not better on this debate.

Caveats: one debate, one run per engine, an edited reference, and the machine was
partly loaded during the KB-Whisper and MLX runs, so those two times are rough.

#### Are names transcribed correctly?

Less often than ordinary words, and WER hides this because names are a small
share of the words. We count a name as any capitalised word in the reference that
does not start a sentence (Swedish capitalises little else), and as correct only
if the same spelling appears in the transcript, ignoring case. No personal
dictionary, decoder hints or replacements were used.

| Model and runtime | 15 min file (76 names) | 60 min file (272 names) |
| --- | ---: | ---: |
| Pianissimo, our Core ML conversion r1 (Sagascript 1.4.1 for 15 min, 1.4.0 for 60 min; same Mac engine) | 75% (57) | 67% (182) |
| Pianissimo, Klang AI's MLX 8-bit build | 74% (56) | 69% (187) |
| KB-Whisper Large via whisper.cpp (Sagascript 1.4.1) | 78% (59) | not run |

About 93% of all words are right, but only about 70% of names. The engines differ
by one to three names on the 15 min file, which is within noise for a sample this
small. Typical misses are foreign or rare names (Hsien Loong, Oravec, Fernández)
and Swedish compounds written differently (Falklandspundet). With Pianissimo,
your personal dictionary still applies its whole-word replacements after
transcription, but Pianissimo cannot take Whisper-style decoder hints. We have
not yet measured what the dictionary adds; that needs a set of names outside the
training data, tested with and without the dictionary. Counting script:
[`docs/benchmarks/data/names-20260930/names.py`](docs/benchmarks/data/names-20260930/names.py).

#### How do these numbers compare with Klang AI's published ones?

They measure different things. Klang AI reports 6.5% WER on FLEURS Swedish with
each clip transcribed separately; transcribed the same way, our Core ML build
scores 6.56% (759 clips). Our long-form numbers come from the joined files above.
Klang's speed figure (about 2,500× real time) is a server GPU (NVIDIA A100)
running 128 clips at once; ours is one file at a time on a laptop, start to text,
which is what dictation and file transcription actually do. See Klang AI's
[How we trained Pianissimo](https://research.klang.ai/papers/how-we-trained-pianissimo/).

#### Is the speed real, or the result of unusual settings?

The speed comes from running the model on the Mac's Neural Engine instead of the
GPU or CPU. The weights are Klang AI's. All engines ran the same files on the same
machine, our time includes starting the program and loading the model (the MLX
time does not), and the whole transcript is checked against the reference. The
one trade-off is the fixed 15 s window: the model was trained to look about 20 s
in each direction, so short windows give it less context and cost about one extra
wrong word in 280 compared with Klang's MLX build. Longer 30 s windows that keep
most of the speed (about 1.35× the time) and beat the MLX build's accuracy are
being prepared for a later release ([#268](https://github.com/Magnus-Gille/sagascript/issues/268)).

#### How fast is it on Windows?

On a Snapdragon X Elite laptop (Windows 11 on ARM, plugged in), Pianissimo runs on
the CPU through ONNX Runtime with Klang AI's official ONNX files: a 19.5-minute
Swedish radio programme took 82 s (Sagascript 1.4.1), and dictated text arrived
0.35–0.46 s after releasing the key (pre-release build with the same engine). On battery at low charge, Windows slowed
dictation to 6–8 s. These are single runs on one machine, without a reference text
for WER.

#### Limits of all numbers above

FLEURS is clean read speech with pauses between sentences. One real
parliament debate is covered above; meetings with overlapping speakers and
background noise are harder and are not covered. Mac numbers are from one base MacBook Air M4; other hardware was
not measured.

## Building from source

### Prerequisites

- **macOS**: macOS 13.0+ on Apple Silicon (Pianissimo needs 14.0+; Intel Macs are not supported by the v1 binary release)
- **Windows beta**: Windows 11 on x64 or ARM64; unsigned preview, not an official stable release
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
Source builds can also produce Windows or experimental Linux packages. The
downloadable Windows beta is documented in the platform notes below.

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

# Manage the default profile's external personal dictionary
sagascript glossary path
sagascript glossary add OpenRouter

# Use one shortcut for English and another for Swedish
sagascript config profiles create swedish --name Swedish --hotkey 'Option+Space' --language sv --model kb-whisper-base
sagascript config profiles list
# Enable deterministic aliases only in the explicit-language profile
sagascript glossary add OpenRouter --alias 'open router' --profile swedish

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

Press Ctrl-C (or send SIGTERM) to cancel a running `transcribe`. Whisper
decoding (including parallel chunks and the gap re-decode pass) and Pianissimo
requests stop within about a second, the engine host is shut down, and the
command prints `Cancelled` and exits with status 130. Cancelling a batch stops
all remaining files and discards the interrupted file's partial result; results
already printed stay printed, and no transcript is written for the cancelled
file. A second Ctrl-C force-quits immediately. The next run starts normally.

## Permissions

### macOS

Sagascript needs the following permissions (macOS will prompt you on first use):

- **Microphone** -- for recording audio
- **Accessibility** -- for pasting transcriptions into the active app and for
  bare F13–F24 shortcuts
Official macOS releases are Developer ID signed and notarized. If a downloaded
release asks you to bypass Gatekeeper, do not run it; report the artifact.

### Windows

The [Windows beta](https://github.com/Magnus-Gille/sagascript/releases/tag/windows-beta-20260905)
is an unsigned preview for Windows 11 on x64 and ARM64. It needs microphone
access for recording audio. Verify the release checksums before running it, and
do not bypass SmartScreen.

## Documentation

- [Installation guide](docs/installation.md) -- detailed install instructions for macOS and Windows
- [Linux notes](docs/linux-notes.md) -- experimental Linux build, prerequisites, and known limitations
- [Windows-specific notes](docs/windows-notes.md) -- feature comparison, known limitations, and troubleshooting
- [Windows release track](docs/windows-release.md) -- unsigned beta distribution, verification record, and stable-release gates
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
