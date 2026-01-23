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
        self.settingsManager = SettingsManager.shared
        self.hotkeyService = HotkeyService()
        self.audioCaptureService = AudioCaptureService()
        self.transcriptionService = TranscriptionService()
        self.pasteService = PasteService()

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
        Task {
            do {
                try await transcriptionService.warmUp()
                isModelReady = true
            } catch {
                print("Failed to warm up model: \(error)")
            }
        }
    }

    // MARK: - Hotkey Handlers

    private func handleHotkeyDown() {
        switch settingsManager.hotkeyMode {
        case .pushToTalk:
            startRecording()
        case .toggle:
            if state.isRecording {
                stopRecordingAndTranscribe()
            } else if state == .idle {
                startRecording()
            }
        }
    }

    private func handleHotkeyUp() {
        guard settingsManager.hotkeyMode == .pushToTalk else { return }
        guard state.isRecording else { return }
        stopRecordingAndTranscribe()
    }

    // MARK: - Recording Control

    func startRecording() {
        guard state == .idle else { return }

        do {
            try audioCaptureService.startCapture()
            state = .recording
            lastError = nil
            showOverlay()
        } catch {
            state = .error("Failed to start recording: \(error.localizedDescription)")
            lastError = .microphonePermissionDenied
        }
    }

    func stopRecordingAndTranscribe() {
        guard state.isRecording else { return }

        let audioData = audioCaptureService.stopCapture()
        hideOverlay()

        guard !audioData.isEmpty else {
            state = .idle
            lastError = .noAudioCaptured
            return
        }

        state = .transcribing

        Task {
            do {
                let text = try await transcriptionService.transcribe(
                    audio: audioData,
                    language: settingsManager.language,
                    backend: settingsManager.backend
                )

                lastTranscription = text

                // Paste the transcribed text
                try await pasteService.paste(text: text)

                state = .idle
            } catch let error as DictationError {
                state = .error(error.localizedDescription)
                lastError = error
                state = .idle
            } catch {
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
