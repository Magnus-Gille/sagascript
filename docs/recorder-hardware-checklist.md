# Recorder: owner hardware checklist

What the automated tests cannot cover (SAGA-564). Tests never open a real microphone or a
real Trash, and a coding agent must never launch the app or `kill -9` it: these are for the
owner, on real machines. Build with `cargo tauri build --debug` (a signed or at least
bundled build: `cargo tauri dev` is unsigned and cannot show TCC behavior).

Tick each line per OS (macOS / Windows); note the build identity (tray menu) next to it.

## Permission and devices

- [ ] First start shows the microphone permission prompt once; Deny gives the
      `recording_permission_denied` message and a way to the system settings, not a hang.
- [ ] After granting, a session starts without relaunching the app.
- [ ] The default input device is the one that records.
- [ ] Headset or Bluetooth microphone: connecting it before and during a session. The profile
      switch (A2DP to HFP, 48 kHz to 16 kHz) ends the recording early with `device_changed` or
      `format_changed` and keeps everything recorded so far.
- [ ] Unplug the microphone mid-recording: the window shows the device loss, the recording is
      finished and playable up to that point, the tray indicator clears.
- [ ] Sleep and wake during a recording: the session ends early or shows a gap; no crash, no
      corrupt file.
- [ ] A multi-channel interface: the channel pair setting picks the channels; `[n, n]` records
      channel n as mono.

## Long runs

- [ ] A run of 2 hours or more: memory of the app stays flat (Activity Monitor / Task Manager
      before, during, after), the level meter and the 250 ms status keep updating, the file grows
      steadily, and stopping finishes in a few seconds.
- [ ] Transcribe the 2 h recording from the window: progress advances, memory is bounded by the
      one sample buffer (about 460 MB for 2 h), the text comes back.

## Playback

- [ ] FLAC playback and seeking of the mic track in the Record window on macOS (WKWebView) and on
      Windows (WebView2), with the fallback in place (`RECORDING_PLAYBACK_FALLBACK = true` in
      `src/lib/record-playback.ts`).
- [ ] If playback and seeking work through a `meeting-audio:`-style protocol: flip
      `RECORDING_PLAYBACK_FALLBACK` and serve the track the way `meeting_media.rs` serves
      meeting audio (a new protocol is a separate change; none exists for recordings today).

## Folders and volumes

- [ ] macOS default folder `~/Music/Sagascript`; Windows `Music\Sagascript`.
- [ ] Windows: a UNC path (`\\server\share\...`) shows the network-volume warning and records.
- [ ] Windows: a folder inside OneDrive shows the cloud-synced warning; macOS: iCloud Drive and
      other cloud-synced folders show it too.
- [ ] A FAT32 and an exFAT volume show their warnings (FAT32 has the 4 GB file limit that a long
      lossless recording reaches).
- [ ] Changing the folder in the window moves new recordings only; old ones stay listed from
      the old folder once it is selected again.
- [ ] Low disk: the `low_disk` warning appears and a full disk ends the recording with
      `recording_disk_full`, keeping what was saved.

## Delete, reveal, recover

- [ ] macOS: "Move to Trash" puts the folder in the Trash (Put Back works).
- [ ] Windows: it goes to the Recycle Bin (`IFileOperation`, see below); it can be restored; a
      path with non-ASCII characters works.
- [ ] Windows: the move goes through `IFileOperation` with `FOFX_RECYCLEONDELETE` and never
      deletes for good. It is refused up front (the folder is **still there** afterwards, the
      window says "It was not deleted") for each of: a mapped network drive, a UNC share and a USB
      stick (no Recycle Bin); a fixed disk with "Don't move files to the Recycle Bin" ticked in the
      bin's properties; a recording larger than the bin's maximum size (set a small maximum in the
      properties, or use a recording over 5 % of a small disk); a recording folder that cannot be
      listed (in the folder's Properties, Security, Advanced, give your user a Deny for "List
      folder / read data" on the recording folder only; denying read access to a single file
      does **not** do it, because a file's size is still readable through its directory entry).
      The folder's size cannot be read then, and the move must be refused with the folder
      untouched, never treated as "0 bytes". Remove the Deny afterwards. A bin setting that exists
      but cannot be read is the same refusal; it is hard to provoke on purpose (it needs a
      denied registry key under `...\Explorer\BitBucket`, which can upset Explorer), so it is
      covered by the unit test of the policy only. A normal fixed disk works and the folder can be restored from the bin.
- [ ] Windows: confirm that `FOFX_RECYCLEONDELETE` really fails instead of deleting when the
      up-front checks are bypassed (the checks read the bin's registry settings under
      `...\Explorer\BitBucket\Volume\{GUID}` and estimate a default quota of 5 % of the volume;
      neither is documented). If Windows asks "permanently delete?" at any point, answer No and
      report it: the registry layout or the estimate is wrong for that setup. If the flag alone
      fails closed in every case above, `FOF_WANTNUKEWARNING` in `src/trash.rs` can go (it stays
      until then: answering Yes to its prompt would delete for good).
- [ ] Windows: moving a recording to the Recycle Bin while the lock is held works. The app holds
      `recording.lock` open during the move; if the shell refuses to move a folder with an open
      file, the delete fails with an error and the folder stays: report it, the lock handling
      then has to change for Windows.
- [ ] A recording another process is writing is not deleted: start `sagascript record` in the
      folder, then "Move to Trash" in the app, and `sagascript recordings delete --yes`: both
      are refused with "in use by another process".
- [ ] Reveal opens Finder with the folder selected / Explorer with `/select`.
- [ ] Recovery after `kill -9` of the app during a recording (do it yourself, from a terminal;
      Task Manager "End task" on Windows): restart the app. The window shows the recovered note,
      the recording is playable, only an unfinished tail is set aside. A recording that is
      locked by a second Sagascript process (CLI recording) is left alone.

## Indicator and wording

- [ ] While recording, the tray shows `●` and "Sagascript - Recording audio..." (menu text and
      tooltip), also with the dictation hotkey unavailable (then "(hotkey unavailable)" is
      appended), and it clears when the recording stops, fails or ends by itself.
- [ ] The dictation hotkey does nothing while recording (it is logged), and the recorder
      refuses to start while dictating.
- [ ] First use: the folder confirmation and the consent reminder ("ask before recording other
      people") appear once each and read correctly; they do not return after being answered.
- [ ] The Record window opens from the tray ("Record…") at about 460 x 680, reopens to the same
      window, and fits a small screen.

## Left over from phases 1a and 1b (platform code that could not run here)

Windows code below compiles for `x86_64-pc-windows-msvc` but has never run; the macOS items need
a real device.

- [ ] Windows, hard links and link swaps: a hard link to a track file planted in a recording
      folder is **not detected on Windows** (`recording/io.rs` checks `st_nlink` on Unix only;
      detecting it needs another `windows-sys` feature). Plant one, start a recovery of that
      folder, and see what it does to the link's other name; decide whether the feature is worth
      adding. A symbolic link or junction swapped in for a track, journal or manifest between
      the check and the open must be refused (`recording_not_regular_file`), never followed.
- [ ] Windows, device identity: Windows devices are told apart by **name** only. With two
      identical USB microphones (same name), switching the default input from one to the other
      mid-recording should end the track with `device_changed`; if it does not, record that as
      a known limitation of name-only identity.
- [ ] Windows, sleep and volumes: a short sleep (under a minute) and a long one during a
      recording are noticed (the `QueryInterruptTime` clock pair; Windows 10 or later) and show as
      a gap or an early end. The volume classification names FAT32, exFAT, network and OneDrive
      folders correctly (`recording/session/win_volume.rs`), including a mapped drive and a UNC
      path.
- [ ] macOS, device identity on a real microphone: the CoreAudio device id is read from
      `cpal`'s `Debug` text (fragile across `cpal` versions). With the built-in microphone, a USB
      microphone and two identical USB microphones, switching the default input mid-recording
      ends the track with `device_changed`; an id that cannot be read must fall back to the name
      without a false "device changed" on every start.
- [ ] macOS, the Trash move while the app holds `recording.lock`: Put Back works afterwards and
      the folder is complete (the lock file inside it is harmless).
- [ ] Both: `sagascript record --transcribe` with a real model on a short real recording prints
      the text (the happy path was never run against a model), and Ctrl+C or `kill -TERM` during
      the first second of `sagascript record` (the source is still starting) still finishes and
      saves the recording.
- [ ] Both: FLAC playback in the web view (see Playback) decides `RECORDING_PLAYBACK_FALLBACK`.
