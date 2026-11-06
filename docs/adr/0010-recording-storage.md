# 0010. Recordings are stored as one FLAC file per track plus a manifest

- Status: Accepted
- Date: 2026-11-05 (proposed), 2026-11-06 (accepted by the owner)
- Related: SAGA-547, SAGA-533, [ADR 0001](0001-cli-first.md), [ADR 0002](0002-privacy-first.md),
  [ADR 0008](0008-publishing-new-files.md), [recorder design](../recorder-design.md)

## Context

Sagascript can only save audio as the 16 kHz mono WAV that transcription uses (`record -o`), and the
two unmerged system-audio drafts (#300, #521) also keep only 16 kHz stereo WAV. A recorder that lets
the user keep a recording (microphone, later system audio, lossless or compact) needs a stored format
that is faithful, survives a crash during a multi-hour recording, plays in the app's webview and the
OS, adds few dependencies, and can later describe an iPhone recording too (SAGA-533).

Measured on this Mac ([study](../research/recorder-formats-2026-11-05/README.md)): FLAC is about 57 %
of 24-bit WAV, round-trips bit-exactly through four implementations, and a file cut mid-write plays
up to the cut. M4A (ALAC and AAC) and a WAV with zero header sizes fail in CoreAudio after a cut.
Classic WAV stops at 4 GiB. The pure-Rust `flacenc` encodes frame by frame at about 300x realtime
with 25 MB peak memory over an hour.

## Decision

1. A recording is a **folder** holding one audio file per **track** and a `recording.json` manifest.
2. Track audio is **FLAC at the device's native sample rate**, never resampled, gain-adjusted or mixed
   on disk. The *compact* tier is also FLAC (16 kHz / 16-bit) until a new ADR adds another codec.
3. Audio is written **incrementally by a writer thread**, never on the audio callback. A placeholder
   STREAMINFO is rewritten at the end. A killed process leaves a playable file; the manifest says
   `state: "recording"` and recovery finalizes it without deleting audio.
4. The manifest names files relative to its folder (no absolute paths, ADR 0006), records per-track
   codec, rate, bit depth, sample count, hash, start time on the host clock and gap markers, and uses
   the vocabulary SAGA-533 needs so a phone recording can use the same description.
5. The folder is built as `<name>.part` and **published without replacing** (ADR 0008) when it is
   complete.
6. Alignment between tracks is metadata. Mixdown, stereo and 16 kHz audio are derived on demand and
   the output says so.

## Consequences

- One codec, one crash model and no codec licence question in v1; Opus and AAC are out of scope here.
- Users who want a single file use an export; two tracks never share one file on disk.
- The decoder gets a per-track API, because today it downmixes to mono.
- Real-microphone ratios, FLAC playback in WKWebView / WebView2 and iOS FLAC encoding are unverified
  (see the study). If AAC is wanted later, a crash-safe container (ADTS journal then remux, or
  fragmented MP4) is part of that decision.
- Accepted on 2026-11-06 with the owner's decisions D1 (compact tier is reduced FLAC) and D2 (one file
  per track plus a manifest) in the [design](../recorder-design.md#9-decisions). Which capture code
  leads system audio (D3) is deliberately not part of this ADR.
