import SwiftUI
import AppKit

/// FlowDictate - A low-latency dictation app for macOS
/// Main entry point using SwiftUI MenuBarExtra
@main
struct FlowDictateApp: App {
    @StateObject private var appController = AppController.shared
    @StateObject private var settingsManager = SettingsManager.shared

    var body: some Scene {
        // Main window shown on launch
        Window("FlowDictate", id: "main") {
            MainWindowView()
                .environmentObject(appController)
                .environmentObject(settingsManager)
        }
        .windowResizability(.contentSize)
        .defaultPosition(.center)

        // Menu bar app
        MenuBarExtra {
            MenuBarView()
                .environmentObject(appController)
                .environmentObject(settingsManager)
        } label: {
            MenuBarIcon(isRecording: appController.isRecording)
        }
        .menuBarExtraStyle(.window)

        // Settings window
        Settings {
            SettingsView()
                .environmentObject(appController)
                .environmentObject(settingsManager)
        }
    }
}

/// Menu bar icon that changes based on recording state
struct MenuBarIcon: View {
    let isRecording: Bool

    var body: some View {
        Image(systemName: isRecording ? "waveform.circle.fill" : "waveform.circle")
            .foregroundColor(isRecording ? .red : .primary)
    }
}
