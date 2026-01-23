import SwiftUI

/// Main window that shows on launch for configuration and status
struct MainWindowView: View {
    @EnvironmentObject var appController: AppController
    @EnvironmentObject var settingsManager: SettingsManager

    var body: some View {
        VStack(spacing: 20) {
            // Title
            Text("FlowDictate")
                .font(.largeTitle)
                .fontWeight(.bold)

            // Status indicator
            StatusBadge(appController: appController)

            Divider()

            // Hotkey display
            HotkeyDisplaySection(settingsManager: settingsManager)

            // Instructions
            InstructionsSection(settingsManager: settingsManager)

            Divider()

            // Last transcription
            TranscriptionResultSection(appController: appController)

            Spacer()

            // Actions
            HStack {
                SettingsLink {
                    Text("Settings...")
                }
                .buttonStyle(.bordered)

                Spacer()

                Button("Quit") {
                    NSApplication.shared.terminate(nil)
                }
                .buttonStyle(.bordered)
            }
        }
        .padding(24)
        .frame(width: 400, height: 450)
    }
}

// MARK: - Status Badge

private struct StatusBadge: View {
    @ObservedObject var appController: AppController

    var body: some View {
        HStack(spacing: 8) {
            Circle()
                .fill(statusColor)
                .frame(width: 12, height: 12)

            Text(statusText)
                .font(.headline)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .background(statusColor.opacity(0.15))
        .cornerRadius(20)
    }

    private var statusColor: Color {
        switch appController.state {
        case .idle:
            return appController.isModelReady ? .green : .yellow
        case .recording:
            return .red
        case .transcribing:
            return .blue
        case .error:
            return .red
        }
    }

    private var statusText: String {
        switch appController.state {
        case .idle:
            return appController.isModelReady ? "Ready" : "Loading model..."
        case .recording:
            return "Recording..."
        case .transcribing:
            return "Transcribing..."
        case .error(let message):
            return "Error: \(message)"
        }
    }
}

// MARK: - Hotkey Display Section

private struct HotkeyDisplaySection: View {
    @ObservedObject var settingsManager: SettingsManager

    var body: some View {
        VStack(spacing: 8) {
            Text("Hotkey")
                .font(.subheadline)
                .foregroundColor(.secondary)

            Text(settingsManager.hotkeyDescription)
                .font(.system(size: 24, weight: .medium, design: .monospaced))
                .padding(.horizontal, 20)
                .padding(.vertical, 10)
                .background(Color.secondary.opacity(0.15))
                .cornerRadius(8)
        }
    }
}

// MARK: - Instructions Section

private struct InstructionsSection: View {
    @ObservedObject var settingsManager: SettingsManager

    var body: some View {
        VStack(spacing: 4) {
            if settingsManager.hotkeyMode == .pushToTalk {
                Text("Hold the hotkey and speak, release to transcribe")
                    .font(.caption)
                    .foregroundColor(.secondary)
            } else {
                Text("Press hotkey to start/stop recording")
                    .font(.caption)
                    .foregroundColor(.secondary)
            }
        }
    }
}

// MARK: - Transcription Result Section

private struct TranscriptionResultSection: View {
    @ObservedObject var appController: AppController

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Last Transcription")
                .font(.subheadline)
                .foregroundColor(.secondary)

            ScrollView {
                if let transcription = appController.lastTranscription, !transcription.isEmpty {
                    Text(transcription)
                        .font(.body)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .textSelection(.enabled)
                } else {
                    Text("No transcription yet. Press the hotkey and speak to begin.")
                        .font(.body)
                        .foregroundColor(.secondary)
                        .italic()
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .frame(height: 100)
            .padding(8)
            .background(Color.secondary.opacity(0.1))
            .cornerRadius(8)

            if let error = appController.lastError {
                HStack {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundColor(.red)
                    Text(error.localizedDescription)
                        .font(.caption)
                        .foregroundColor(.red)
                }
            }
        }
    }
}
