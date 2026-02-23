use tauri::Manager;
use tracing::{error, info};

const OVERLAY_LABEL: &str = "overlay";

/// Show the recording overlay window (create lazily on first call)
pub fn show(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        let _ = window.show();
        #[cfg(target_os = "macos")]
        macos_order_front(&window);
        info!("Overlay shown (existing window)");
    } else {
        match create_overlay(app) {
            Ok(_) => info!("Overlay created and shown"),
            Err(e) => error!("Failed to create overlay: {e}"),
        }
    }
}

/// Hide the recording overlay window
pub fn hide(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        let _ = window.hide();
        info!("Overlay hidden");
    }
}

fn create_overlay(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    // Calculate horizontal center position
    let (x, _screen_width) = if let Some(monitor) = app.primary_monitor()? {
        let size = monitor.size();
        let scale = monitor.scale_factor();
        let logical_width = size.width as f64 / scale;
        ((logical_width / 2.0 - 110.0), logical_width)
    } else {
        (500.0, 1200.0)
    };

    let window = tauri::WebviewWindowBuilder::new(
        app,
        OVERLAY_LABEL,
        tauri::WebviewUrl::App("index.html?overlay=true".into()),
    )
    .title("")
    .inner_size(220.0, 60.0)
    .position(x, 80.0)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .focused(false)
    .resizable(false)
    .skip_taskbar(true)
    .build()?;

    #[cfg(target_os = "macos")]
    configure_macos_window(&window);

    // Click-through: cross-platform via Tauri API
    let _ = window.set_ignore_cursor_events(true);

    // Suppress close — just hide instead
    let _ = window;

    Ok(())
}

/// macOS-specific: configure NSWindow for overlay behaviour
#[cfg(target_os = "macos")]
#[allow(deprecated, unexpected_cfgs)]
fn configure_macos_window(window: &tauri::WebviewWindow) {
    use cocoa::appkit::NSWindow;
    use cocoa::base::{id, NO};

    let ns_window: id = window.ns_window().unwrap() as id;

    unsafe {
        // NSStatusWindowLevel (25) — above normal windows but below screen saver
        ns_window.setLevel_(25);

        // canJoinAllSpaces (1) | stationary (16) | fullScreenAuxiliary (256)
        let behavior: u64 = 1 | 16 | 256;
        let _: () = objc::msg_send![ns_window, setCollectionBehavior: behavior];

        // Transparent chrome
        ns_window.setOpaque_(NO);
        let clear_color: id = objc::msg_send![objc::class!(NSColor), clearColor];
        ns_window.setBackgroundColor_(clear_color);

        // No shadow (CSS provides its own)
        ns_window.setHasShadow_(NO);
    }
}

/// macOS-specific: bring window to front without stealing focus
#[cfg(target_os = "macos")]
#[allow(deprecated, unexpected_cfgs)]
fn macos_order_front(window: &tauri::WebviewWindow) {
    use cocoa::base::{id, nil};

    let ns_window: id = window.ns_window().unwrap() as id;
    unsafe {
        let _: () = objc::msg_send![ns_window, orderFront: nil];
    }
}
