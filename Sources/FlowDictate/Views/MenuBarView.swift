import SwiftUI

/// Menu bar dropdown content
struct MenuBarView: View {
    @EnvironmentObject var appController: AppController
    @EnvironmentObject var settingsManager: SettingsManager

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            // Status section
            StatusSection(appController: appController)

            Divider()

            // Quick settings
            QuickSettingsSection(settingsManager: settingsManager)

            Divider()

            // Actions
            ActionsSection()
        }
        .padding(12)
        .frame(width: 280)
    }
}

// MARK: - Status Section

private struct StatusSection: View {
    @ObservedObject var appController: AppController

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Circle()
                    .fill(statusColor)
                    .frame(width: 8, height: 8)
                Text(statusText)
                    .font(.headline)
                Spacer()
            }

            if let lastTranscription = appController.lastTranscription, !lastTranscription.isEmpty {
                Text("Last: \(lastTranscription.prefix(50))...")
                    .font(.caption)
                    .foregroundColor(.secondary)
                    .lineLimit(1)
            }

            if let error = appController.lastError {
                Text(error.localizedDescription)
                    .font(.caption)
                    .foregroundColor(.red)
                    .lineLimit(2)
            }
        }
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

// MARK: - Quick Settings Section

private struct QuickSettingsSection: View {
    @ObservedObject var settingsManager: SettingsManager

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            // Language picker
            HStack {
                Text("Language:")
                    .foregroundColor(.secondary)
                Spacer()
                Picker("", selection: $settingsManager.language) {
                    ForEach(Language.allCases) { lang in
                        Text(lang.displayName).tag(lang)
                    }
                }
                .labelsHidden()
                .frame(width: 120)
            }

            // Backend picker
            HStack {
                Text("Backend:")
                    .foregroundColor(.secondary)
                Spacer()
                Picker("", selection: $settingsManager.backend) {
                    ForEach(TranscriptionBackend.allCases) { backend in
                        Text(backend.displayName).tag(backend)
                    }
                }
                .labelsHidden()
                .frame(width: 120)
            }

            // Hotkey display
            HStack {
                Text("Hotkey:")
                    .foregroundColor(.secondary)
                Spacer()
                Text(settingsManager.hotkeyDescription)
                    .font(.system(.body, design: .monospaced))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 2)
                    .background(Color.secondary.opacity(0.2))
                    .cornerRadius(4)
            }
        }
    }
}

// MARK: - Actions Section

private struct ActionsSection: View {
    @EnvironmentObject var appController: AppController
    @EnvironmentObject var settingsManager: SettingsManager

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Button(action: {
                SettingsWindowController.shared.show(
                    appController: appController,
                    settingsManager: settingsManager
                )
            }) {
                Label("Settings...", systemImage: "gear")
            }
            .buttonStyle(.plain)
            .keyboardShortcut(",", modifiers: .command)

            Divider()

            Button(action: {
                NSApplication.shared.terminate(nil)
            }) {
                Label("Quit FlowDictate", systemImage: "power")
            }
            .buttonStyle(.plain)
        }
    }
}
