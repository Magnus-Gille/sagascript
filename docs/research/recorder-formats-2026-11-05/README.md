# Recorder formats and streaming FLAC: feasibility study

- Ticket: SAGA-547 (record audio directly in Sagascript: mic or mic + system audio, optional lossless)
- Date: 2026-11-05, base `dc6ef732` (origin/main when the worktree was created)
- Status: study only. No product code was changed and no audio was captured from any device.
- Design built on these results: [docs/recorder-design.md](../../recorder-design.md), ADR
  [0009](../../adr/0009-recording-storage.md) (Accepted 2026-11-06).

## Question

What can Sagascript write when a user records audio and keeps it (microphone, or microphone plus
system audio; lossless or compact), so that it works on macOS now, can follow on Windows and the
iPhone, survives a crash during a multi-hour recording, and needs few new dependencies?

## Findings

1. **FLAC is the right lossless format.** About 57 % of a 24-bit WAV on a speech signal with a
   realistic noise floor, bit-exact round trip through four independent implementations, and
   macOS can encode and decode it natively.
2. **A pure-Rust encoder is enough.** `flacenc` 0.5.1 (Apache-2.0) encodes frame by frame, so a
   recording never needs to be held in memory: one hour of 48 kHz / 24-bit audio took 11.7 s
   (about 300x realtime) and a peak of 25 MB. Output verified bit-exact with `claxon`, `libFLAC`
   (`flac -t`, which checks the stored MD5) and CoreAudio.
3. **FLAC survives a crash by construction.** A file cut at 60 % plays to the cut in CoreAudio,
   ffmpeg and `claxon`; only the STREAMINFO totals and MD5 are missing, and a recovery pass can
   restore them.
4. **M4A (ALAC and AAC) does not survive a crash** in CoreAudio when the process dies before the
   file is finalized. A WAV whose header sizes were left at 0 fails the same way; a WAV written
   with `0xFFFFFFFF` sizes plays. Classic WAV also stops at 4 GiB (4.1 h for two 48 kHz / 24-bit
   tracks in one file).
5. **"Compact" does not need a lossy codec for v1.** FLAC at 16 kHz / 16-bit (the transcription
   resolution) is 46 MB per hour against 295 MB per hour for 48 kHz / 24-bit. AAC-LC at 64 kbit/s
   is 30 MB per hour but needs platform encoders, a crash-safe container strategy and a licence
   review.
6. **Both draft system-audio PRs store 16 kHz stereo WAV**, which is the transcription format and
   not an original worth keeping. Their capture code can be reused; their storage cannot.

## Method and environment

| Item | Value |
|---|---|
| Machine | Apple Silicon (arm64), macOS 27.2 |
| Tools | flac 1.5.0, ffmpeg 9.0.2 (libopus), sox, afconvert (AudioToolbox), rustc 1.99.0 |
| Signal | `test-audio/diarization/dj_2022_feu.wav`, 58.6 s of Swedish speech (fetched by `test-audio/diarization/fetch.sh`, git-ignored), converted to 48 kHz / 24-bit mono and mixed with a -54 dBFS pink-noise floor |
| Caveat | The speech is band-limited at 8 kHz. A real 48 kHz microphone has wideband noise, so real FLAC sizes will be somewhat larger than measured here. Sizes per hour are indicative. |
| Timings | One run each, process start included; read them as orders of magnitude. |
| Spike | Throwaway crate outside the repository (`flacenc`, `hound`, `claxon`). All build scripts in the dependency tree were read first: they only run `rustc --version` or write build info. Nothing was installed into the repository or its `Cargo.lock`. |

Reproduce with `measure-formats.sh` and `run-flac-spike.sh` (usage in each header). The spike source is
kept as `flac-spike.main.rs.txt` and `flac-spike.Cargo.toml.txt` (renamed to `.txt` so they are not
picked up as Rust sources or a Cargo package; copy them to `Cargo.toml` and `src/main.rs` in a scratch
directory to build).

## Results

### 1. Size and encode speed (58.6 s, 48 kHz / 24-bit mono)

| Format and encoder | Size | vs WAV | Encode time |
|---|---:|---:|---:|
| WAV PCM 24-bit | 8.44 MB | 100 % | - |
| FLAC, libFLAC `-0` / `-5` / `-8` | 4.91 / 4.82 / 4.81 MB | 58.1 / 57.0 / 57.0 % | 0.08-0.24 s |
| FLAC, ffmpeg | 4.82 MB | 57.1 % | 0.16 s |
| FLAC, AudioToolbox (`afconvert`) | 4.81 MB | 56.9 % | 0.24 s |
| ALAC, AudioToolbox | 4.85 MB | 57.4 % | 0.21 s |
| AAC-LC 128 / 64 kbit/s, AudioToolbox | 0.97 / 0.49 MB | 11.4 / 5.7 % | 0.3 s |
| Opus 64 / 32 kbit/s, ffmpeg libopus | 0.42 / 0.21 MB | 5.0 / 2.5 % | 0.7-1.0 s |

### 2. Lossless round trip

The raw 24-bit PCM MD5 of the decoded file equals the source for: libFLAC encode and decode;
AudioToolbox FLAC and ALAC encode and decode; ffmpeg FLAC encode decoded by libFLAC; AudioToolbox
FLAC decoded by ffmpeg. So macOS has a native FLAC and ALAC encoder (`afconvert` is a front end to
AudioToolbox). Whether the same is true on iOS is not verified (see below).

### 3. A file cut at 60 % (what a crash leaves)

Decoded seconds out of 58.6; CoreAudio (through `afconvert`) stands in for the macOS system
decoders and is not a test of WKWebView or WebView2.

| File | ffmpeg | CoreAudio |
|---|---:|---|
| FLAC (libFLAC, AudioToolbox) | 34.9-35.0 s | 35.0 s |
| Ogg Opus | 33.9 s | 34.0 s |
| WAV, sizes `0xFFFFFFFF` (streamed) | 35.1 s | 35.2 s |
| WAV, sizes 0 (naive recorder) | 35.1 s | **fails** |
| M4A (ALAC) | 35.0 s | **fails** |
| M4A (AAC) | 34.2 s | **fails** |

### 4. `flacenc` streaming spike

Frame-by-frame encoding with `FrameBuf` + `Context` + `encode_fixed_size_frame`, a placeholder
STREAMINFO written first and rewritten at stop (total samples and MD5 set).

| Case | Result |
|---|---|
| Mono 58.6 s, streaming vs whole-stream API | identical size (4 813 327 B), 0 sample mismatches (`claxon`), `flac -t` OK, CoreAudio decode MD5 equals source |
| Stereo (second track = quieter delayed copy) | 0 mismatches over 5.6 M samples, `flac -t` OK, about 100-130x realtime |
| Killed at 60 %, header never fixed | `claxon` decodes all 1 688 371 written frames, CoreAudio 35.2 s, `flac -t` only warns that the MD5 is unset |
| 58 minutes streamed, mono | 11.7 s (about 300x realtime), **25 MB peak resident**, 289 MB output, `flac -t` OK (MD5 of the whole hour matches) |

The encoder runs far faster than realtime, so a writer thread needs well under 1 % of one core
per track. The memory is bounded because only one block is encoded at a time.

### 5. Size per hour of mono speech

| Tier | MB per hour |
|---|---:|
| FLAC 48 kHz / 24-bit (lossless) | 295 |
| FLAC 48 kHz / 16-bit | 123 |
| FLAC 24 kHz / 16-bit | 65 |
| FLAC 16 kHz / 16-bit | 46 |
| AAC-LC 64 / 32 kbit/s (M4A) | 30 / 15 |
| WAV 48 kHz / 24-bit | 518 |
| WAV 16 kHz / 16-bit (what `record -o` writes today) | 115 |

## Web survey (secondary; checked 2026-11-05 by a delegated research pass)

Confirmed by me: `flacenc` 0.5.1, Apache-2.0, pure Rust. The build scripts and licences of its full
dependency tree were read (all permissive and within the licences in `src-tauri/deny.toml`'s allow
list by my reading; `cargo deny` itself was not run). Apple lists `kAudioFormatFLAC` as available
from iOS 11.0.

Reported with sources but not independently confirmed:

- FLAC: 1-8 channels and 4-32 bits ([RFC 9639](https://www.rfc-editor.org/rfc/rfc9639.html));
  STREAMINFO total samples 0 means unknown. Plays in Chrome/Edge and in Safari 11+
  ([MDN audio codecs](https://developer.mozilla.org/en-US/docs/Web/Media/Guides/Formats/Audio_codecs)).
- ALAC: does not play in Chromium (MDN); the Rust `alac-encoder` crate is stale (2022).
- AAC: Windows Media Foundation has an AAC-LC encoder (mono/stereo, 44.1/48 kHz;
  [Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/medfound/aac-encoder));
  Fraunhofer `fdk-aac` bindings need a C library and a licence review. Patent position not checked.
- Opus: royalty-free per the IETF disclosures; Rust bindings need libopus (C); Ogg Opus in Safari
  is not confirmed, so it is a poor fit for WKWebView playback.

## Not verified

These need the installed app, a device or the owner's explicit approval to capture audio:

- FLAC playback in the app's own webview (WKWebView on macOS, WebView2 on Windows) through the
  `meeting-audio:` protocol.
- Whether `AVAudioFile` / `ExtAudioFile` can **encode** FLAC on iOS (the format identifier exists;
  encoding is untested). This belongs in the iPhone feasibility work (SAGA-532).
- A Windows on ARM64 build of `flacenc` (pure Rust, expected to be fine).
- Real microphone and system-audio compression ratios, and the sample format and rate that the
  macOS system-audio tap delivers.
- AAC patent and licence terms.
