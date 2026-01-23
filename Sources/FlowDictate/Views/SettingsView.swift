import SwiftUI

/// Settings window content
struct SettingsView: View {
    @EnvironmentObject var appController: AppController
    @EnvironmentObject var settingsManager: SettingsManager

    var body: some View {
        TabView {
            GeneralSettingsTab()
                .environmentObject(settingsManager)
                .tabItem {
                    Label("General", systemImage: "gear")
                }

            TranscriptionSettingsTab()
                .environmentObject(settingsManager)
                .tabItem {
                    Label("Transcription", systemImage: "waveform")
                }

            APISettingsTab()
                .tabItem {
                    Label("API", systemImage: "key")
                }
        }
        .frame(width: 450, height: 300)
        .padding()
    }
}

// MARK: - General Settings Tab

private struct GeneralSettingsTab: View {
    @EnvironmentObject var settingsManager: SettingsManager

    var body: some View {
        Form {
            Section {
                // Hotkey mode
                Picker("Hotkey Mode:", selection: $settingsManager.hotkeyMode) {
                    ForEach(HotkeyMode.allCases) { mode in
                        VStack(alignment: .leading) {
                            Text(mode.displayName)
                            Text(mode.description)
                                .font(.caption)
                                .foregroundColor(.secondary)
                        }
                        .tag(mode)
                    }
                }
                .pickerStyle(.radioGroup)

                // Current hotkey display
                HStack {
                    Text("Current Hotkey:")
                    Spacer()
                    Text(settingsManager.hotkeyDescription)
                        .font(.system(.body, design: .monospaced))
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background(Color.secondary.opacity(0.2))
                        .cornerRadius(4)
                }

                // Overlay toggle
                Toggle("Show recording overlay", isOn: $settingsManager.showOverlay)
            } header: {
                Text("Hotkey & Display")
            }
        }
        .formStyle(.grouped)
    }
}

// MARK: - Transcription Settings Tab

private struct TranscriptionSettingsTab: View {
    @EnvironmentObject var settingsManager: SettingsManager

    var body: some View {
        Form {
            Section {
                // Language picker
                Picker("Language:", selection: $settingsManager.language) {
                    ForEach(Language.allCases) { lang in
                        Text(lang.displayName).tag(lang)
                    }
                }

                // Backend picker
                Picker("Backend:", selection: $settingsManager.backend) {
                    ForEach(TranscriptionBackend.allCases) { backend in
                        VStack(alignment: .leading) {
                            Text(backend.displayName)
                            Text(backend.description)
                                .font(.caption)
                                .foregroundColor(.secondary)
                        }
                        .tag(backend)
                    }
                }
                .pickerStyle(.radioGroup)
            } header: {
                Text("Transcription")
            }

            if settingsManager.backend == .remote {
                Section {
                    HStack {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .foregroundColor(.yellow)
                        Text("Audio will be sent to OpenAI for transcription. Configure your API key in the API tab.")
                            .font(.caption)
                    }
                } header: {
                    Text("Privacy Notice")
                }
            }
        }
        .formStyle(.grouped)
    }
}

// MARK: - API Settings Tab

private struct APISettingsTab: View {
    @State private var apiKey: String = ""
    @State private var hasExistingKey: Bool = false
    @State private var showSaveConfirmation: Bool = false

    private let keychainService = KeychainService.shared

    var body: some View {
        Form {
            Section {
                VStack(alignment: .leading, spacing: 8) {
                    if hasExistingKey {
                        HStack {
                            Image(systemName: "checkmark.circle.fill")
                                .foregroundColor(.green)
                            Text("API key is configured")
                        }
                    }

                    SecureField("OpenAI API Key", text: $apiKey)
                        .textFieldStyle(.roundedBorder)

                    HStack {
                        Button("Save Key") {
                            saveAPIKey()
                        }
                        .disabled(apiKey.isEmpty)

                        if hasExistingKey {
                            Button("Delete Key", role: .destructive) {
                                deleteAPIKey()
                            }
                        }

                        Spacer()

                        if showSaveConfirmation {
                            Text("Saved!")
                                .foregroundColor(.green)
                        }
                    }
                }
            } header: {
                Text("OpenAI API Key")
            } footer: {
                Text("Your API key is stored securely in the macOS Keychain and is never logged.")
                    .font(.caption)
                    .foregroundColor(.secondary)
            }

            Section {
                Link("Get an API key from OpenAI",
                     destination: URL(string: "https://platform.openai.com/api-keys")!)
                Link("View OpenAI API pricing",
                     destination: URL(string: "https://openai.com/pricing")!)
            } header: {
                Text("Resources")
            }
        }
        .formStyle(.grouped)
        .onAppear {
            hasExistingKey = keychainService.hasAPIKey
        }
    }

    private func saveAPIKey() {
        if keychainService.saveAPIKey(apiKey) {
            hasExistingKey = true
            apiKey = ""
            showSaveConfirmation = true

            // Hide confirmation after delay
            DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
                showSaveConfirmation = false
            }
        }
    }

    private func deleteAPIKey() {
        keychainService.deleteAPIKey()
        hasExistingKey = false
    }
}
