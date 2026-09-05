//! Windows text restoration must compare and write while the system clipboard
//! is open. A process mutex or a sequence sampled after arboard closes it cannot
//! exclude foreign writers. No guard survives into the paste delay/keystroke.

use std::num::NonZeroU32;

pub(super) struct TemporaryText {
    saved_text: Option<String>,
    generation: Option<NonZeroU32>,
}

trait Transaction {
    fn text(&mut self) -> Result<Option<String>, String>;
    fn set_text(&mut self, text: &str) -> Result<(), String>;
    fn generation(&mut self) -> Option<NonZeroU32>;
}

fn set_temporary_in(
    mut transaction: impl Transaction,
    text: &str,
) -> Result<TemporaryText, String> {
    // Windows currently preserves plain text only. A non-text clipboard has no
    // text snapshot; this is not a promise to preserve images/custom formats.
    let saved_text = transaction.text()?;
    if let Err(error) = transaction.set_text(text) {
        // A failed native set can already have emptied the clipboard. The
        // system guard is still held, so no newer writer can be overwritten.
        if let Some(saved) = saved_text.as_deref() {
            if let Err(restore_error) = transaction.set_text(saved) {
                tracing::warn!("Clipboard rollback failed: {restore_error}");
            }
        }
        return Err(error);
    }
    let generation = transaction.generation();
    Ok(TemporaryText {
        saved_text,
        generation,
    })
}

fn restore_in(mut transaction: impl Transaction, saved: TemporaryText) -> Result<bool, String> {
    let (Some(text), Some(generation)) = (saved.saved_text, saved.generation) else {
        return Ok(false);
    };
    if transaction.generation() != Some(generation) {
        return Ok(false);
    }
    transaction.set_text(&text)?;
    Ok(true)
}

#[cfg(target_os = "windows")]
struct NativeTransaction {
    _guard: clipboard_win::Clipboard,
    // An open clipboard belongs to this thread, not to a Send worker payload.
    _thread: std::marker::PhantomData<*const ()>,
}

#[cfg(target_os = "windows")]
impl NativeTransaction {
    fn open() -> Result<Self, String> {
        // Match arboard's bounded retries, including a real sleep rather than
        // clipboard-win's zero-ms retry loop. Guard is local and RAII-closed.
        for attempt in 0..=5 {
            match clipboard_win::Clipboard::new() {
                Ok(guard) => {
                    return Ok(Self {
                        _guard: guard,
                        _thread: std::marker::PhantomData,
                    })
                }
                Err(error) if attempt == 5 => {
                    return Err(format!("Open clipboard failed: {error}"))
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
        }
        unreachable!("bounded loop returns on its last attempt")
    }
}

#[cfg(target_os = "windows")]
impl Transaction for NativeTransaction {
    fn text(&mut self) -> Result<Option<String>, String> {
        if !clipboard_win::is_format_avail(13) {
            // CF_UNICODETEXT
            return Ok(None);
        }
        let mut text = String::new();
        clipboard_win::raw::get_string(&mut text)
            .map_err(|error| format!("Read clipboard failed: {error}"))?;
        Ok(Some(text))
    }
    fn set_text(&mut self, text: &str) -> Result<(), String> {
        clipboard_win::raw::set_string(text)
            .map_err(|error| format!("Write clipboard failed: {error}"))
    }
    fn generation(&mut self) -> Option<NonZeroU32> {
        clipboard_win::seq_num()
    }
}

#[cfg(target_os = "windows")]
pub(super) fn set_temporary_text(text: &str) -> Result<TemporaryText, String> {
    set_temporary_in(NativeTransaction::open()?, text)
}

#[cfg(target_os = "windows")]
pub(super) fn restore_if_unchanged(saved: TemporaryText) -> Result<bool, String> {
    restore_in(NativeTransaction::open()?, saved)
}

#[cfg(test)]
#[path = "windows_clipboard_tests.rs"]
mod tests;
