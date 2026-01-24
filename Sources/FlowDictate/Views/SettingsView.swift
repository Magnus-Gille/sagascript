import SwiftUI

/// Settings window content
struct SettingsView: View {
    @EnvironmentObject var appController: AppController
    @EnvironmentObject var settingsManager: SettingsManager

    var body: some View {
        TabView {
            GeneralSettingsTab()
                .environmentObject(settingsManager)
                .environmentObject(appController)
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
    @EnvironmentObject var appController: AppController
    @StateObject private var launchAtLogin = LaunchAtLoginService.shared

    var body: some View {
        Form {
            Section {
                // Launch at login
                Toggle("Launch at login", isOn: Binding(
                    get: { launchAtLogin.isEnabled },
                    set: { launchAtLogin.setEnabled($0) }
                ))

                if let error = launchAtLogin.error {
                    HStack {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .foregroundColor(.yellow)
                        Text(error)
                            .font(.caption)
                    }
                }
            } header: {
                Text("Startup")
            }

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

                // Hotkey recorder
                HotkeyRecorderView(
                    keyCode: $settingsManager.hotkeyKeyCode,
                    modifiers: $settingsManager.hotkeyModifiers,
                    onHotkeyChanged: {
                        appController.updateHotkey(
                            keyCode: settingsManager.hotkeyKeyCode,
                            modifiers: settingsManager.hotkeyModifiers
                        )
                    }
                )

                // Overlay toggle
                Toggle("Show recording overlay", isOn: $settingsManager.showOverlay)

                // Auto-paste toggle
                Toggle("Auto-paste transcription", isOn: $settingsManager.autoPaste)
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

    /// Whether the current model/language combination has a compatibility issue
    private var hasModelLanguageConflict: Bool {
        // English-only model with non-English language
        if settingsManager.whisperModel.isEnglishOnly && settingsManager.language == .swedish {
            return true
        }
        return false
    }

    /// Whether to show Swedish model recommendation
    private var shouldRecommendSwedishModel: Bool {
        settingsManager.language == .swedish &&
        !settingsManager.whisperModel.isSwedishOptimized &&
        !settingsManager.autoSelectModel
    }

    /// Models to show based on current settings
    private var availableModels: [WhisperModel] {
        // Show all models, but group them logically
        WhisperModel.allCases
    }

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

            // Model selection (only for local backend)
            if settingsManager.backend == .local {
                Section {
                    // Auto-select model toggle
                    Toggle("Auto-select best model for language", isOn: $settingsManager.autoSelectModel)

                    if settingsManager.autoSelectModel {
                        HStack {
                            Image(systemName: "checkmark.circle.fill")
                                .foregroundColor(.green)
                            Text("Using: \(settingsManager.effectiveModel.displayName)")
                                .font(.caption)
                        }
                    }

                    // Manual model picker (only when auto-select is off)
                    if !settingsManager.autoSelectModel {
                        Picker("Model:", selection: $settingsManager.whisperModel) {
                            // Standard models section
                            Section(header: Text("Standard Models")) {
                                ForEach(WhisperModel.standardModels) { model in
                                    VStack(alignment: .leading) {
                                        Text(model.displayName)
                                        Text(model.description)
                                            .font(.caption)
                                            .foregroundColor(.secondary)
                                    }
                                    .tag(model)
                                }
                            }

                            // Swedish-optimized models section
                            Section(header: Text("Swedish-Optimized (KB-Whisper)")) {
                                ForEach(WhisperModel.swedishModels) { model in
                                    VStack(alignment: .leading) {
                                        HStack {
                                            Text(model.displayName)
                                            if settingsManager.language == .swedish {
                                                Text("Recommended")
                                                    .font(.caption2)
                                                    .padding(.horizontal, 4)
                                                    .padding(.vertical, 1)
                                                    .background(Color.blue.opacity(0.2))
                                                    .cornerRadius(4)
                                            }
                                        }
                                        Text(model.description)
                                            .font(.caption)
                                            .foregroundColor(.secondary)
                                    }
                                    .tag(model)
                                }
                            }
                        }
                        .pickerStyle(.radioGroup)
                    }

                    // Warning for English-only model with Swedish language
                    if hasModelLanguageConflict && !settingsManager.autoSelectModel {
                        HStack {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .foregroundColor(.red)
                            Text("This model only supports English. Swedish transcription will not work.")
                                .font(.caption)
                        }
                    }

                    // Recommendation for Swedish language with standard model
                    if shouldRecommendSwedishModel {
                        HStack {
                            Image(systemName: "lightbulb.fill")
                                .foregroundColor(.yellow)
                            VStack(alignment: .leading) {
                                Text("For better Swedish transcription, try KB-Whisper Base.")
                                    .font(.caption)
                                Text("KB-Whisper models have 4x lower error rate on Swedish.")
                                    .font(.caption2)
                                    .foregroundColor(.secondary)
                            }
                        }
                    }
                } header: {
                    Text("WhisperKit Model")
                } footer: {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Standard models work for all languages. KB-Whisper models are fine-tuned on 50,000+ hours of Swedish speech for significantly better Swedish transcription.")
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }
                }
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
