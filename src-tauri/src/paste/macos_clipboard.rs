//! Lossless macOS pasteboard snapshot/restore for auto-paste.
#![allow(deprecated)]

use cocoa::base::{id, nil};
use cocoa::foundation::NSString;
use objc::{class, msg_send, sel, sel_impl};

#[derive(Debug)]
pub(super) struct Snapshot {
    items: Vec<Vec<(String, Vec<u8>)>>,
}

/// Capture every representation of every item on the general pasteboard.
pub(super) fn snapshot() -> Option<Snapshot> {
    unsafe {
        let pool: id = msg_send![class!(NSAutoreleasePool), new];
        let pasteboard: id = msg_send![class!(NSPasteboard), generalPasteboard];
        if pasteboard == nil {
            let _: () = msg_send![pool, drain];
            return None;
        }

        let pasteboard_items: id = msg_send![pasteboard, pasteboardItems];
        let item_count: usize = msg_send![pasteboard_items, count];
        let mut items = Vec::with_capacity(item_count);

        for item_index in 0..item_count {
            let item: id = msg_send![pasteboard_items, objectAtIndex: item_index];
            let types: id = msg_send![item, types];
            let type_count: usize = msg_send![types, count];
            let mut representations = Vec::with_capacity(type_count);

            for type_index in 0..type_count {
                let pasteboard_type: id = msg_send![types, objectAtIndex: type_index];
                let utf8: *const std::os::raw::c_char = msg_send![pasteboard_type, UTF8String];
                if utf8.is_null() {
                    continue;
                }
                let name = std::ffi::CStr::from_ptr(utf8)
                    .to_string_lossy()
                    .into_owned();
                let data: id = msg_send![item, dataForType: pasteboard_type];
                if data == nil {
                    continue;
                }
                let length: usize = msg_send![data, length];
                let bytes: *const u8 = msg_send![data, bytes];
                let contents = if length == 0 {
                    Vec::new()
                } else if bytes.is_null() {
                    continue;
                } else {
                    std::slice::from_raw_parts(bytes, length).to_vec()
                };
                representations.push((name, contents));
            }
            items.push(representations);
        }

        let _: () = msg_send![pool, drain];
        Some(Snapshot { items })
    }
}

pub(super) fn change_count() -> isize {
    unsafe {
        let pasteboard: id = msg_send![class!(NSPasteboard), generalPasteboard];
        if pasteboard == nil {
            return -1;
        }
        msg_send![pasteboard, changeCount]
    }
}

#[derive(Debug)]
pub(super) struct TemporaryTextWriteError {
    pub(super) message: String,
    pub(super) owned_generation: isize,
}

/// Replace the pasteboard with temporary UTF-8 text and return the generation
/// created by our own `clearContents` call. Capturing ownership at the clear,
/// rather than sampling `changeCount` after the write, ensures a foreign copy
/// racing immediately afterward can never be mistaken for ours.
pub(super) fn set_temporary_text(text: &str) -> Result<isize, TemporaryTextWriteError> {
    unsafe {
        let pool: id = msg_send![class!(NSAutoreleasePool), new];
        let pasteboard: id = msg_send![class!(NSPasteboard), generalPasteboard];
        if pasteboard == nil {
            let _: () = msg_send![pool, drain];
            return Err(TemporaryTextWriteError {
                message: "general pasteboard is unavailable".to_string(),
                owned_generation: -1,
            });
        }

        let result = set_temporary_text_on_pasteboard(pasteboard, text);
        let _: () = msg_send![pool, drain];
        result
    }
}

/// Documented AppKit write path: populate an unattached NSPasteboardItem, then
/// publish it with `writeObjects:` after taking ownership via `clearContents`.
unsafe fn set_temporary_text_on_pasteboard(
    pasteboard: id,
    text: &str,
) -> Result<isize, TemporaryTextWriteError> {
    let owned_generation: isize = msg_send![pasteboard, clearContents];
    let item: id = msg_send![class!(NSPasteboardItem), new];
    let value = NSString::alloc(nil).init_str(text);
    let pasteboard_type = NSString::alloc(nil).init_str("public.utf8-plain-text");
    let item_ready: bool = msg_send![item, setString: value forType: pasteboard_type];

    let items: id = msg_send![class!(NSMutableArray), arrayWithCapacity: 1usize];
    let _: () = msg_send![items, addObject: item];
    let still_owned_before_write = generation_still_owned(
        owned_generation,
        msg_send![pasteboard, changeCount],
    );
    let written = item_ready
        && still_owned_before_write
        && msg_send![pasteboard, writeObjects: items];
    let current_generation: isize = msg_send![pasteboard, changeCount];

    let _: () = msg_send![value, release];
    let _: () = msg_send![pasteboard_type, release];
    let _: () = msg_send![item, release];

    if written && generation_still_owned(owned_generation, current_generation) {
        Ok(owned_generation)
    } else {
        Err(TemporaryTextWriteError {
            message: if !item_ready {
                "NSPasteboardItem failed to store temporary text".to_string()
            } else if !still_owned_before_write
                || !generation_still_owned(owned_generation, current_generation)
            {
                "pasteboard ownership changed during temporary text write".to_string()
            } else {
                "NSPasteboard writeObjects failed to publish temporary text".to_string()
            },
            owned_generation,
        })
    }
}

fn generation_still_owned(owned: isize, current: isize) -> bool {
    owned >= 0 && owned == current
}

fn should_restore(expected: isize, current: isize) -> bool {
    generation_still_owned(expected, current)
}

/// Restore only while Sagascript's temporary text is still the newest write.
pub(super) fn restore_if_unchanged(snapshot: Snapshot, expected_change_count: isize) -> bool {
    if !should_restore(expected_change_count, change_count()) {
        return false;
    }

    unsafe {
        let pool: id = msg_send![class!(NSAutoreleasePool), new];
        let pasteboard: id = msg_send![class!(NSPasteboard), generalPasteboard];
        if pasteboard == nil
            || !should_restore(expected_change_count, msg_send![pasteboard, changeCount])
        {
            let _: () = msg_send![pool, drain];
            return false;
        }

        let restored_items: id =
            msg_send![class!(NSMutableArray), arrayWithCapacity: snapshot.items.len()];
        for representations in snapshot.items {
            let item: id = msg_send![class!(NSPasteboardItem), new];
            for (name, contents) in representations {
                let pasteboard_type = NSString::alloc(nil).init_str(&name);
                let data: id = msg_send![class!(NSData), dataWithBytes: contents.as_ptr() length: contents.len()];
                let _ok: bool = msg_send![item, setData: data forType: pasteboard_type];
                let _: () = msg_send![pasteboard_type, release];
            }
            let _: () = msg_send![restored_items, addObject: item];
            let _: () = msg_send![item, release];
        }

        let _: isize = msg_send![pasteboard, clearContents];
        let restored: bool = msg_send![pasteboard, writeObjects: restored_items];
        let _: () = msg_send![pool, drain];
        restored
    }
}

#[cfg(test)]
mod tests {
    use super::{generation_still_owned, set_temporary_text_on_pasteboard, should_restore};
    use cocoa::base::{id, nil};
    use cocoa::foundation::NSString;
    use objc::{class, msg_send, sel, sel_impl};

    #[test]
    fn restores_only_when_app_still_owns_pasteboard() {
        assert!(should_restore(42, 42));
        assert!(!should_restore(42, 43));
        assert!(!should_restore(-1, -1));
    }

    #[test]
    fn foreign_copy_after_our_clear_invalidates_ownership() {
        let generation_returned_by_our_clear = 100;
        let generation_after_foreign_copy = 101;
        assert!(!should_restore(
            generation_returned_by_our_clear,
            generation_after_foreign_copy
        ));
    }

    #[test]
    fn write_never_adopts_a_later_generation() {
        assert!(generation_still_owned(100, 100));
        assert!(!generation_still_owned(100, 101));
        assert!(!generation_still_owned(-1, -1));
    }

    #[test]
    fn named_pasteboard_utf8_roundtrip_preserves_clear_generation() {
        unsafe {
            let pool: id = msg_send![class!(NSAutoreleasePool), new];
            let pasteboard: id = msg_send![class!(NSPasteboard), pasteboardWithUniqueName];
            assert_ne!(pasteboard, nil);

            let expected = "Sagascript – räksmörgås 🎙️";
            let owned_generation =
                set_temporary_text_on_pasteboard(pasteboard, expected).unwrap();
            let current_generation: isize = msg_send![pasteboard, changeCount];
            assert_eq!(owned_generation, current_generation);

            let pasteboard_type = NSString::alloc(nil).init_str("public.utf8-plain-text");
            let value: id = msg_send![pasteboard, stringForType: pasteboard_type];
            assert_ne!(value, nil);
            let utf8: *const std::os::raw::c_char = msg_send![value, UTF8String];
            assert!(!utf8.is_null());
            assert_eq!(std::ffi::CStr::from_ptr(utf8).to_string_lossy(), expected);

            let _: () = msg_send![pasteboard_type, release];
            let _: () = msg_send![pasteboard, releaseGlobally];
            let _: () = msg_send![pool, drain];
        }
    }
}
