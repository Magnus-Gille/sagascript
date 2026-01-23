import SwiftUI
import AppKit

/// Floating overlay window that shows when recording is active
/// Uses NSPanel for always-on-top behavior
final class RecordingOverlayWindow {
    private var window: NSPanel?

    init() {
        setupWindow()
    }

    private func setupWindow() {
        // Create a borderless, floating panel
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 200, height: 60),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )

        // Configure panel behavior
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.level = .floating
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.isMovableByWindowBackground = true
        panel.hasShadow = true

        // Position at top center of screen
        if let screen = NSScreen.main {
            let screenFrame = screen.visibleFrame
            let x = screenFrame.midX - 100
            let y = screenFrame.maxY - 80
            panel.setFrameOrigin(NSPoint(x: x, y: y))
        }

        // Set SwiftUI content
        let contentView = RecordingOverlayView()
        panel.contentView = NSHostingView(rootView: contentView)

        self.window = panel
    }

    func show() {
        window?.orderFront(nil)
    }

    func hide() {
        window?.orderOut(nil)
    }
}

// MARK: - Overlay View

private struct RecordingOverlayView: View {
    @State private var isAnimating = false

    var body: some View {
        HStack(spacing: 12) {
            // Animated recording indicator
            Circle()
                .fill(Color.red)
                .frame(width: 16, height: 16)
                .scaleEffect(isAnimating ? 1.2 : 1.0)
                .animation(
                    .easeInOut(duration: 0.5)
                    .repeatForever(autoreverses: true),
                    value: isAnimating
                )

            Text("Recording...")
                .font(.system(size: 16, weight: .medium))
                .foregroundColor(.white)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
        .background(
            RoundedRectangle(cornerRadius: 12)
                .fill(Color.black.opacity(0.8))
        )
        .onAppear {
            isAnimating = true
        }
    }
}

// MARK: - Preview

#Preview {
    RecordingOverlayView()
        .frame(width: 200, height: 60)
        .background(Color.gray)
}
