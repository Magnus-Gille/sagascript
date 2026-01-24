import AppKit
import SwiftUI

/// Manages the app's settings window so we can guarantee it becomes key/main even in LSUIElement apps.
final class SettingsWindowController: NSWindowController {
    static let shared = SettingsWindowController()

    private init() {
        super.init(window: nil)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    /// Show the settings window, creating it if needed.
    func show(appController: AppController, settingsManager: SettingsManager) {
        if window == nil {
            let contentView = SettingsView()
                .environmentObject(appController)
                .environmentObject(settingsManager)
            let hostingController = NSHostingController(rootView: contentView)
            let settingsWindow = SettingsWindow(contentViewController: hostingController)
            settingsWindow.title = "FlowDictate Settings"
            settingsWindow.isReleasedWhenClosed = false
            self.window = settingsWindow
        }

        guard let window = window else { return }

        // Bring forward and ensure it is key/main so it can receive keyboard events.
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }
}

/// NSWindow subclass that always allows key/main status.
private final class SettingsWindow: NSWindow {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { true }

    convenience init(contentViewController: NSViewController) {
        self.init(
            contentRect: NSRect(x: 0, y: 0, width: 520, height: 360),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        self.contentViewController = contentViewController
        self.center()
        level = .normal
    }
}
