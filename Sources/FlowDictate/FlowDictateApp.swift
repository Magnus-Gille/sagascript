import SwiftUI
import AppKit

/// FlowDictate - A low-latency dictation app for macOS
/// Main entry point using SwiftUI MenuBarExtra (menu bar only, no dock icon)
@main
struct FlowDictateApp: App {
    @StateObject private var appController = AppController.shared
    @StateObject private var settingsManager = SettingsManager.shared

    var body: some Scene {
        // Menu bar app - primary interface
        MenuBarExtra {
            MenuBarView()
                .environmentObject(appController)
                .environmentObject(settingsManager)
        } label: {
            MenuBarIcon(isRecording: appController.isRecording)
        }
        .menuBarExtraStyle(.window)
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
