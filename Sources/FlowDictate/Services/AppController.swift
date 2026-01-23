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

    // MARK: - Overlay Window

    private var overlayWindow: RecordingOverlayWindow?

    // MARK: - Computed Properties

    var isRecording: Bool { state.isRecording }
    var isTranscribing: Bool { state.isTranscribing }
    var isBusy: Bool { state.isBusy }

    // MARK: - Initialization

    private init() {
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

        // Register the hotkey
        hotkeyService.register(
            keyCode: UInt32(settingsManager.hotkeyKeyCode),
            modifiers: UInt32(settingsManager.hotkeyModifiers)
        )
    }

    private func warmUpModel() {
        print("[AppController] Starting model warm-up (this may take a while on first run)...")
        print("[AppController] The model needs to be downloaded (~50-150MB) on first launch.")
        print("")

        Task {
            do {
                try await transcriptionService.warmUp()
                isModelReady = true
                print("")
                print("╔════════════════════════════════════════════════════════════╗")
                print("║           FlowDictate Ready! Press Option+Space            ║")
                print("╚════════════════════════════════════════════════════════════╝")
                print("")
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
        print("[Hotkey] Mode: Push-to-talk → Stopping recording...")
        stopRecordingAndTranscribe()
    }

    // MARK: - Recording Control

    func startRecording() {
        guard state == .idle else {
            print("[Recording] Cannot start - state is \(state), not idle")
            return
        }

        print("[Recording] 🎤 Starting audio capture...")
        do {
            try audioCaptureService.startCapture()
            state = .recording
            lastError = nil
            showOverlay()
            print("[Recording] ✓ Audio capture started - SPEAK NOW!")
        } catch {
            print("[Recording] ✗ Failed to start: \(error.localizedDescription)")
            state = .error("Failed to start recording: \(error.localizedDescription)")
            lastError = .microphonePermissionDenied
        }
    }

    func stopRecordingAndTranscribe() {
        guard state.isRecording else {
            print("[Recording] Cannot stop - not recording")
            return
        }

        print("[Recording] 🛑 Stopping audio capture...")
        let audioData = audioCaptureService.stopCapture()
        hideOverlay()

        let durationSeconds = Double(audioData.count) / 16000.0
        print("[Recording] ✓ Captured \(audioData.count) samples (~\(String(format: "%.1f", durationSeconds))s of audio)")

        guard !audioData.isEmpty else {
            print("[Recording] ✗ No audio captured!")
            state = .idle
            lastError = .noAudioCaptured
            return
        }

        state = .transcribing
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

                print("[Transcription] ✓ Completed in \(String(format: "%.2f", elapsed))s")
                print("[Transcription] Result: \"\(text)\"")
                print("")

                lastTranscription = text

                // Paste the transcribed text
                print("[Paste] 📋 Pasting text to active application...")
                try await pasteService.paste(text: text)
                print("[Paste] ✓ Text pasted successfully!")
                print("")

                state = .idle
            } catch let error as DictationError {
                print("[Transcription] ✗ Error: \(error.localizedDescription)")
                state = .error(error.localizedDescription)
                lastError = error
                state = .idle
            } catch {
                print("[Transcription] ✗ Error: \(error.localizedDescription)")
                state = .error(error.localizedDescription)
                lastError = .transcriptionFailed(error.localizedDescription)
                state = .idle
            }
        }
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
