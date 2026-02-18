import Foundation

/// Build information — version and git hash are stamped by build-app.sh
enum BuildInfo {
    static let version = "1.1.0"
    static let build = "1"
    // Replaced by build-app.sh with actual git short hash
    static let gitHash = "dev"

    static var displayString: String {
        if gitHash == "dev" {
            return "v\(version) (dev build)"
        }
        return "v\(version) build \(build) (\(gitHash))"
    }
}
