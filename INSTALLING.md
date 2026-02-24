# Installing Sagascript

## macOS

Download **Sagascript.dmg** from the [latest release](https://github.com/Magnus-Gille/sagascript/releases/latest). This is a universal binary that runs natively on both Apple Silicon and Intel Macs.

### Unsigned build warning

Sagascript is not yet code-signed with an Apple Developer certificate. macOS will block the app the first time you open it.

**To open it:**

1. Open the DMG and drag Sagascript to Applications
2. Open Sagascript from Applications — macOS will show *"Sagascript can't be opened because Apple cannot check it for malicious software"*
3. Open **System Settings > Privacy & Security**, scroll down to the Security section
4. You'll see *"Sagascript was blocked from use because it is not from an identified developer"* — click **Open Anyway**
5. Confirm in the dialog that appears

Alternatively, run this in Terminal before the first launch:

```
xattr -cr /Applications/Sagascript.app
```

You only need to do this once. Subsequent launches will work normally.

## Windows

Download **Sagascript-Setup.exe** from the [latest release](https://github.com/Magnus-Gille/sagascript/releases/latest).

### SmartScreen warning

Sagascript is not yet signed with a Windows code-signing certificate. Windows Defender SmartScreen will show a warning on first run.

**To proceed:**

1. Run the installer — SmartScreen shows *"Windows protected your PC"*
2. Click **More info**
3. Click **Run anyway**

The warning only appears once per downloaded file.

## First launch

On first launch, Sagascript will walk you through setup:

1. **Language** — pick your primary dictation language (English, Swedish, or Norwegian)
2. **Speech engine download** — downloads the recommended Whisper model for your language (55-142 MB)
3. **Microphone permission** (macOS) — required for live dictation
4. **Accessibility permission** (macOS) — allows auto-paste into any app after dictation

All speech processing happens locally on your device. No audio is sent to any server.

## System requirements

- **macOS:** 13.0 (Ventura) or later, Apple Silicon or Intel
- **Windows:** 10 or later, WebView2 runtime (installed automatically)
