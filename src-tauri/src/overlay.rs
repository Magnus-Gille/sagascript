use tauri::Manager;
#[cfg(not(target_os = "linux"))]
use tracing::error;
use tracing::info;

/// True until the first overlay window of this app session has been created.
#[cfg(not(target_os = "linux"))]
static FIRST_OVERLAY_PENDING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

const OVERLAY_LABEL: &str = "overlay";

// The recording indicator is click-through and must never become the key
// window. In particular, Tauri's macOS `show` implementation calls
// `makeKeyAndOrderFront`, which would move focus away from the destination
// editor while a transcription is in flight.

/// Show the recording overlay window (create lazily on first call).
///
/// Disabled on Linux: creating the transparent, always-on-top overlay window
/// triggers an X11 window-lifecycle crash that terminates the app on several
/// compositors. Transcription and auto-paste still work — only the visual
/// recording indicator is suppressed. See issue #44.
pub fn show(app: &tauri::AppHandle) {
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        info!("Overlay disabled on Linux (avoids X11 window-lifecycle crash)");
    }
    #[cfg(not(target_os = "linux"))]
    {
        if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
            reposition(app, &window);
            let moved_to_active_space = present_existing_overlay(&window);
            info!("Overlay shown (existing window)");
            crate::focus_diag::log(
                "overlay_shown",
                serde_json::json!({
                    "created": false,
                    "firstCreationInSession": false,
                    "movedToActiveSpace": moved_to_active_space,
                }),
            );
        } else {
            let first = FIRST_OVERLAY_PENDING.swap(false, std::sync::atomic::Ordering::AcqRel);
            match create_overlay(app, true) {
                Ok(_) => {
                    info!("Overlay created and shown");
                    crate::focus_diag::log(
                        "overlay_shown",
                        serde_json::json!({ "created": true, "firstCreationInSession": first }),
                    );
                }
                Err(e) => error!("Failed to create overlay: {e}"),
            }
        }
    }
}

/// Hide the recording overlay window
pub fn hide(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        #[cfg(target_os = "macos")]
        let token = begin_hide(&MacOverlayWindow(&window), &PRESENTATIONS);
        let _ = window.hide();
        // Off the main thread (`cancel_recording`) the restore is queued behind
        // the hide; on the main thread it runs at once.
        #[cfg(target_os = "macos")]
        restore_all_spaces_on_main(app.clone(), token);
        info!("Overlay hidden");
    }
}

#[cfg(not(target_os = "linux"))]
fn create_overlay(app: &tauri::AppHandle, present: bool) -> Result<(), Box<dyn std::error::Error>> {
    let (x, y) = overlay_position(app);

    let window = tauri::WebviewWindowBuilder::new(
        app,
        OVERLAY_LABEL,
        tauri::WebviewUrl::App("index.html?overlay=true".into()),
    )
    .title("")
    .inner_size(OVERLAY_WIDTH, 60.0)
    .position(x, y)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .visible(false)
    .focusable(false)
    .focused(false)
    .resizable(false)
    .skip_taskbar(true)
    .build()?;

    #[cfg(target_os = "macos")]
    configure_macos_window(&window);

    // Click-through: cross-platform via Tauri API
    let _ = window.set_ignore_cursor_events(true);

    if present {
        present_existing_overlay(&window);
    }

    Ok(())
}

#[cfg(any(not(target_os = "linux"), test))]
const OVERLAY_WIDTH: f64 = 220.0;
#[cfg(not(target_os = "linux"))]
const OVERLAY_TOP: f64 = 80.0;
#[cfg(not(target_os = "linux"))]
const FALLBACK_SCREEN_WIDTH: f64 = 1200.0;

/// Horizontal origin (logical points) that centres the overlay on a screen.
#[cfg(any(not(target_os = "linux"), test))]
fn overlay_x_for_screen_width(logical_screen_width: f64) -> f64 {
    logical_screen_width / 2.0 - OVERLAY_WIDTH / 2.0
}

/// Current target position, computed from the primary monitor at call time.
#[cfg(not(target_os = "linux"))]
fn overlay_position(app: &tauri::AppHandle) -> (f64, f64) {
    let width = match app.primary_monitor() {
        Ok(Some(monitor)) => monitor.size().width as f64 / monitor.scale_factor(),
        _ => FALLBACK_SCREEN_WIDTH,
    };
    (overlay_x_for_screen_width(width), OVERLAY_TOP)
}

/// Move the (possibly pre-created) overlay to the current screen before it is shown.
#[cfg(not(target_os = "linux"))]
fn reposition(app: &tauri::AppHandle, window: &tauri::WebviewWindow) {
    let (x, y) = overlay_position(app);
    let _ = window.set_position(tauri::LogicalPosition::new(x, y));
}

/// Whether a settings change should pre-create the overlay now.
pub fn should_precreate_on_toggle(enabled: bool, exists: bool) -> bool {
    enabled && !exists
}

/// Settings toggle hook: build the hidden overlay on the main thread when enabled.
pub fn on_show_overlay_changed(app: &tauri::AppHandle, enabled: bool) {
    let exists = app.get_webview_window(OVERLAY_LABEL).is_some();
    if should_precreate_on_toggle(enabled, exists) {
        let handle = app.clone();
        if let Err(e) = app.run_on_main_thread(move || precreate_hidden(&handle)) {
            tracing::warn!("Could not schedule overlay pre-creation: {e}");
        }
    }
}

/// Whether the overlay window should be built (hidden) during app startup.
///
/// The first `WebviewWindowBuilder::build` after launch is where the Settings
/// window was observed coming forward during the first dictation (#291). On
/// macOS tao's launch path activates the app and re-orders visible windows key
/// (`tao::platform_impl::macos::app_state::launched` ->
/// `activateIgnoringOtherApps` + `window_activation_hack`), and a queued
/// activation from `main_window_reveal` lands around the same time. Building the
/// window at startup, while Sagascript is still frontmost, means the first
/// dictation only runs `orderFront` on an existing window, which is the path
/// later dictations take and never steals focus. Linux never creates it (#44).
pub fn should_precreate_at_startup(show_overlay: bool, target_os_supported: bool) -> bool {
    show_overlay && target_os_supported
}

/// Build the overlay window hidden (never ordered front, never key/main) so the
/// first dictation reuses it. No-op if it already exists or on Linux.
pub fn precreate_hidden(app: &tauri::AppHandle) {
    #[cfg(not(target_os = "linux"))]
    {
        if app.get_webview_window(OVERLAY_LABEL).is_some() {
            return;
        }
        match create_overlay(app, false) {
            Ok(()) => {
                FIRST_OVERLAY_PENDING.store(false, std::sync::atomic::Ordering::Release);
                info!("Overlay pre-created hidden at startup");
                crate::focus_diag::log("overlay_precreated", serde_json::json!({}));
            }
            Err(e) => error!("Failed to pre-create overlay: {e}"),
        }
    }
    #[cfg(target_os = "linux")]
    let _ = app;
}

/// Show the overlay; returns whether it first had to be moved to the active Space.
#[cfg(target_os = "macos")]
fn present_existing_overlay(window: &tauri::WebviewWindow) -> bool {
    let moved = present_on_active_space(&MacOverlayWindow(window), &PRESENTATIONS);
    if moved {
        info!("Overlay was not on the active Space; moved it there");
    }
    moved
}

#[cfg(target_os = "windows")]
fn present_existing_overlay(window: &tauri::WebviewWindow) -> bool {
    let _ = window.show();
    false
}

/// canJoinAllSpaces (1) | stationary (16) | ignoresCycle (64) | fullScreenAuxiliary (256)
#[cfg(any(target_os = "macos", test))]
const OVERLAY_COLLECTION_BEHAVIOR: u64 = 1 | 16 | 64 | 256;
/// The same with moveToActiveSpace (2) in place of canJoinAllSpaces; AppKit
/// rejects a behavior that has both.
#[cfg(any(target_os = "macos", test))]
const MOVE_TO_ACTIVE_SPACE_BEHAVIOR: u64 = 2 | 16 | 64 | 256;

/// Every overlay show and hide starts a new presentation. A deferred
/// all-Spaces restore applies only while the presentation that scheduled it is
/// still the latest, so it cannot land in a later show's Space move.
#[cfg(any(target_os = "macos", test))]
struct OverlayPresentations(std::sync::atomic::AtomicU64);

#[cfg(any(target_os = "macos", test))]
impl OverlayPresentations {
    const fn new() -> Self {
        Self(std::sync::atomic::AtomicU64::new(0))
    }

    fn next(&self) -> u64 {
        self.0.fetch_add(1, std::sync::atomic::Ordering::AcqRel) + 1
    }

    fn is_current(&self, presentation: u64) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire) == presentation
    }
}

#[cfg(target_os = "macos")]
static PRESENTATIONS: OverlayPresentations = OverlayPresentations::new();

/// Which presentation, and which native window, a restore belongs to.
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RestoreToken {
    presentation: u64,
    window: usize,
}

/// The NSWindow calls that put the overlay on the Space the user is looking at.
#[cfg(any(target_os = "macos", test))]
trait OverlaySpaceWindow {
    /// The native window, so a recreated overlay is told apart.
    fn identity(&self) -> usize;
    fn is_on_active_space(&self) -> bool;
    fn set_collection_behavior(&self, behavior: u64);
    fn order_front(&self);
    /// Run [`restore_all_spaces_if_current`] with `token` on a later main-thread turn.
    fn restore_all_spaces_later(&self, token: RestoreToken);
}

/// Order the overlay front without making it key, on the active Space.
///
/// A long-running overlay window can end up a member of only some Spaces even
/// though it was configured with canJoinAllSpaces: `orderFront` then puts it on
/// a Space the user is not looking at, macOS reports it occluded, WebKit keeps
/// its page hidden and no indicator appears. Re-setting canJoinAllSpaces does
/// not repair that. Ordering it front with moveToActiveSpace does; restoring
/// canJoinAllSpaces in the same main-thread turn undoes the move, so it waits.
#[cfg(any(target_os = "macos", test))]
fn present_on_active_space(
    window: &impl OverlaySpaceWindow,
    presentations: &OverlayPresentations,
) -> bool {
    let token = RestoreToken {
        presentation: presentations.next(),
        window: window.identity(),
    };
    let off_active_space = !window.is_on_active_space();
    if off_active_space {
        window.set_collection_behavior(MOVE_TO_ACTIVE_SPACE_BEHAVIOR);
    }
    window.order_front();
    if off_active_space {
        window.restore_all_spaces_later(token);
    }
    off_active_space
}

/// End the current presentation before the overlay is hidden; restoring the
/// returned token puts the hidden window back on all Spaces.
#[cfg(any(target_os = "macos", test))]
fn begin_hide(
    window: &impl OverlaySpaceWindow,
    presentations: &OverlayPresentations,
) -> RestoreToken {
    RestoreToken {
        presentation: presentations.next(),
        window: window.identity(),
    }
}

/// Restore canJoinAllSpaces unless a later show or hide, or a different
/// overlay window, has taken over since `token` was issued.
#[cfg(any(target_os = "macos", test))]
fn restore_all_spaces_if_current(
    window: &impl OverlaySpaceWindow,
    token: RestoreToken,
    presentations: &OverlayPresentations,
) -> bool {
    if !presentations.is_current(token.presentation) || window.identity() != token.window {
        return false;
    }
    window.set_collection_behavior(OVERLAY_COLLECTION_BEHAVIOR);
    true
}

/// Apply `token` to the current overlay window on the main thread.
#[cfg(target_os = "macos")]
fn restore_all_spaces_on_main(app: tauri::AppHandle, token: RestoreToken) {
    let handle = app.clone();
    let queued = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_webview_window(OVERLAY_LABEL) {
            restore_all_spaces_if_current(&MacOverlayWindow(&window), token, &PRESENTATIONS);
        }
    });
    if let Err(e) = queued {
        tracing::warn!("Could not restore the overlay's all-Spaces behavior: {e}");
    }
}

#[cfg(target_os = "macos")]
struct MacOverlayWindow<'a>(&'a tauri::WebviewWindow);

#[cfg(target_os = "macos")]
#[allow(deprecated, unexpected_cfgs)]
impl OverlaySpaceWindow for MacOverlayWindow<'_> {
    fn identity(&self) -> usize {
        self.0
            .ns_window()
            .map(|ns_window| ns_window as usize)
            .unwrap_or(0)
    }

    fn is_on_active_space(&self) -> bool {
        use cocoa::base::{id, NO};

        let ns_window: id = self.0.ns_window().unwrap() as id;
        let on_active_space: cocoa::base::BOOL =
            unsafe { objc::msg_send![ns_window, isOnActiveSpace] };
        on_active_space != NO
    }

    fn set_collection_behavior(&self, behavior: u64) {
        use cocoa::base::id;

        let ns_window: id = self.0.ns_window().unwrap() as id;
        unsafe {
            let _: () = objc::msg_send![ns_window, setCollectionBehavior: behavior];
        }
    }

    fn order_front(&self) {
        use cocoa::base::{id, nil};

        let ns_window: id = self.0.ns_window().unwrap() as id;
        unsafe {
            let _: () = objc::msg_send![ns_window, orderFront: nil];
        }
    }

    fn restore_all_spaces_later(&self, token: RestoreToken) {
        // On the main thread `run_on_main_thread` runs the task immediately,
        // which is too early; queued from a worker it runs on a later turn.
        let app = self.0.app_handle().clone();
        tauri::async_runtime::spawn(async move { restore_all_spaces_on_main(app, token) });
    }
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

        let _: () = objc::msg_send![ns_window, setCollectionBehavior: OVERLAY_COLLECTION_BEHAVIOR];

        // Transparent chrome
        ns_window.setOpaque_(NO);
        let clear_color: id = objc::msg_send![objc::class!(NSColor), clearColor];
        ns_window.setBackgroundColor_(clear_color);

        // No shadow (CSS provides its own)
        ns_window.setHasShadow_(NO);
    }
}

#[cfg(test)]
mod tests {
    use super::should_precreate_at_startup;
    use super::{
        begin_hide, present_on_active_space, restore_all_spaces_if_current, OverlayPresentations,
        OverlaySpaceWindow, RestoreToken, MOVE_TO_ACTIVE_SPACE_BEHAVIOR,
        OVERLAY_COLLECTION_BEHAVIOR,
    };
    use std::cell::{Cell, RefCell};

    const CAN_JOIN_ALL_SPACES: u64 = 1;
    const MOVE_TO_ACTIVE_SPACE: u64 = 2;
    const FULL_SCREEN_AUXILIARY: u64 = 256;

    /// Records the native calls; queued restores run only when a test runs them.
    struct FakeOverlayWindow {
        identity: usize,
        on_active_space: bool,
        behavior: Cell<u64>,
        operations: RefCell<Vec<String>>,
        queued: RefCell<Vec<RestoreToken>>,
    }

    impl Default for FakeOverlayWindow {
        fn default() -> Self {
            Self {
                identity: 1,
                on_active_space: false,
                behavior: Cell::new(OVERLAY_COLLECTION_BEHAVIOR),
                operations: RefCell::default(),
                queued: RefCell::default(),
            }
        }
    }

    impl FakeOverlayWindow {
        /// The restore the latest off-Space show queued.
        fn take_queued(&self) -> RestoreToken {
            self.queued.borrow_mut().pop().expect("a queued restore")
        }
    }

    impl OverlaySpaceWindow for FakeOverlayWindow {
        fn identity(&self) -> usize {
            self.identity
        }

        fn is_on_active_space(&self) -> bool {
            self.on_active_space
        }

        fn set_collection_behavior(&self, behavior: u64) {
            self.behavior.set(behavior);
            self.operations
                .borrow_mut()
                .push(format!("set_collection_behavior({behavior})"));
        }

        fn order_front(&self) {
            self.operations.borrow_mut().push("order_front".into());
        }

        fn restore_all_spaces_later(&self, token: RestoreToken) {
            self.queued.borrow_mut().push(token);
            self.operations
                .borrow_mut()
                .push("restore_all_spaces_later".into());
        }
    }

    #[test]
    fn overlay_on_the_active_space_is_only_ordered_front() {
        let window = FakeOverlayWindow {
            on_active_space: true,
            ..Default::default()
        };

        assert!(!present_on_active_space(
            &window,
            &OverlayPresentations::new()
        ));
        assert_eq!(*window.operations.borrow(), ["order_front"]);
    }

    #[test]
    fn overlay_left_on_another_space_is_moved_to_the_active_space() {
        let window = FakeOverlayWindow::default();
        let presentations = OverlayPresentations::new();

        assert!(present_on_active_space(&window, &presentations));
        assert_eq!(
            *window.operations.borrow(),
            [
                format!("set_collection_behavior({MOVE_TO_ACTIVE_SPACE_BEHAVIOR})"),
                "order_front".to_string(),
                "restore_all_spaces_later".to_string(),
            ]
        );

        let restore = window.take_queued();
        assert!(restore_all_spaces_if_current(
            &window,
            restore,
            &presentations
        ));
        assert_eq!(window.behavior.get(), OVERLAY_COLLECTION_BEHAVIOR);
    }

    #[test]
    fn hide_rejoins_all_spaces_and_drops_the_pending_restore() {
        let window = FakeOverlayWindow::default();
        let presentations = OverlayPresentations::new();
        present_on_active_space(&window, &presentations);
        let shown = window.take_queued();

        let hidden = begin_hide(&window, &presentations);
        assert!(restore_all_spaces_if_current(
            &window,
            hidden,
            &presentations
        ));
        assert_eq!(window.behavior.get(), OVERLAY_COLLECTION_BEHAVIOR);

        window.operations.borrow_mut().clear();
        assert!(!restore_all_spaces_if_current(
            &window,
            shown,
            &presentations
        ));
        assert!(window.operations.borrow().is_empty());
    }

    #[test]
    fn restore_from_an_earlier_show_never_undoes_a_newer_move() {
        let window = FakeOverlayWindow::default();
        let presentations = OverlayPresentations::new();
        present_on_active_space(&window, &presentations);
        let first = window.take_queued();
        let hidden = begin_hide(&window, &presentations);
        present_on_active_space(&window, &presentations);
        let second = window.take_queued();

        // The earlier show's and the hide's restores complete during the newer move.
        assert!(!restore_all_spaces_if_current(
            &window,
            first,
            &presentations
        ));
        assert!(!restore_all_spaces_if_current(
            &window,
            hidden,
            &presentations
        ));
        assert_eq!(window.behavior.get(), MOVE_TO_ACTIVE_SPACE_BEHAVIOR);

        assert!(restore_all_spaces_if_current(
            &window,
            second,
            &presentations
        ));
        assert_eq!(window.behavior.get(), OVERLAY_COLLECTION_BEHAVIOR);
    }

    #[test]
    fn restores_completing_in_reverse_order_keep_the_newest_presentation() {
        let window = FakeOverlayWindow::default();
        let presentations = OverlayPresentations::new();
        present_on_active_space(&window, &presentations);
        let first = window.take_queued();
        let hidden = begin_hide(&window, &presentations);
        present_on_active_space(&window, &presentations);
        let second = window.take_queued();

        assert!(restore_all_spaces_if_current(
            &window,
            second,
            &presentations
        ));
        window.operations.borrow_mut().clear();
        assert!(!restore_all_spaces_if_current(
            &window,
            hidden,
            &presentations
        ));
        assert!(!restore_all_spaces_if_current(
            &window,
            first,
            &presentations
        ));
        assert!(window.operations.borrow().is_empty());
        assert_eq!(window.behavior.get(), OVERLAY_COLLECTION_BEHAVIOR);
    }

    #[test]
    fn restore_for_a_replaced_overlay_window_is_ignored() {
        let old = FakeOverlayWindow::default();
        let presentations = OverlayPresentations::new();
        present_on_active_space(&old, &presentations);
        let restore = old.take_queued();

        let replacement = FakeOverlayWindow {
            identity: 2,
            ..Default::default()
        };
        assert!(!restore_all_spaces_if_current(
            &replacement,
            restore,
            &presentations
        ));
        assert!(replacement.operations.borrow().is_empty());
    }

    #[test]
    fn overlay_behaviors_follow_spaces_and_full_screen_apps() {
        for behavior in [OVERLAY_COLLECTION_BEHAVIOR, MOVE_TO_ACTIVE_SPACE_BEHAVIOR] {
            assert_ne!(behavior & FULL_SCREEN_AUXILIARY, 0, "{behavior}");
            // AppKit rejects canJoinAllSpaces together with moveToActiveSpace.
            assert_ne!(
                behavior & (CAN_JOIN_ALL_SPACES | MOVE_TO_ACTIVE_SPACE),
                CAN_JOIN_ALL_SPACES | MOVE_TO_ACTIVE_SPACE,
                "{behavior}"
            );
        }
        assert_ne!(OVERLAY_COLLECTION_BEHAVIOR & CAN_JOIN_ALL_SPACES, 0);
        assert_ne!(MOVE_TO_ACTIVE_SPACE_BEHAVIOR & MOVE_TO_ACTIVE_SPACE, 0);
        assert_eq!(
            OVERLAY_COLLECTION_BEHAVIOR & !CAN_JOIN_ALL_SPACES,
            MOVE_TO_ACTIVE_SPACE_BEHAVIOR & !MOVE_TO_ACTIVE_SPACE
        );
    }

    #[test]
    fn centres_overlay_on_screen_width() {
        assert_eq!(super::overlay_x_for_screen_width(1440.0), 610.0);
        assert_eq!(super::overlay_x_for_screen_width(1200.0), 490.0);
    }

    #[test]
    fn precreate_on_toggle_only_when_enabled_and_missing() {
        assert!(super::should_precreate_on_toggle(true, false));
        assert!(!super::should_precreate_on_toggle(true, true));
        assert!(!super::should_precreate_on_toggle(false, false));
    }

    #[test]
    fn precreates_only_when_enabled_and_supported() {
        assert!(should_precreate_at_startup(true, true));
        assert!(!should_precreate_at_startup(false, true));
        assert!(!should_precreate_at_startup(true, false));
        assert!(!should_precreate_at_startup(false, false));
    }
}
