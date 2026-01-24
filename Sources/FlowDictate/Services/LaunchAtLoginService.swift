import Foundation
import ServiceManagement

/// Manages launch-at-login functionality using SMAppService (macOS 13+)
@MainActor
final class LaunchAtLoginService: ObservableObject {
    static let shared = LaunchAtLoginService()

    @Published private(set) var isEnabled: Bool = false
    @Published private(set) var error: String?

    private init() {
        updateStatus()
    }

    /// Update the current status from the system
    func updateStatus() {
        let status = SMAppService.mainApp.status
        isEnabled = (status == .enabled)

        switch status {
        case .notRegistered:
            error = nil
        case .enabled:
            error = nil
        case .requiresApproval:
            error = "Requires approval in System Settings > General > Login Items"
        case .notFound:
            error = "App not found for login item registration"
        @unknown default:
            error = "Unknown status"
        }
    }

    /// Enable or disable launch at login
    func setEnabled(_ enabled: Bool) {
        do {
            if enabled {
                try SMAppService.mainApp.register()
            } else {
                try SMAppService.mainApp.unregister()
            }
            updateStatus()
        } catch {
            self.error = error.localizedDescription
            updateStatus()
        }
    }
}
