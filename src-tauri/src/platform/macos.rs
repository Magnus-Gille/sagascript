use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use std::io;
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicI32, Ordering};

use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSWorkspace};

const ACCESSIBILITY_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Accessibility";
static DICTATION_TARGET_PID: AtomicI32 = AtomicI32::new(0);

fn own_pid() -> i32 {
    i32::try_from(std::process::id()).unwrap_or(-1)
}

pub fn frontmost_pid() -> Option<i32> {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|application| application.processIdentifier())
}

fn should_restore_dictation_target(target: i32, current: Option<i32>, own: i32) -> bool {
    target > 0 && target != own && current == Some(own)
}

/// Guard decision plus a stable, privacy-free reason string for diagnostics.
fn paste_guard_decision(target: i32, current: Option<i32>, own: i32) -> (bool, &'static str) {
    match current {
        Some(pid) if pid == own && target == own => (true, "ok_sagascript_is_original_target"),
        Some(pid) if pid == own => (false, "reject_sagascript_frontmost_but_not_original_target"),
        Some(_) => (true, "ok_other_app_frontmost"),
        None => (false, "reject_no_frontmost_app"),
    }
}

#[cfg(test)]
fn can_paste_to_frontmost(target: i32, current: Option<i32>, own: i32) -> bool {
    paste_guard_decision(target, current, own).0
}

/// Everything the paste guard looked at, captured once so the logged decision
/// is exactly the decision that was enforced.
pub struct PasteGuardCheck {
    pub valid: bool,
    pub reason: &'static str,
    pub target_pid: i32,
    pub current_pid: Option<i32>,
    pub own_pid: i32,
}

pub fn dictation_paste_guard_check() -> PasteGuardCheck {
    let target_pid = DICTATION_TARGET_PID.load(Ordering::Acquire);
    let current_pid = frontmost_pid();
    let own = own_pid();
    let (valid, reason) = paste_guard_decision(target_pid, current_pid, own);
    PasteGuardCheck { valid, reason, target_pid, current_pid, own_pid: own }
}

fn bundle_id_for_pid(pid: i32) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .and_then(|app| app.bundleIdentifier())
        .map(|id| id.to_string())
}

/// Diagnostic snapshot for issue #256. Privacy: only bundle identifiers, pids,
/// booleans and Sagascript window labels. `windows` maps label to NSWindow
/// pointer. AppKit state is read only on the main thread.
pub fn focus_snapshot(windows: &[(String, usize)]) -> serde_json::Value {
    use objc2_app_kit::NSApplication;
    use objc2_foundation::MainThreadMarker;

    let front = NSWorkspace::sharedWorkspace().frontmostApplication();
    let front_pid = front.as_ref().map(|app| app.processIdentifier());
    let front_bundle = front
        .as_ref()
        .and_then(|app| app.bundleIdentifier())
        .map(|id| id.to_string());
    let target = DICTATION_TARGET_PID.load(Ordering::Acquire);
    let mut snapshot = serde_json::json!({
        "frontmostPid": front_pid,
        "frontmostBundleId": front_bundle,
        "ownPid": own_pid(),
        "dictationTargetPid": target,
        "dictationTargetBundleId": bundle_id_for_pid(target),
    });
    let map = snapshot.as_object_mut().expect("object");
    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        let key = app.keyWindow().map(|w| (&*w as *const objc2_app_kit::NSWindow) as usize);
        let key_label = key.map(|ptr| {
            windows
                .iter()
                .find(|(_, p)| *p == ptr)
                .map(|(label, _)| label.clone())
                .unwrap_or_else(|| "other".to_string())
        });
        map.insert("appActive".into(), app.isActive().into());
        map.insert("keyWindow".into(), key_label.into());
        map.insert("activationPolicy".into(), format!("{:?}", app.activationPolicy()).into());
    } else {
        map.insert("appActive".into(), serde_json::Value::Null);
        map.insert("keyWindow".into(), serde_json::Value::Null);
    }
    snapshot
}

/// Preserve the editor seen at hotkey-down before recording setup or the first
/// overlay WebView can activate Sagascript.
pub fn remember_dictation_target(target: Option<i32>) {
    let target = target.unwrap_or(0);
    DICTATION_TARGET_PID.store(target, Ordering::Release);
}

/// Restore only when Sagascript itself took focus. A deliberate switch to a
/// different app during dictation remains the user's chosen paste destination.
pub fn restore_dictation_target_if_stolen() -> Result<bool, String> {
    let target = DICTATION_TARGET_PID.load(Ordering::Acquire);
    if !should_restore_dictation_target(target, frontmost_pid(), own_pid()) {
        return Ok(false);
    }
    let target_app = NSRunningApplication::runningApplicationWithProcessIdentifier(target)
        .ok_or_else(|| "The previous paste target has closed".to_string())?;
    if !target_app.activateWithOptions(NSApplicationActivationOptions::empty()) {
        return Err("Could not restore the previous paste target".to_string());
    }
    Ok(true)
}

/// Result of the restore-before-paste step, for diagnostics.
#[derive(Debug, PartialEq, Eq)]
pub struct RestoreBeforePaste {
    /// A re-activation of the target was requested.
    pub attempted: bool,
    /// The target was frontmost when the wait ended (only meaningful if attempted).
    pub restored: bool,
    pub attempts: u32,
    pub wait_ms: u64,
    pub reason: &'static str,
}

pub const RESTORE_POLL_INTERVAL_MS: u64 = 20;
pub const RESTORE_MAX_WAIT_MS: u64 = 400;

/// Decide and run the restore-before-paste step with injected primitives so the
/// decision logic is testable. Never pastes; the caller still runs the guard.
/// Restores only when Sagascript is frontmost, was not the original target, and
/// the remembered target is a different, still-running app.
pub fn restore_target_before_paste(
    target: i32,
    own: i32,
    mut frontmost: impl FnMut() -> Option<i32>,
    target_running: impl Fn(i32) -> bool,
    mut activate: impl FnMut(i32) -> bool,
    mut sleep: impl FnMut(std::time::Duration),
) -> RestoreBeforePaste {
    let skip = |reason| RestoreBeforePaste { attempted: false, restored: false, attempts: 0, wait_ms: 0, reason };
    if !should_restore_dictation_target(target, frontmost(), own) {
        return skip("not_needed");
    }
    if !target_running(target) {
        return skip("target_not_running");
    }
    if !activate(target) {
        return RestoreBeforePaste { attempted: true, restored: false, attempts: 0, wait_ms: 0, reason: "activate_refused" };
    }
    let mut attempts = 0u32;
    let mut waited = 0u64;
    loop {
        if frontmost() == Some(target) {
            return RestoreBeforePaste { attempted: true, restored: true, attempts, wait_ms: waited, reason: "target_frontmost" };
        }
        if waited >= RESTORE_MAX_WAIT_MS {
            return RestoreBeforePaste { attempted: true, restored: false, attempts, wait_ms: waited, reason: "timed_out" };
        }
        sleep(std::time::Duration::from_millis(RESTORE_POLL_INTERVAL_MS));
        waited += RESTORE_POLL_INTERVAL_MS;
        attempts += 1;
    }
}

pub fn dictation_target_pid() -> i32 {
    DICTATION_TARGET_PID.load(Ordering::Acquire)
}

pub fn current_pid() -> i32 {
    own_pid()
}

pub fn target_is_running(pid: i32) -> bool {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .map(|app| !app.isTerminated())
        .unwrap_or(false)
}

/// Ask the target to become active. `activateWithOptions(empty)` is the
/// non-forcing request: it does not use the deprecated
/// `ActivateIgnoringOtherApps`, and works because Sagascript is the active app
/// and may yield activation. Must run on the main thread.
pub fn activate_dictation_target(pid: i32) -> bool {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .map(|app| app.activateWithOptions(NSApplicationActivationOptions::empty()))
        .unwrap_or(false)
}

static LAUNCH_ACTIVATION_PENDING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// What to do when an app activation notification arrives while the launch
/// activation request is still unapplied.
#[derive(Debug, PartialEq, Eq)]
enum LaunchActivationEvent {
    Ignore,
    Applied,
    Superseded,
}

fn launch_activation_event(pending: bool, frontmost: Option<i32>, own: i32) -> LaunchActivationEvent {
    match (pending, frontmost) {
        (false, _) | (_, None) => LaunchActivationEvent::Ignore,
        (true, Some(pid)) if pid == own => LaunchActivationEvent::Applied,
        (true, Some(_)) => LaunchActivationEvent::Superseded,
    }
}

/// Launch-time activation for the initial Settings window. macOS defers the
/// request for an accessory app and applies it when the first new window is
/// ordered front, which can be seconds later, after the user has moved to
/// another app (issue #256). Request it, then watch workspace activations: if
/// another app becomes frontmost before ours was applied, the request is stale,
/// so `deactivate` withdraws it. Must run on the main thread.
pub fn activate_app_for_launch() {
    use block2::RcBlock;
    use objc2_app_kit::{NSApplication, NSWorkspaceDidActivateApplicationNotification};
    use objc2_foundation::{MainThreadMarker, NSNotification};
    use std::ptr::NonNull;

    static OBSERVER: std::sync::Once = std::sync::Once::new();
    if NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|app| app.processIdentifier())
        == Some(own_pid())
    {
        return; // already frontmost: nothing to request, nothing can go stale
    }
    LAUNCH_ACTIVATION_PENDING.store(true, Ordering::Release);
    OBSERVER.call_once(|| {
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        let block = RcBlock::new(move |_n: NonNull<NSNotification>| {
            let pending = LAUNCH_ACTIVATION_PENDING.load(Ordering::Acquire);
            match launch_activation_event(pending, frontmost_pid(), own_pid()) {
                LaunchActivationEvent::Ignore => {}
                LaunchActivationEvent::Applied => {
                    LAUNCH_ACTIVATION_PENDING.store(false, Ordering::Release);
                }
                LaunchActivationEvent::Superseded => {
                    LAUNCH_ACTIVATION_PENDING.store(false, Ordering::Release);
                    if let Some(mtm) = MainThreadMarker::new() {
                        NSApplication::sharedApplication(mtm).deactivate();
                    }
                    crate::focus_diag::activation("launch_activation", "withdrawn_user_moved_on");
                }
            }
        });
        // SAFETY: valid notification constant; None queue runs on the posting
        // thread (main for NSWorkspace); the center retains the block.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidActivateApplicationNotification),
                None,
                None,
                &block,
            )
        };
        std::mem::forget(token);
    });
    activate_app();
}

extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: core_foundation::base::CFTypeRef) -> bool;
}

/// Check if the process has accessibility (AX) permissions
pub fn is_accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

fn accessibility_settings_command() -> Command {
    let mut command = Command::new("open");
    command.arg(ACCESSIBILITY_SETTINGS_URL);
    command
}

fn accessibility_request_prompts_user() -> bool {
    false
}

fn interpret_open_result(result: io::Result<ExitStatus>) -> Result<(), String> {
    let status =
        result.map_err(|error| format!("Failed to open Accessibility settings: {error}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Failed to open Accessibility settings: open exited with {status}"
        ))
    }
}

/// Bring the Accessibility pane forward without asking macOS to show its
/// permission prompt again. This is used when onboarding is already polling
/// and the user only needs to return to the correct System Settings pane.
pub fn open_accessibility_settings() -> Result<(), String> {
    interpret_open_result(accessibility_settings_command().status())
}

/// Register the Accessibility check without showing macOS's redundant native
/// alert, then bring the relevant System Settings pane forward directly. The
/// onboarding UI already explains why access is needed and keeps an explicit
/// button available if the user navigates away from the pane.
pub fn request_accessibility_permission() -> Result<(), String> {
    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let value = if accessibility_request_prompts_user() {
        CFBoolean::true_value()
    } else {
        CFBoolean::false_value()
    };
    let options = CFDictionary::from_CFType_pairs(&[(key, value)]);

    unsafe {
        AXIsProcessTrustedWithOptions(options.as_CFTypeRef());
    }

    open_accessibility_settings()
}

/// Set the app as an accessory (no dock icon)
#[allow(deprecated)]
pub fn set_activation_policy_accessory() {
    use cocoa::appkit::{NSApp, NSApplication, NSApplicationActivationPolicy};
    unsafe {
        let app = NSApp();
        app.setActivationPolicy_(
            NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
        );
    }
}

/// Bring the accessory application to the foreground before presenting a
/// user-requested window. Accessory apps do not reliably activate when a
/// window merely calls `show`/`set_focus`.
#[allow(deprecated)]
pub fn activate_app() {
    use cocoa::appkit::{NSApp, NSApplication};
    use cocoa::base::YES;

    unsafe {
        NSApp().activateIgnoringOtherApps_(YES);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        accessibility_request_prompts_user, accessibility_settings_command, interpret_open_result,
        can_paste_to_frontmost, should_restore_dictation_target, ACCESSIBILITY_SETTINGS_URL,
    };
    use std::ffi::OsStr;
    use std::io;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    #[test]
    fn focus_repair_only_runs_when_sagascript_took_the_target_from_another_app() {
        assert!(should_restore_dictation_target(101, Some(202), 202));
        assert!(!should_restore_dictation_target(101, Some(303), 202));
        assert!(!should_restore_dictation_target(202, Some(202), 202));
        assert!(!should_restore_dictation_target(0, Some(202), 202));
    }

    #[test]
    fn restore_before_paste_decision_matrix() {
        use super::{restore_target_before_paste, RestoreBeforePaste};
        use std::cell::Cell;
        let run = |target: i32, front_seq: Vec<Option<i32>>, running: bool, accepts: bool| {
            let i = Cell::new(0usize);
            let activated = Cell::new(false);
            let r = restore_target_before_paste(
                target,
                202,
                || {
                    let k = i.get();
                    i.set(k + 1);
                    // sequence entries after activation; first call is the decision
                    front_seq.get(k).copied().unwrap_or(*front_seq.last().unwrap())
                },
                |_| running,
                |_| {
                    activated.set(true);
                    accepts
                },
                |_| {},
            );
            (r, activated.get())
        };
        // Other app already frontmost: no restore.
        let (r, a) = run(101, vec![Some(303)], true, true);
        assert!(!a && !r.attempted && r.reason == "not_needed");
        // Sagascript was the original target: no restore.
        let (r, a) = run(202, vec![Some(202)], true, true);
        assert!(!a && !r.attempted);
        // No remembered target.
        let (r, a) = run(0, vec![Some(202)], true, true);
        assert!(!a && !r.attempted);
        // Target gone.
        let (r, a) = run(101, vec![Some(202)], false, true);
        assert!(!a && r.reason == "target_not_running");
        // Stolen, target comes back after two polls.
        let (r, a) = run(101, vec![Some(202), Some(202), Some(202), Some(101)], true, true);
        assert!(a);
        assert_eq!(
            r,
            RestoreBeforePaste { attempted: true, restored: true, attempts: 2, wait_ms: 40, reason: "target_frontmost" }
        );
        // Activation refused.
        let (r, _) = run(101, vec![Some(202)], true, false);
        assert!(r.attempted && !r.restored && r.reason == "activate_refused");
        // Never comes back: bounded, then guard will reject.
        let (r, _) = run(101, vec![Some(202)], true, true);
        assert!(r.attempted && !r.restored && r.reason == "timed_out" && r.wait_ms == 400 && r.attempts == 20);
        // Some other app (not the target) appears: not restored, guard decides.
        let (r, _) = run(101, vec![Some(202), Some(303)], true, true);
        assert!(!r.restored);
    }

    #[test]
    fn restored_target_passes_guard_and_unrestored_is_rejected() {
        use super::paste_guard_decision;
        assert!(paste_guard_decision(101, Some(101), 202).0);
        assert!(!paste_guard_decision(101, Some(202), 202).0);
    }

    #[test]
    fn launch_activation_is_withdrawn_only_when_another_app_takes_over() {
        use super::{launch_activation_event as e, LaunchActivationEvent as E};
        assert_eq!(e(false, Some(303), 202), E::Ignore);
        assert_eq!(e(true, None, 202), E::Ignore);
        assert_eq!(e(true, Some(202), 202), E::Applied);
        assert_eq!(e(true, Some(303), 202), E::Superseded);
    }

    #[test]
    fn paste_guard_reason_names_the_failed_condition() {
        use super::paste_guard_decision;
        assert_eq!(paste_guard_decision(101, Some(303), 202), (true, "ok_other_app_frontmost"));
        assert_eq!(paste_guard_decision(202, Some(202), 202), (true, "ok_sagascript_is_original_target"));
        assert_eq!(
            paste_guard_decision(101, Some(202), 202),
            (false, "reject_sagascript_frontmost_but_not_original_target")
        );
        assert_eq!(paste_guard_decision(101, None, 202), (false, "reject_no_frontmost_app"));
    }

    #[test]
    fn paste_guard_rejects_own_stolen_focus_but_allows_an_original_own_target() {
        assert!(!can_paste_to_frontmost(101, Some(202), 202));
        assert!(can_paste_to_frontmost(202, Some(202), 202));
        assert!(can_paste_to_frontmost(101, Some(303), 202));
        assert!(!can_paste_to_frontmost(0, Some(202), 202));
        assert!(!can_paste_to_frontmost(101, None, 202));
    }

    #[test]
    fn initial_accessibility_request_does_not_show_a_redundant_native_prompt() {
        assert!(!accessibility_request_prompts_user());
    }

    #[test]
    fn accessibility_request_opens_the_privacy_pane() {
        let command = accessibility_settings_command();

        assert_eq!(command.get_program(), OsStr::new("open"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![OsStr::new(ACCESSIBILITY_SETTINGS_URL)]
        );
    }

    #[test]
    fn accessibility_settings_launch_failure_is_reported() {
        let error = interpret_open_result(Err(io::Error::new(
            io::ErrorKind::NotFound,
            "open is unavailable",
        )))
        .unwrap_err();

        assert!(error.contains("open is unavailable"));
    }

    #[test]
    fn accessibility_settings_nonzero_exit_is_reported() {
        let error = interpret_open_result(Ok(ExitStatus::from_raw(1 << 8))).unwrap_err();

        assert!(error.contains("open exited with"));
    }

    #[test]
    fn accessibility_settings_success_is_accepted() {
        assert!(interpret_open_result(Ok(ExitStatus::from_raw(0))).is_ok());
    }
}

/// Run `on_wake` whenever the system wakes from sleep (`NSWorkspace`
/// `didWakeNotification`). The observer lives for the rest of the process. The
/// callback runs on the posting thread, so it must be cheap and non-blocking.
pub fn observe_system_wake(on_wake: impl Fn() + Send + Sync + 'static) {
    use block2::RcBlock;
    use objc2_app_kit::NSWorkspaceDidWakeNotification;
    use objc2_foundation::NSNotification;
    use std::ptr::NonNull;

    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    let block = RcBlock::new(move |_notification: NonNull<NSNotification>| on_wake());
    // SAFETY: the notification name is a valid constant, a `None` queue means
    // the block runs on the posting thread, and the block is `Send + Sync`.
    let token = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceDidWakeNotification),
            None,
            None,
            &block,
        )
    };
    // The center retains the block; the token must outlive the process.
    std::mem::forget(token);
}

mod memory_pressure {
    use std::ffi::c_void;
    use std::os::raw::c_ulong;

    const DISPATCH_MEMORYPRESSURE_WARN: c_ulong = 0x2;
    const DISPATCH_MEMORYPRESSURE_CRITICAL: c_ulong = 0x4;

    unsafe extern "C" {
        static _dispatch_source_type_memorypressure: c_void;
        fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *mut c_void;
        fn dispatch_source_create(
            kind: *const c_void,
            handle: usize,
            mask: c_ulong,
            queue: *mut c_void,
        ) -> *mut c_void;
        fn dispatch_set_context(object: *mut c_void, context: *mut c_void);
        fn dispatch_source_set_event_handler_f(
            source: *mut c_void,
            handler: extern "C" fn(*mut c_void),
        );
        fn dispatch_source_get_data(source: *mut c_void) -> c_ulong;
        fn dispatch_resume(object: *mut c_void);
    }

    static HANDLER: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>> = std::sync::OnceLock::new();

    extern "C" fn on_event(source: *mut c_void) {
        // SAFETY: `source` is the live dispatch source this handler was set on
        // (its context points at itself) and is never released.
        let flags = unsafe { dispatch_source_get_data(source) };
        if flags & (DISPATCH_MEMORYPRESSURE_WARN | DISPATCH_MEMORYPRESSURE_CRITICAL) != 0 {
            if let Some(handler) = HANDLER.get() {
                handler();
            }
        }
    }

    pub fn observe(handler: impl Fn() + Send + Sync + 'static) {
        if HANDLER.set(Box::new(handler)).is_err() {
            return;
        }
        // SAFETY: plain libdispatch calls. The source is created on the global
        // default queue, never released (process lifetime), and its context is
        // itself so the C handler can read the event data.
        unsafe {
            let queue = dispatch_get_global_queue(0, 0);
            let source = dispatch_source_create(
                std::ptr::addr_of!(_dispatch_source_type_memorypressure),
                0,
                DISPATCH_MEMORYPRESSURE_WARN | DISPATCH_MEMORYPRESSURE_CRITICAL,
                queue,
            );
            if source.is_null() {
                tracing::warn!("could not create the memory-pressure dispatch source");
                return;
            }
            dispatch_set_context(source, source);
            dispatch_source_set_event_handler_f(source, on_event);
            dispatch_resume(source);
        }
    }
}

/// Run `on_pressure` on the system's memory-pressure warning or critical
/// events (libdispatch memory-pressure source). Runs on a libdispatch worker
/// thread for the rest of the process; first registration wins.
pub fn observe_memory_pressure(on_pressure: impl Fn() + Send + Sync + 'static) {
    memory_pressure::observe(on_pressure);
}
