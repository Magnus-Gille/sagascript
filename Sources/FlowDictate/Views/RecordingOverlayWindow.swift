import SwiftUI
import AppKit

/// State of the recording overlay
enum RecordingOverlayState {
    case recording          // Model ready, recording in progress
    case recordingNoModel   // Recording but model not loaded yet
    case loadingModel       // Model is being loaded/switched
}

/// Floating overlay window that shows when recording is active
/// Uses NSPanel for always-on-top behavior
final class RecordingOverlayWindow {
    private var window: NSPanel?
    private var hostingView: NSHostingView<RecordingOverlayView>?
    private var overlayState = RecordingOverlayState.recording

    init() {
        setupWindow()
    }

    private func setupWindow() {
        // Create a borderless, floating panel
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 220, height: 60),
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
            let x = screenFrame.midX - 110
            let y = screenFrame.maxY - 80
            panel.setFrameOrigin(NSPoint(x: x, y: y))
        }

        // Set SwiftUI content
        let contentView = RecordingOverlayView(state: overlayState)
        let hosting = NSHostingView(rootView: contentView)
        panel.contentView = hosting

        self.window = panel
        self.hostingView = hosting
    }

    func show(modelReady: Bool = true) {
        overlayState = modelReady ? .recording : .recordingNoModel
        hostingView?.rootView = RecordingOverlayView(state: overlayState)
        window?.orderFront(nil)
    }

    func showLoadingModel() {
        overlayState = .loadingModel
        hostingView?.rootView = RecordingOverlayView(state: overlayState)
        window?.orderFront(nil)
    }

    func updateState(modelReady: Bool) {
        let newState: RecordingOverlayState = modelReady ? .recording : .recordingNoModel
        if newState != overlayState {
            overlayState = newState
            hostingView?.rootView = RecordingOverlayView(state: overlayState)
        }
    }

    func hide() {
        window?.orderOut(nil)
    }
}

// MARK: - Overlay View

private struct RecordingOverlayView: View {
    let state: RecordingOverlayState
    @State private var isAnimating = false

    var body: some View {
        HStack(spacing: 12) {
            // Indicator based on state
            switch state {
            case .recording:
                // Animated red recording indicator
                Circle()
                    .fill(Color.red)
                    .frame(width: 16, height: 16)
                    .scaleEffect(isAnimating ? 1.2 : 1.0)
                    .animation(
                        .easeInOut(duration: 0.5)
                        .repeatForever(autoreverses: true),
                        value: isAnimating
                    )

            case .recordingNoModel:
                // Warning indicator - orange with exclamation
                ZStack {
                    Circle()
                        .fill(Color.orange)
                        .frame(width: 20, height: 20)
                    Text("!")
                        .font(.system(size: 14, weight: .bold))
                        .foregroundColor(.white)
                }
                .scaleEffect(isAnimating ? 1.1 : 1.0)
                .animation(
                    .easeInOut(duration: 0.3)
                    .repeatForever(autoreverses: true),
                    value: isAnimating
                )

            case .loadingModel:
                // Loading spinner
                ProgressView()
                    .scaleEffect(0.8)
                    .progressViewStyle(CircularProgressViewStyle(tint: .white))
            }

            // Text based on state
            Text(stateText)
                .font(.system(size: 14, weight: .medium))
                .foregroundColor(.white)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
        .background(
            RoundedRectangle(cornerRadius: 12)
                .fill(backgroundColor)
        )
        .onAppear {
            isAnimating = true
        }
    }

    private var stateText: String {
        switch state {
        case .recording:
            return "Recording..."
        case .recordingNoModel:
            return "Loading model..."
        case .loadingModel:
            return "Switching model..."
        }
    }

    private var backgroundColor: Color {
        switch state {
        case .recording:
            return Color.black.opacity(0.8)
        case .recordingNoModel:
            return Color.orange.opacity(0.9)
        case .loadingModel:
            return Color.blue.opacity(0.8)
        }
    }
}

// MARK: - Preview

#Preview {
    VStack(spacing: 20) {
        RecordingOverlayView(state: .recording)
        RecordingOverlayView(state: .recordingNoModel)
        RecordingOverlayView(state: .loadingModel)
    }
    .padding()
    .background(Color.gray)
}
