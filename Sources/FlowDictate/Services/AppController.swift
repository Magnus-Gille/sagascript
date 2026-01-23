import Foundation
import SwiftUI
import Combine

/// Central coordinator for the app's dictation workflow
/// Manages state machine: Idle → Recording → Transcribing → Idle
@MainActor
final class AppController: ObservableObject {
    static let shared = AppController()

    // MARK: - Published State

    @Published private(set) var state: AppState = .idle
    @Published private(set) var lastTranscription: String?
    @Published private(set) var lastError: DictationError?
    @Published private(set) var isModelReady: Bool = false

    // MARK: - Services

    private let hotkeyService: HotkeyService
    private let audioCaptureService: AudioCaptureService
    private let transcriptionService: TranscriptionService
    private let pasteService: PasteService
    private let settingsManager: SettingsManager
    private let loggingService: LoggingService

    // MARK: - Overlay Window

    private var overlayWindow: RecordingOverlayWindow?

    // MARK: - Recording Timing

    private var recordingStartTime: Date?
    private var transcriptionStartTime: Date?
    private var currentAudioSamples: Int = 0
    private let minimumRecordingDuration: TimeInterval = 0.3 // 300ms minimum

    // MARK: - Computed Properties

    var isRecording: Bool { state.isRecording }
    var isTranscribing: Bool { state.isTranscribing }
    var isBusy: Bool { state.isBusy }

    // MARK: - Initialization

    private init() {
        self.loggingService = LoggingService.shared

        print("╔════════════════════════════════════════════════════════════╗")
        print("║              FlowDictate Starting Up...                    ║")
        print("╚════════════════════════════════════════════════════════════╝")
        print("")
        print("[AppController] Initializing services...")

        self.settingsManager = SettingsManager.shared
        print("[AppController] ✓ SettingsManager ready")

        self.hotkeyService = HotkeyService()
        print("[AppController] ✓ HotkeyService ready")

        self.audioCaptureService = AudioCaptureService()
        print("[AppController] ✓ AudioCaptureService ready")

        self.transcriptionService = TranscriptionService()
        print("[AppController] ✓ TranscriptionService ready")

        self.pasteService = PasteService()
        print("[AppController] ✓ PasteService ready")
        print("")

        loggingService.info(.App, LogEvent.App.started, data: [
            "appSessionId": AnyCodable(loggingService.appSessionId)
        ])

        setupHotkeyCallbacks()
        warmUpModel()
    }

    // MARK: - Setup

    private func setupHotkeyCallbacks() {
        hotkeyService.onKeyDown = { [weak self] in
            Task { @MainActor in
                self?.handleHotkeyDown()
            }
        }

        hotkeyService.onKeyUp = { [weak self] in
            Task { @MainActor in
                self?.handleHotkeyUp()
            }
        }

        // Delay hotkey registration slightly to avoid spurious triggers on startup
        // This gives the app time to fully initialize before listening for input
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in
            guard let self = self else { return }
            self.hotkeyService.register(
                keyCode: UInt32(self.settingsManager.hotkeyKeyCode),
                modifiers: UInt32(self.settingsManager.hotkeyModifiers)
            )
            print("[AppController] Hotkey registered: \(self.settingsManager.hotkeyDescription)")
        }
    }

    private func warmUpModel() {
        print("[AppController] Starting model warm-up (this may take a while on first run)...")
        print("[AppController] The model needs to be downloaded (~50-150MB) on first launch.")
        print("")

        Task {
            do {
                try await transcriptionService.warmUp()
                isModelReady = true
                let hotkeyDesc = settingsManager.hotkeyDescription
                print("")
                print("╔════════════════════════════════════════════════════════════╗")
                print("║       FlowDictate Ready! Press \(hotkeyDesc.padding(toLength: 28, withPad: " ", startingAt: 0))║")
                print("╚════════════════════════════════════════════════════════════╝")
                print("")

                loggingService.info(.App, LogEvent.App.ready, data: [
                    "hotkeyDescription": AnyCodable(hotkeyDesc)
                ])
            } catch {
                print("[AppController] ✗ Failed to warm up model: \(error)")
                print("[AppController] You can still try dictating - it will attempt to load on demand.")
            }
        }
    }

    // MARK: - Hotkey Handlers

    private func handleHotkeyDown() {
        print("[Hotkey] ⬇️  Key DOWN detected")
        switch settingsManager.hotkeyMode {
        case .pushToTalk:
            print("[Hotkey] Mode: Push-to-talk → Starting recording...")
            startRecording()
        case .toggle:
            if state.isRecording {
                print("[Hotkey] Mode: Toggle → Stopping recording...")
                stopRecordingAndTranscribe()
            } else if state == .idle {
                print("[Hotkey] Mode: Toggle → Starting recording...")
                startRecording()
            }
        }
    }

    private func handleHotkeyUp() {
        print("[Hotkey] ⬆️  Key UP detected")
        guard settingsManager.hotkeyMode == .pushToTalk else {
            print("[Hotkey] Mode is Toggle, ignoring key up")
            return
        }
        guard state.isRecording else {
            print("[Hotkey] Not recording, ignoring key up")
            return
        }

        // Enforce minimum recording duration to allow audio buffers to arrive
        if let startTime = recordingStartTime {
            let elapsed = Date().timeIntervalSince(startTime)
            if elapsed < minimumRecordingDuration {
                let remaining = minimumRecordingDuration - elapsed
                print("[Hotkey] Recording too short (\(String(format: "%.0f", elapsed * 1000))ms), waiting \(String(format: "%.0f", remaining * 1000))ms...")
                Task { @MainActor in
                    try? await Task.sleep(nanoseconds: UInt64(remaining * 1_000_000_000))
                    if self.state.isRecording {
                        print("[Hotkey] Minimum duration reached, stopping...")
                        self.stopRecordingAndTranscribe()
                    }
                }
                return
            }
        }

        print("[Hotkey] Mode: Push-to-talk → Stopping recording...")
        stopRecordingAndTranscribe()
    }

    // MARK: - Recording Control

    func startRecording() {
        guard state == .idle else {
            print("[Recording] Cannot start - state is \(state), not idle")
            return
        }

        // Start a new dictation session for logging correlation
        let dictationId = loggingService.startDictationSession()
        loggingService.info(.App, LogEvent.Session.dictationStarted, data: [
            "dictationSessionId": AnyCodable(dictationId)
        ])

        print("[Recording] 🎤 Starting audio capture...")
        do {
            try audioCaptureService.startCapture()
            state = .recording
            recordingStartTime = Date()
            lastError = nil
            showOverlay()

            loggingService.info(.App, LogEvent.Session.stateChanged, data: [
                "from": AnyCodable("idle"),
                "to": AnyCodable("recording")
            ])

            print("[Recording] ✓ Audio capture started - SPEAK NOW!")
        } catch {
            print("[Recording] ✗ Failed to start: \(error.localizedDescription)")
            loggingService.error(.Audio, LogEvent.Audio.permissionDenied, data: [
                "error": AnyCodable(error.localizedDescription)
            ])
            state = .error("Failed to start recording: \(error.localizedDescription)")
            lastError = .microphonePermissionDenied
            loggingService.endDictationSession()
        }
    }

    func stopRecordingAndTranscribe() {
        guard state.isRecording else {
            print("[Recording] Cannot stop - not recording")
            return
        }

        let recordingEndTime = Date()
        let recordingDurationMs = Int((recordingEndTime.timeIntervalSince(recordingStartTime ?? recordingEndTime)) * 1000)

        print("[Recording] 🛑 Stopping audio capture...")
        let audioData = audioCaptureService.stopCapture()
        hideOverlay()
        currentAudioSamples = audioData.count

        let durationSeconds = Double(audioData.count) / 16000.0
        print("[Recording] ✓ Captured \(audioData.count) samples (~\(String(format: "%.1f", durationSeconds))s of audio)")

        loggingService.info(.App, LogEvent.Session.stateChanged, data: [
            "from": AnyCodable("recording"),
            "to": AnyCodable("transcribing")
        ])

        guard !audioData.isEmpty else {
            print("[Recording] ✗ No audio captured!")
            logDictationComplete(
                recordingDurationMs: recordingDurationMs,
                transcriptionDurationMs: 0,
                audioSamples: 0,
                resultCharacters: 0,
                success: false
            )
            state = .idle
            lastError = .noAudioCaptured
            return
        }

        state = .transcribing
        transcriptionStartTime = Date()
        print("")
        print("[Transcription] 🔄 Starting transcription...")
        print("[Transcription] Backend: \(settingsManager.backend.displayName)")
        print("[Transcription] Language: \(settingsManager.language.displayName)")

        Task {
            do {
                let startTime = CFAbsoluteTimeGetCurrent()
                let text = try await transcriptionService.transcribe(
                    audio: audioData,
                    language: settingsManager.language,
                    backend: settingsManager.backend
                )
                let elapsed = CFAbsoluteTimeGetCurrent() - startTime
                let transcriptionDurationMs = Int(elapsed * 1000)

                print("[Transcription] ✓ Completed in \(String(format: "%.2f", elapsed))s")
                print("")
                print("╔════════════════════════════════════════════════════════════╗")
                print("║  TRANSCRIPTION RESULT:                                     ║")
                print("╚════════════════════════════════════════════════════════════╝")
                print("")
                print("  \(text)")
                print("")
                print("════════════════════════════════════════════════════════════════")
                print("")

                lastTranscription = text
                state = .idle

                logDictationComplete(
                    recordingDurationMs: recordingDurationMs,
                    transcriptionDurationMs: transcriptionDurationMs,
                    audioSamples: currentAudioSamples,
                    resultCharacters: text.count,
                    success: true
                )
            } catch let error as DictationError {
                print("[Transcription] ✗ Error: \(error.localizedDescription)")
                let transcriptionDurationMs = Int((Date().timeIntervalSince(transcriptionStartTime ?? Date())) * 1000)
                logDictationComplete(
                    recordingDurationMs: recordingDurationMs,
                    transcriptionDurationMs: transcriptionDurationMs,
                    audioSamples: currentAudioSamples,
                    resultCharacters: 0,
                    success: false
                )
                state = .error(error.localizedDescription)
                lastError = error
                state = .idle
            } catch {
                print("[Transcription] ✗ Error: \(error.localizedDescription)")
                let transcriptionDurationMs = Int((Date().timeIntervalSince(transcriptionStartTime ?? Date())) * 1000)
                logDictationComplete(
                    recordingDurationMs: recordingDurationMs,
                    transcriptionDurationMs: transcriptionDurationMs,
                    audioSamples: currentAudioSamples,
                    resultCharacters: 0,
                    success: false
                )
                state = .error(error.localizedDescription)
                lastError = .transcriptionFailed(error.localizedDescription)
                state = .idle
            }
        }
    }

    private func logDictationComplete(
        recordingDurationMs: Int,
        transcriptionDurationMs: Int,
        audioSamples: Int,
        resultCharacters: Int,
        success: Bool
    ) {
        let totalDurationMs = recordingDurationMs + transcriptionDurationMs

        loggingService.info(.App, LogEvent.Session.dictationComplete, data: [
            "totalDurationMs": AnyCodable(totalDurationMs),
            "recordingDurationMs": AnyCodable(recordingDurationMs),
            "transcriptionDurationMs": AnyCodable(transcriptionDurationMs),
            "audioSamples": AnyCodable(audioSamples),
            "backend": AnyCodable(settingsManager.backend.rawValue),
            "language": AnyCodable(settingsManager.language.rawValue),
            "resultCharacters": AnyCodable(resultCharacters),
            "success": AnyCodable(success)
        ])

        loggingService.info(.App, LogEvent.Session.stateChanged, data: [
            "from": AnyCodable("transcribing"),
            "to": AnyCodable("idle")
        ])

        loggingService.endDictationSession()
    }

    func cancelRecording() {
        guard state.isRecording else { return }
        _ = audioCaptureService.stopCapture()
        hideOverlay()
        state = .idle
    }

    // MARK: - Overlay Management

    private func showOverlay() {
        guard settingsManager.showOverlay else { return }

        if overlayWindow == nil {
            overlayWindow = RecordingOverlayWindow()
        }
        overlayWindow?.show()
    }

    private func hideOverlay() {
        overlayWindow?.hide()
    }

    // MARK: - Hotkey Management

    func updateHotkey(keyCode: UInt32, modifiers: UInt32) {
        hotkeyService.unregister()
        settingsManager.hotkeyKeyCode = Int(keyCode)
        settingsManager.hotkeyModifiers = Int(modifiers)
        hotkeyService.register(keyCode: keyCode, modifiers: modifiers)
    }
}
