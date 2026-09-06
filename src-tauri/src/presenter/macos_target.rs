//! Read-only macOS Accessibility target observation for Presenter Mode.
//!
//! This adapter deliberately contains no AX setter, key event, clipboard, or
//! permission-prompt path. All native entry points are main-thread-only and
//! every native failure is reduced to a content-free `TargetError`.

use core_foundation::base::{CFEqual, CFGetTypeID, CFRelease, CFTypeRef, TCFType};
use core_foundation::runloop::{
    kCFRunLoopDefaultMode, CFRunLoopAddSource, CFRunLoopGetCurrent, CFRunLoopGetMain, CFRunLoopRef,
    CFRunLoopRemoveSource, CFRunLoopSourceRef,
};
use core_foundation::string::{CFString, CFStringGetCharacters, CFStringGetLength, CFStringRef};
use objc::runtime::{Class, Object};
use objc::{class, msg_send, sel, sel_impl};
use std::ffi::{c_char, c_void, CStr};
use std::marker::PhantomData;
use std::ptr;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

type AXUIElementRef = *const c_void;
type AXObserverRef = *const c_void;
type AXValueRef = *const c_void;
type AXValueType = u32;
type AXError = i32;
type Pid = i32;

const AX_SUCCESS: AXError = 0;
const AX_ERROR_ATTRIBUTE_UNSUPPORTED: AXError = -25205;
const AX_ERROR_API_DISABLED: AXError = -25211;
const AX_ERROR_NO_VALUE: AXError = -25212;
const AX_VALUE_CF_RANGE_TYPE: AXValueType = 4;
const MAX_VALUE_UTF8_BYTES: usize = 32_768;
const MAX_BUNDLE_ID_BYTES: usize = 512;
const MESSAGING_TIMEOUT_SECONDS: f32 = 0.1;

static NEXT_SESSION_TOKEN: AtomicUsize = AtomicUsize::new(1);
static LAST_INVALIDATED_TOKEN: AtomicUsize = AtomicUsize::new(0);

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut Pid) -> AXError;
    fn AXUIElementGetTypeID() -> usize;
    fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, timeout: f32) -> AXError;
    fn AXObserverCreate(
        application: Pid,
        callback: extern "C" fn(AXObserverRef, AXUIElementRef, CFStringRef, *mut c_void),
        observer: *mut AXObserverRef,
    ) -> AXError;
    fn AXObserverAddNotification(
        observer: AXObserverRef,
        element: AXUIElementRef,
        notification: CFStringRef,
        refcon: *mut c_void,
    ) -> AXError;
    fn AXObserverRemoveNotification(
        observer: AXObserverRef,
        element: AXUIElementRef,
        notification: CFStringRef,
    ) -> AXError;
    fn AXObserverGetRunLoopSource(observer: AXObserverRef) -> CFRunLoopSourceRef;
    fn AXValueGetTypeID() -> usize;
    fn AXValueGetType(value: AXValueRef) -> AXValueType;
    fn AXValueGetValue(value: AXValueRef, value_type: AXValueType, value_out: *mut CFRange)
        -> bool;
}

#[link(name = "AppKit", kind = "framework")]
extern "C" {}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CFRange {
    location: isize,
    length: isize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetError {
    MainThreadRequired,
    AccessibilityUnavailable,
    NativeFailure,
    MessagingTimeout,
    AttributeUnavailable,
    AttributeTypeMismatch,
    InvalidRole,
    SecureField,
    InvalidValue,
    ValueTooLarge,
    InvalidSelection,
    AppIdentityUnavailable,
    AppIdentityMismatch,
    ObserverUnavailable,
    ObserverRegistrationFailed,
    ObserverRunLoopUnavailable,
    SessionTokenExhausted,
    TargetChanged,
}

fn map_ax_error(error: AXError) -> TargetError {
    match error {
        AX_ERROR_API_DISABLED => TargetError::AccessibilityUnavailable,
        AX_ERROR_ATTRIBUTE_UNSUPPORTED | AX_ERROR_NO_VALUE => TargetError::AttributeUnavailable,
        _ => TargetError::NativeFailure,
    }
}

fn require_main_thread() -> Result<(), TargetError> {
    unsafe {
        let current = CFRunLoopGetCurrent();
        let main = CFRunLoopGetMain();
        let thread_class: *const Class = class!(NSThread);
        let main_thread: bool = msg_send![thread_class, isMainThread];
        let current_is_main = !current.is_null() && !main.is_null() && current == main;
        if !main_thread_and_runloop_are_valid(current_is_main, main_thread) {
            return Err(TargetError::MainThreadRequired);
        }
    }
    Ok(())
}

fn main_thread_and_runloop_are_valid(current_is_main: bool, main_thread: bool) -> bool {
    current_is_main && main_thread
}

fn cf_name(value: &'static str) -> CFString {
    CFString::from_static_string(value)
}

fn attribute_focused_application() -> CFString {
    cf_name("AXFocusedApplication")
}

fn attribute_focused_element() -> CFString {
    cf_name("AXFocusedUIElement")
}

fn attribute_role() -> CFString {
    cf_name("AXRole")
}

fn attribute_subrole() -> CFString {
    cf_name("AXSubrole")
}

fn attribute_value() -> CFString {
    cf_name("AXValue")
}

fn notification_focused_element() -> CFString {
    cf_name("AXFocusedUIElementChanged")
}

fn notification_deactivated() -> CFString {
    cf_name("AXApplicationDeactivated")
}

fn notification_focused_window() -> CFString {
    cf_name("AXFocusedWindowChanged")
}

fn validate_editable_role(role: &str, subrole: &str) -> Result<(), TargetError> {
    if subrole == "AXSecureTextField" {
        return Err(TargetError::SecureField);
    }
    if matches!(role, "AXTextField" | "AXTextArea") {
        Ok(())
    } else {
        Err(TargetError::InvalidRole)
    }
}

fn validate_bundle_id(value: &str) -> Result<(), TargetError> {
    if value.is_empty() || value.len() > MAX_BUNDLE_ID_BYTES || value.chars().any(char::is_control)
    {
        Err(TargetError::AppIdentityUnavailable)
    } else {
        Ok(())
    }
}

fn capture_revalidation_succeeds(snapshot_matches: bool, token_valid: bool) -> bool {
    snapshot_matches && token_valid
}

fn decode_utf16_bounded(units: &[u16], max_utf8_bytes: usize) -> Result<String, TargetError> {
    let value = String::from_utf16(units).map_err(|_| TargetError::InvalidValue)?;
    if value.len() > max_utf8_bytes {
        return Err(TargetError::ValueTooLarge);
    }
    Ok(value)
}

fn validate_selection_range(
    units: &[u16],
    location: isize,
    length: isize,
) -> Result<(usize, usize), TargetError> {
    if location < 0 || length < 0 {
        return Err(TargetError::InvalidSelection);
    }
    let start = usize::try_from(location).map_err(|_| TargetError::InvalidSelection)?;
    let count = usize::try_from(length).map_err(|_| TargetError::InvalidSelection)?;
    let end = start
        .checked_add(count)
        .ok_or(TargetError::InvalidSelection)?;
    if end > units.len() {
        return Err(TargetError::InvalidSelection);
    }
    if is_surrogate_boundary(units, start) || is_surrogate_boundary(units, end) {
        return Err(TargetError::InvalidSelection);
    }
    Ok((start, count))
}

fn is_surrogate_boundary(units: &[u16], boundary: usize) -> bool {
    boundary > 0
        && boundary < units.len()
        && (0xDC00..=0xDFFF).contains(&units[boundary])
        && (0xD800..=0xDBFF).contains(&units[boundary - 1])
}

struct OwnedCf(CFTypeRef);

impl OwnedCf {
    fn new(value: CFTypeRef) -> Result<Self, TargetError> {
        if value.is_null() {
            Err(TargetError::AttributeUnavailable)
        } else {
            Ok(Self(value))
        }
    }

    fn as_ptr(&self) -> CFTypeRef {
        self.0
    }
}

impl Drop for OwnedCf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

fn copy_attribute(element: AXUIElementRef, attribute: &CFString) -> Result<OwnedCf, TargetError> {
    let mut value: CFTypeRef = ptr::null();
    let error = unsafe {
        AXUIElementCopyAttributeValue(element, attribute.as_concrete_TypeRef(), &mut value)
    };
    if error != AX_SUCCESS {
        return Err(map_ax_error(error));
    }
    OwnedCf::new(value)
}

// Only the optional subrole may be absent; a successful call with a null
// value or a transport/permission failure is not proof of absence.
fn optional_subrole_present(error: AXError, has_value: bool) -> Result<bool, TargetError> {
    if matches!(error, AX_ERROR_ATTRIBUTE_UNSUPPORTED | AX_ERROR_NO_VALUE) && !has_value {
        return Ok(false);
    }
    if error != AX_SUCCESS {
        return Err(map_ax_error(error));
    }
    if !has_value { return Err(TargetError::InvalidValue); }
    Ok(true)
}

fn copy_subrole(element: AXUIElementRef) -> Result<String, TargetError> {
    let mut value: CFTypeRef = ptr::null();
    let error = unsafe {
        AXUIElementCopyAttributeValue(element, attribute_subrole().as_concrete_TypeRef(), &mut value)
    };
    // Retain ownership even for an unexpected non-null error result.
    let owned = if value.is_null() { None } else { Some(OwnedCf(value)) };
    if !optional_subrole_present(error, owned.is_some())? {
        return Ok(String::new());
    }
    let value = owned.ok_or(TargetError::InvalidValue)?;
    read_cf_string(value.as_ptr(), MAX_BUNDLE_ID_BYTES).map(|(text, _)| text)
}

fn require_ax_element(value: CFTypeRef) -> Result<(), TargetError> {
    let actual = unsafe { CFGetTypeID(value) };
    if actual == unsafe { AXUIElementGetTypeID() } {
        Ok(())
    } else {
        Err(TargetError::AttributeTypeMismatch)
    }
}

fn set_messaging_timeout(element: AXUIElementRef) -> Result<(), TargetError> {
    if unsafe { AXUIElementSetMessagingTimeout(element, MESSAGING_TIMEOUT_SECONDS) } != AX_SUCCESS {
        Err(TargetError::MessagingTimeout)
    } else {
        Ok(())
    }
}

fn read_cf_string(
    value: CFTypeRef,
    max_utf8_bytes: usize,
) -> Result<(String, Vec<u16>), TargetError> {
    if unsafe { CFGetTypeID(value) } != CFString::type_id() {
        return Err(TargetError::AttributeTypeMismatch);
    }
    let string_ref = value as CFStringRef;
    let length = unsafe { CFStringGetLength(string_ref) };
    if length < 0
        || usize::try_from(length).map_err(|_| TargetError::ValueTooLarge)? > max_utf8_bytes
    {
        return Err(TargetError::ValueTooLarge);
    }
    let mut units = vec![0u16; length as usize];
    if length > 0 {
        unsafe {
            CFStringGetCharacters(
                string_ref,
                core_foundation::base::CFRange {
                    location: 0,
                    length,
                },
                units.as_mut_ptr(),
            );
        }
    }
    let decoded = decode_utf16_bounded(&units, max_utf8_bytes)?;
    Ok((decoded, units))
}

fn read_range(value: &OwnedCf, units: &[u16]) -> Result<(usize, usize), TargetError> {
    if unsafe { CFGetTypeID(value.as_ptr()) } != unsafe { AXValueGetTypeID() }
        || unsafe { AXValueGetType(value.as_ptr() as AXValueRef) } != AX_VALUE_CF_RANGE_TYPE
    {
        return Err(TargetError::AttributeTypeMismatch);
    }
    let mut range = CFRange {
        location: 0,
        length: 0,
    };
    let valid = unsafe {
        AXValueGetValue(
            value.as_ptr() as AXValueRef,
            AX_VALUE_CF_RANGE_TYPE,
            &mut range,
        )
    };
    if !valid {
        return Err(TargetError::InvalidSelection);
    }
    validate_selection_range(units, range.location, range.length)
}

fn read_value_and_selection(field: AXUIElementRef) -> Result<(String, usize, usize), TargetError> {
    let value = copy_attribute(field, &attribute_value())?;
    let (text, units) = read_cf_string(value.as_ptr(), MAX_VALUE_UTF8_BYTES)?;
    let selected_range = copy_attribute(field, &cf_name("AXSelectedTextRange"))?;
    let (location, length) = read_range(&selected_range, &units)?;
    Ok((text, location, length))
}

fn frontmost_application_identity() -> Result<(Pid, String), TargetError> {
    let pool: *mut Object = unsafe { msg_send![class!(NSAutoreleasePool), new] };
    if pool.is_null() {
        return Err(TargetError::AppIdentityUnavailable);
    }
    let result = unsafe {
        let workspace: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
        let application: *mut Object = if workspace.is_null() {
            ptr::null_mut()
        } else {
            msg_send![workspace, frontmostApplication]
        };
        if application.is_null() {
            Err(TargetError::AppIdentityUnavailable)
        } else {
            let pid: Pid = msg_send![application, processIdentifier];
            let bundle: *mut Object = msg_send![application, bundleIdentifier];
            if pid <= 0 || bundle.is_null() {
                Err(TargetError::AppIdentityUnavailable)
            } else {
                let byte_len: usize = msg_send![bundle, lengthOfBytesUsingEncoding: 4usize];
                if byte_len == 0 || byte_len > MAX_BUNDLE_ID_BYTES {
                    Err(TargetError::AppIdentityUnavailable)
                } else {
                    let utf8: *const c_char = msg_send![bundle, UTF8String];
                    if utf8.is_null() {
                        Err(TargetError::AppIdentityUnavailable)
                    } else {
                        CStr::from_ptr(utf8)
                            .to_str()
                            .map_err(|_| TargetError::AppIdentityUnavailable)
                            .and_then(|value| {
                                validate_bundle_id(value)?;
                                Ok((pid, value.to_owned()))
                            })
                    }
                }
            }
        }
    };
    unsafe {
        let _: () = msg_send![pool, drain];
    }
    result
}

struct CurrentTarget {
    application: OwnedCf,
    field: OwnedCf,
    pid: Pid,
    app_id: String,
}

fn current_target() -> Result<CurrentTarget, TargetError> {
    require_main_thread()?;
    let system = OwnedCf::new(unsafe { AXUIElementCreateSystemWide() })?;
    require_ax_element(system.as_ptr())?;
    set_messaging_timeout(system.as_ptr() as AXUIElementRef)?;
    let focused_app = copy_attribute(
        system.as_ptr() as AXUIElementRef,
        &attribute_focused_application(),
    )?;
    require_ax_element(focused_app.as_ptr())?;
    let mut pid: Pid = 0;
    let pid_error = unsafe { AXUIElementGetPid(focused_app.as_ptr() as AXUIElementRef, &mut pid) };
    if pid_error != AX_SUCCESS || pid <= 0 {
        return Err(if pid_error == AX_ERROR_API_DISABLED {
            TargetError::AccessibilityUnavailable
        } else {
            TargetError::AppIdentityUnavailable
        });
    }
    set_messaging_timeout(focused_app.as_ptr() as AXUIElementRef)?;
    let focused_field = copy_attribute(
        focused_app.as_ptr() as AXUIElementRef,
        &attribute_focused_element(),
    )?;
    require_ax_element(focused_field.as_ptr())?;
    set_messaging_timeout(focused_field.as_ptr() as AXUIElementRef)?;
    let (role, _) = read_cf_string(
        copy_attribute(focused_field.as_ptr() as AXUIElementRef, &attribute_role())?.as_ptr(),
        MAX_BUNDLE_ID_BYTES,
    )?;
    let subrole = copy_subrole(focused_field.as_ptr() as AXUIElementRef)?;
    validate_editable_role(&role, &subrole)?;
    let (frontmost_pid, app_id) = frontmost_application_identity()?;
    if frontmost_pid != pid {
        return Err(TargetError::AppIdentityMismatch);
    }
    Ok(CurrentTarget {
        application: focused_app,
        field: focused_field,
        pid,
        app_id,
    })
}

extern "C" fn observer_callback(
    _observer: AXObserverRef,
    _element: AXUIElementRef,
    _notification: CFStringRef,
    refcon: *mut c_void,
) {
    // The refcon is an opaque monotonic token, never a dereferenced pointer.
    let token = refcon as usize;
    if token != 0 {
        LAST_INVALIDATED_TOKEN.fetch_max(token, Ordering::Release);
    }
}

struct NotificationRegistration {
    element: AXUIElementRef,
    name: CFString,
}

struct ObserverResources {
    observer: AXObserverRef,
    run_loop: CFRunLoopRef,
    source: CFRunLoopSourceRef,
    source_added: bool,
    registered: Vec<NotificationRegistration>,
}

impl ObserverResources {
    fn new(application: AXUIElementRef, pid: Pid, token: usize) -> Result<Self, TargetError> {
        let run_loop = unsafe { CFRunLoopGetCurrent() };
        if run_loop.is_null() {
            return Err(TargetError::ObserverRunLoopUnavailable);
        }
        let mut observer = ptr::null();
        let create_error = unsafe { AXObserverCreate(pid, observer_callback, &mut observer) };
        if create_error != AX_SUCCESS || observer.is_null() {
            return Err(TargetError::ObserverUnavailable);
        }
        let mut resources = Self {
            observer,
            run_loop,
            source: ptr::null_mut(),
            source_added: false,
            registered: Vec::new(),
        };
        let refcon = token as *mut c_void;
        for name in [
            notification_deactivated(),
            notification_focused_element(),
            notification_focused_window(),
        ] {
            let error = unsafe {
                AXObserverAddNotification(
                    resources.observer,
                    application,
                    name.as_concrete_TypeRef(),
                    refcon,
                )
            };
            if error != AX_SUCCESS {
                return Err(TargetError::ObserverRegistrationFailed);
            }
            resources.registered.push(NotificationRegistration {
                element: application,
                name,
            });
        }
        resources.source = unsafe { AXObserverGetRunLoopSource(resources.observer) };
        if resources.source.is_null() {
            return Err(TargetError::ObserverRunLoopUnavailable);
        }
        unsafe {
            CFRunLoopAddSource(resources.run_loop, resources.source, kCFRunLoopDefaultMode);
        }
        resources.source_added = true;
        Ok(resources)
    }

    fn teardown(&mut self) {
        for registration in self.registered.drain(..) {
            unsafe {
                AXObserverRemoveNotification(
                    self.observer,
                    registration.element,
                    registration.name.as_concrete_TypeRef(),
                );
            }
        }
        if self.source_added {
            unsafe {
                CFRunLoopRemoveSource(self.run_loop, self.source, kCFRunLoopDefaultMode);
            }
            self.source_added = false;
        }
        if !self.observer.is_null() {
            unsafe {
                CFRelease(self.observer as CFTypeRef);
            }
            self.observer = ptr::null();
        }
    }
}

impl Drop for ObserverResources {
    fn drop(&mut self) {
        self.teardown();
    }
}

pub struct TargetGuard {
    application: OwnedCf,
    field: OwnedCf,
    app_id: String,
    pid: Pid,
    original_text: String,
    original_location: usize,
    original_length: usize,
    token: usize,
    observer: ObserverResources,
    // AXUIElement/AXObserver callbacks are main-run-loop confined by contract.
    _main_thread_only: PhantomData<Rc<()>>,
}

impl TargetGuard {
    pub fn capture() -> Result<Self, TargetError> {
        require_main_thread()?;
        let token = NEXT_SESSION_TOKEN
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(1)
            })
            .map_err(|_| TargetError::SessionTokenExhausted)?;
        let result = (|| {
            let target = current_target()?;
            let (original_text, original_location, original_length) =
                read_value_and_selection(target.field.as_ptr() as AXUIElementRef)?;
            let observer = ObserverResources::new(
                target.application.as_ptr() as AXUIElementRef,
                target.pid,
                token,
            )?;
            let candidate = Self {
                application: target.application,
                field: target.field,
                app_id: target.app_id,
                pid: target.pid,
                original_text,
                original_location,
                original_length,
                token,
                observer,
                _main_thread_only: PhantomData,
            };
            let snapshot_matches = candidate.snapshot_matches()?;
            // snapshot_matches already traverses and verifies the current
            // app/field identity. Avoid a third full AX traversal here, but
            // reject any observer invalidation during the snapshot read.
            if !capture_revalidation_succeeds(snapshot_matches, candidate.valid_token().is_ok()) {
                return Err(TargetError::TargetChanged);
            }
            Ok(candidate)
        })();
        if result.is_err() {
            LAST_INVALIDATED_TOKEN.fetch_max(token, Ordering::AcqRel);
        }
        result
    }

    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    fn valid_token(&self) -> Result<(), TargetError> {
        if LAST_INVALIDATED_TOKEN.load(Ordering::Acquire) < self.token {
            Ok(())
        } else {
            Err(TargetError::TargetChanged)
        }
    }

    fn matches_current(&self, current: &CurrentTarget) -> bool {
        let same_native = unsafe {
            CFEqual(self.application.as_ptr(), current.application.as_ptr()) != 0
                && CFEqual(self.field.as_ptr(), current.field.as_ptr()) != 0
        };
        same_native && self.pid == current.pid && self.app_id == current.app_id
    }

    pub fn unchanged(&self) -> Result<bool, TargetError> {
        require_main_thread()?;
        self.valid_token()?;
        let current = current_target()?;
        if self.matches_current(&current) {
            Ok(true)
        } else {
            Err(TargetError::TargetChanged)
        }
    }

    pub fn snapshot_matches(&self) -> Result<bool, TargetError> {
        let observed = self.observed_value_and_selection()?;
        Ok(observed.0 == self.original_text
            && observed.1 == self.original_location
            && observed.2 == self.original_length)
    }

    pub fn observed_value_and_selection(&self) -> Result<(String, usize, usize), TargetError> {
        require_main_thread()?;
        self.valid_token()?;
        let current = current_target()?;
        if !self.matches_current(&current) {
            return Err(TargetError::TargetChanged);
        }
        let observation = read_value_and_selection(current.field.as_ptr() as AXUIElementRef)?;
        self.valid_token()?;
        Ok(observation)
    }
}

impl Drop for TargetGuard {
    fn drop(&mut self) {
        LAST_INVALIDATED_TOKEN.fetch_max(self.token, Ordering::AcqRel);
        self.observer.teardown();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        capture_revalidation_succeeds, decode_utf16_bounded, main_thread_and_runloop_are_valid,
        validate_bundle_id, validate_editable_role, validate_selection_range, TargetError,
    };

    #[test]
    fn optional_subrole_absence_is_not_a_transport_or_type_failure() {
        assert_eq!(super::optional_subrole_present(super::AX_ERROR_NO_VALUE, false), Ok(false));
        assert_eq!(super::optional_subrole_present(super::AX_ERROR_ATTRIBUTE_UNSUPPORTED, false), Ok(false));
        assert_eq!(super::optional_subrole_present(super::AX_SUCCESS, true), Ok(true));
        assert_eq!(super::optional_subrole_present(super::AX_SUCCESS, false), Err(TargetError::InvalidValue));
        assert!(super::optional_subrole_present(super::AX_ERROR_API_DISABLED, false).is_err());
        assert!(super::optional_subrole_present(-25204, false).is_err());
        assert!(super::optional_subrole_present(-25202, false).is_err());
        assert!(validate_editable_role("AXTextArea", "").is_ok());
        assert_eq!(validate_editable_role("AXTextField", "AXSecureTextField"), Err(TargetError::SecureField));
    }

    #[test]
    fn native_calls_require_both_main_run_loop_and_main_thread() {
        assert!(main_thread_and_runloop_are_valid(true, true));
        assert!(!main_thread_and_runloop_are_valid(true, false));
        assert!(!main_thread_and_runloop_are_valid(false, true));
        assert!(!main_thread_and_runloop_are_valid(false, false));
    }

    #[test]
    fn capture_revalidation_requires_stable_identity_and_snapshot() {
        assert!(capture_revalidation_succeeds(true, true));
        assert!(!capture_revalidation_succeeds(false, true));
        assert!(!capture_revalidation_succeeds(true, false));
        assert!(!capture_revalidation_succeeds(false, false));
    }

    #[test]
    fn utf16_decoding_is_strict_and_bounded_in_utf8_bytes() {
        assert_eq!(
            decode_utf16_bounded(&[0x0041, 0xD83D, 0xDE00], 5).unwrap(),
            "A😀"
        );
        assert_eq!(
            decode_utf16_bounded(&[0xD83D], 10),
            Err(TargetError::InvalidValue)
        );
        assert_eq!(
            decode_utf16_bounded(&[b'a' as u16; 4], 3),
            Err(TargetError::ValueTooLarge)
        );
    }

    #[test]
    fn selection_uses_utf16_units_and_rejects_surrogate_boundaries() {
        let value = [0x0041, 0xD83D, 0xDE00, 0x0042];
        assert_eq!(validate_selection_range(&value, 1, 2), Ok((1, 2)));
        assert_eq!(
            validate_selection_range(&value, 2, 1),
            Err(TargetError::InvalidSelection)
        );
        assert_eq!(
            validate_selection_range(&value, -1, 0),
            Err(TargetError::InvalidSelection)
        );
        assert_eq!(
            validate_selection_range(&value, isize::MAX, 2),
            Err(TargetError::InvalidSelection)
        );
        assert_eq!(
            validate_selection_range(&value, 4, 1),
            Err(TargetError::InvalidSelection)
        );
    }

    #[test]
    fn only_nonsecure_text_roles_are_eligible() {
        assert!(validate_editable_role("AXTextField", "AXUnknown").is_ok());
        assert!(validate_editable_role("AXTextArea", "AXUnknown").is_ok());
        assert_eq!(
            validate_editable_role("AXTextField", "AXSecureTextField"),
            Err(TargetError::SecureField)
        );
        assert_eq!(
            validate_editable_role("AXButton", "AXButton"),
            Err(TargetError::InvalidRole)
        );
    }

    #[test]
    fn bundle_id_is_nonempty_bounded_utf8_without_controls() {
        assert!(validate_bundle_id("com.example.Editor").is_ok());
        assert_eq!(
            validate_bundle_id(""),
            Err(TargetError::AppIdentityUnavailable)
        );
        assert_eq!(
            validate_bundle_id("com.example\nEditor"),
            Err(TargetError::AppIdentityUnavailable)
        );
        assert_eq!(
            validate_bundle_id(&"x".repeat(513)),
            Err(TargetError::AppIdentityUnavailable)
        );
    }
}
