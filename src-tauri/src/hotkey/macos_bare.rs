use std::mem;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};

use block2::RcBlock;
use objc2_app_kit::{
    NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSF13FunctionKey, NSF24FunctionKey,
};
use tauri::{AppHandle, Manager};

use super::{BareHotkeyState, HotkeyHealth, OperationalHotkey};

static MONITOR_INSTALLED: AtomicBool = AtomicBool::new(false);

pub fn bare_function_key_monitor_installed() -> bool {
    MONITOR_INSTALLED.load(Ordering::Acquire)
}

fn function_key_from_scalar(scalar: u32, modifiers: NSEventModifierFlags) -> Option<String> {
    let disallowed_modifiers = NSEventModifierFlags::Shift
        | NSEventModifierFlags::Control
        | NSEventModifierFlags::Option
        | NSEventModifierFlags::Command;
    if modifiers.intersects(disallowed_modifiers) {
        return None;
    }

    (NSF13FunctionKey..=NSF24FunctionKey)
        .contains(&scalar)
        .then(|| format!("F{}", scalar - NSF13FunctionKey + 13))
}

fn bare_function_key(event: &NSEvent) -> Option<String> {
    let characters = event.charactersIgnoringModifiers()?;
    let characters = characters.to_string();
    let mut characters = characters.chars();
    let scalar = characters.next()? as u32;
    if characters.next().is_some() {
        return None;
    }

    function_key_from_scalar(scalar, event.modifierFlags())
}

fn shortcut_is_operational(app: &AppHandle, shortcut: &str) -> bool {
    let health: tauri::State<'_, HotkeyHealth> = app.state();
    shortcut_is_registered(shortcut, &health.operational_hotkey())
}

fn shortcut_is_registered(shortcut: &str, operational: &OperationalHotkey) -> bool {
    let Ok(target) = sagascript_core::settings::canonical_hotkey(shortcut) else {
        return false;
    };
    match operational {
        OperationalHotkey::Registered(shortcuts) => shortcuts.iter().any(|registered| {
            sagascript_core::settings::canonical_hotkey(registered).as_deref()
                == Ok(target.as_str())
        }),
        OperationalHotkey::Inactive | OperationalHotkey::Unknown => false,
    }
}

fn dispatch_event(app: &AppHandle, event: &NSEvent) {
    let state = match event.r#type() {
        NSEventType::KeyDown if !event.isARepeat() => BareHotkeyState::Pressed,
        NSEventType::KeyUp => BareHotkeyState::Released,
        _ => return,
    };
    if let Some(shortcut) = bare_function_key(event) {
        if shortcut_is_operational(app, &shortcut) {
            crate::handle_hotkey_event(app, &shortcut, state);
        }
    }
}

/// Install process-lifetime AppKit monitors for unmodified F13-F24 events.
///
/// AppKit delivers events for other applications to the global monitor and
/// events targeting Sagascript to the local monitor, so both are required.
/// This function is called from Tauri setup, which runs on the macOS main
/// thread. Monitor tokens are intentionally retained for the process lifetime.
pub fn install_bare_function_key_monitor(app: &AppHandle) -> Result<(), String> {
    if MONITOR_INSTALLED.swap(true, Ordering::AcqRel) {
        return Ok(());
    }

    let mask = NSEventMask::KeyDown | NSEventMask::KeyUp;
    let global_app = app.clone();
    let global_handler = RcBlock::new(move |event: NonNull<NSEvent>| {
        // SAFETY: AppKit supplies a valid NSEvent for the duration of the block.
        dispatch_event(&global_app, unsafe { event.as_ref() });
    });
    let Some(global_monitor) =
        NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &global_handler)
    else {
        MONITOR_INSTALLED.store(false, Ordering::Release);
        return Err("macOS did not create the global F13-F24 event monitor".to_string());
    };

    let local_app = app.clone();
    let local_handler = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit supplies a valid NSEvent for the duration of the block.
        dispatch_event(&local_app, unsafe { event.as_ref() });
        event.as_ptr()
    });
    // SAFETY: Returning the unchanged event pointer preserves AppKit's event
    // dispatch contract; neither the pointer nor the reference escapes.
    let local_monitor =
        unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &local_handler) };
    let Some(local_monitor) = local_monitor else {
        // SAFETY: `global_monitor` is the token returned by the matching AppKit API.
        unsafe { NSEvent::removeMonitor(&global_monitor) };
        MONITOR_INSTALLED.store(false, Ordering::Release);
        return Err("macOS did not create the local F13-F24 event monitor".to_string());
    };

    mem::forget(global_monitor);
    mem::forget(local_monitor);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{function_key_from_scalar, shortcut_is_registered};
    use crate::hotkey::OperationalHotkey;
    use objc2_app_kit::{NSEventModifierFlags, NSF13FunctionKey, NSF24FunctionKey};

    #[test]
    fn maps_the_complete_private_use_function_key_range() {
        for number in 13_u32..=24 {
            let scalar = NSF13FunctionKey + number - 13;
            assert_eq!(
                function_key_from_scalar(scalar, NSEventModifierFlags::empty()),
                Some(format!("F{number}"))
            );
        }
        assert_eq!(
            function_key_from_scalar(NSF13FunctionKey - 1, NSEventModifierFlags::empty()),
            None
        );
        assert_eq!(
            function_key_from_scalar(NSF24FunctionKey + 1, NSEventModifierFlags::empty()),
            None
        );
    }

    #[test]
    fn rejects_user_modifiers_but_allows_the_function_flag() {
        for modifier in [
            NSEventModifierFlags::Shift,
            NSEventModifierFlags::Control,
            NSEventModifierFlags::Option,
            NSEventModifierFlags::Command,
        ] {
            assert_eq!(function_key_from_scalar(NSF13FunctionKey, modifier), None);
        }
        assert_eq!(
            function_key_from_scalar(NSF24FunctionKey, NSEventModifierFlags::Function),
            Some("F24".to_string())
        );
    }

    #[test]
    fn dispatch_requires_the_exact_shortcut_to_be_operational() {
        let registered =
            OperationalHotkey::registered_many(&[" f13 ".to_string(), "Control+Space".to_string()]);
        assert!(shortcut_is_registered("F13", &registered));
        assert!(!shortcut_is_registered("F14", &registered));
        assert!(!shortcut_is_registered("F13", &OperationalHotkey::Inactive));
        assert!(!shortcut_is_registered("F13", &OperationalHotkey::Unknown));
    }
}
