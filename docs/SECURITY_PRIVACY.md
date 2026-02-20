# Security & Privacy — Sagascript

## 1. Threat Model

### Assets to Protect

| Asset | Sensitivity | Protection Priority |
|-------|-------------|---------------------|
| Voice audio data | High | Audio contains sensitive conversations |
| Transcribed text | Medium | May contain personal/business info |
| API keys | High | Could be abused for unauthorized API usage |
| User preferences | Low | Convenience data only |

### Threat Actors

| Actor | Capability | Motivation |
|-------|------------|------------|
| Malicious app on same machine | Process inspection, clipboard access | Data theft |
| Network eavesdropper | Traffic interception | Data theft |
| Malware | Full system access if installed | Credential theft |
| Curious family member | Physical access | Accidental exposure |

### Attack Vectors & Mitigations

| Vector | Risk | Mitigation |
|--------|------|------------|
| Audio persistence on disk | Data exfiltration | Never write audio to disk |
| API key in plaintext | Credential theft | Store in Keychain only |
| HTTP traffic interception | Data in transit theft | TLS only, no HTTP fallback |
| Clipboard snooping | Text exposure | Brief clipboard use; warn user |
| Memory dumping | Audio/key extraction | Memory freed after use; not encrypted in RAM |
| Debug logging | Accidental exposure | No sensitive data in logs; debug disabled in release |

## 2. Data Retention

### Audio Data
```
┌──────────────┐     ┌───────────────┐     ┌────────────────┐
│ Microphone   │────▶│ Memory Buffer │────▶│ Transcription  │
│              │     │ (transient)   │     │ Engine         │
└──────────────┘     └───────────────┘     └────────────────┘
                            │                       │
                            │ Discarded after       │ Discarded after
                            │ processing            │ inference
                            ▼                       ▼
                     ┌─────────────────────────────────┐
                     │    Audio never written to disk  │
                     └─────────────────────────────────┘
```

- **Recording phase**: Audio samples held in ring buffer (~30s max)
- **After stop**: Audio passed to transcription engine
- **After transcription**: Audio data deallocated
- **Disk writes**: NEVER (unless user explicitly enables future logging feature)

### Transcribed Text
- Copied to clipboard briefly for paste
- Not persisted by app
- Clipboard contents controlled by macOS

### User Preferences
- Stored in UserDefaults (standard macOS app preferences)
- Contains: hotkey config, language, backend choice
- Does NOT contain: API keys, audio, transcripts

## 3. Remote Transcription

### When Disabled (Default)
- No network requests made
- All processing on-device
- Complete privacy

### When Enabled

```
┌──────────────┐     TLS 1.3      ┌─────────────────┐
│ Sagascript  │ ───────────────▶ │ api.openai.com  │
│              │                  │                 │
│ Audio data   │                  │ Transcription   │
│ (encrypted)  │ ◀─────────────── │ result          │
└──────────────┘                  └─────────────────┘
```

**User Consent Flow:**
1. User explicitly enables "Remote Transcription" in settings
2. Clear disclosure shown: "Audio will be sent to OpenAI for transcription"
3. User must enter API key
4. Consent persisted (can be revoked anytime)

**Data Sent to OpenAI:**
- Audio file (WAV/M4A format)
- Language hint (optional)
- API key in Authorization header

**Data NOT Sent:**
- Device identifiers
- User information
- Usage patterns

**OpenAI Data Handling:**
- Subject to [OpenAI API Data Usage Policies](https://openai.com/policies/api-data-usage-policies)
- API data NOT used for training by default (as of 2024)
- User should review OpenAI's current policies

## 4. API Key Security

### Storage
```swift
// Keychain storage implementation
let query: [String: Any] = [
    kSecClass as String: kSecClassGenericPassword,
    kSecAttrService as String: "com.sagascript.openai-api-key",
    kSecAttrAccount as String: "openai",
    kSecValueData as String: keyData,
    kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly
]
```

- Stored in macOS Keychain (encrypted at rest)
- Accessible only when device is unlocked
- Tied to this device only (not synced via iCloud Keychain)

### Key Handling Rules
1. **Entry**: Password field (masked), no echo to logs
2. **Storage**: Immediately to Keychain, never in memory longer than necessary
3. **Retrieval**: Only when making API request
4. **Display**: Never shown after entry (only "••••••••" placeholder)
5. **Logging**: NEVER logged, even in debug builds
6. **Transmission**: Only in Authorization header over TLS

### Key Rotation
- User can update key anytime in settings
- Old key deleted from Keychain when new key saved

## 5. Permissions

### Required Permissions

| Permission | Purpose | When Requested |
|------------|---------|----------------|
| Microphone | Audio capture for dictation | First dictation attempt |
| Accessibility | Simulate Cmd+V to paste text | First paste attempt |

### Microphone Permission
- Required for core functionality
- If denied: App cannot function; show guidance to enable in System Settings
- Revocable by user at any time

### Accessibility Permission
- Required for automatic paste into any app
- If denied: Text copied to clipboard only; user must manually Cmd+V
- App should handle gracefully and inform user

### Permissions NOT Required
- Full Disk Access
- Location
- Contacts
- Camera
- Network (for local mode)

## 6. Secure Coding Practices

### Input Validation
- Audio data validated for format before processing
- API responses validated for expected structure
- No user-provided data executed as code

### Memory Safety
- Swift's memory safety by default
- No unsafe pointers except where required by C APIs (AVFoundation, Keychain)
- Audio buffers explicitly cleared after use

### Dependency Security
- Dependencies reviewed for security advisories
- Dependabot enabled for security updates
- Minimal dependency footprint

### Build Security
- Release builds strip debug symbols
- No debug logging in release
- Code signing with hardened runtime (when signing available)

## 7. Incident Response

### If Security Issue Discovered
1. Create private security advisory on GitHub
2. Assess severity (CVSS scoring)
3. Develop fix on private branch
4. Release patch update
5. Disclose after fix available

### User Notification
- Critical issues: Notify via GitHub release notes
- App can check for critical update notices (future feature)

## 8. Privacy by Design Checklist

- [x] Local-first architecture
- [x] No data collection by default
- [x] Explicit consent for remote features
- [x] Minimal permission requests
- [x] No persistent audio storage
- [x] Secure credential storage
- [x] TLS-only network communication
- [x] No third-party analytics/tracking
- [x] Clear privacy disclosures in UI
