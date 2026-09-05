//! Windows text restoration must compare and write while the system clipboard
//! is open. A process mutex or a sequence sampled after arboard closes it cannot
//! exclude foreign writers. No guard survives into the paste delay/keystroke.

use std::num::NonZeroU32;
use std::sync::mpsc;
use std::time::{Duration, Instant};

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

/// Windows may finalize synthesized formats when the write guard closes.
/// Re-sample only after reacquiring the clipboard AND proving that the same
/// dedicated owner still owns our text. A foreign write in the gap must never
/// become the generation that authorizes restoring an older snapshot.
fn finalize_generation_in(
    mut transaction: impl Transaction,
    mut saved: TemporaryText,
    temporary: &str,
    owner_matches: bool,
) -> Result<TemporaryText, String> {
    saved.generation = None;
    if owner_matches && transaction.text()?.as_deref() == Some(temporary) {
        saved.generation = transaction.generation();
    }
    Ok(saved)
}

enum RestoreRequest {
    Restore,
}

/// Send-only handle for the clipboard worker. The worker owns the native
/// clipboard owner and its saved snapshot; neither an HWND nor a native token
/// crosses the paste/restore boundary.
pub(super) struct PendingRestore {
    restore_tx: mpsc::Sender<RestoreRequest>,
}

impl PendingRestore {
    pub(super) fn schedule_restore(self) {
        if self.restore_tx.send(RestoreRequest::Restore).is_err() {
            tracing::debug!("Clipboard worker exited before restore was scheduled");
        }
    }
}

/// Run the complete native session on one worker thread. The initializer runs
/// before the ready result is sent; the session value never leaves that thread.
/// Dropping the pending handle disconnects the receiver and therefore drops
/// the session without restoring, preserving the existing failure behavior.
fn spawn_restore_worker<T, Init, Restore, Pump>(
    init: Init,
    restore: Restore,
    mut pump: Pump,
) -> Result<PendingRestore, String>
where
    Init: FnOnce() -> Result<T, String> + Send + 'static,
    Restore: FnOnce(T) -> Result<(), String> + Send + 'static,
    Pump: FnMut() + Send + 'static,
{
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (restore_tx, restore_rx) = mpsc::channel();

    std::thread::Builder::new()
        .name("sagascript-clipboard".to_owned())
        .spawn(move || {
            let session = match init() {
                Ok(session) => session,
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return;
                }
            };

            if ready_tx.send(Ok(())).is_err() {
                // The caller went away before observing readiness. Keep the
                // temporary clipboard contents, but drop the native session
                // here, on its creating thread.
                return;
            }

            let should_restore = loop {
                pump();
                match restore_rx.recv_timeout(Duration::from_millis(5)) {
                    Ok(RestoreRequest::Restore) => break true,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break false,
                }
            };

            if should_restore {
                let deadline = Instant::now() + Duration::from_millis(100);
                while Instant::now() < deadline {
                    pump();
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    std::thread::sleep(remaining.min(Duration::from_millis(5)));
                }
                if let Err(error) = restore(session) {
                    tracing::warn!("Clipboard restore failed: {error}");
                }
            }
            // A dropped sender (caller-side failure or cancellation) reaches
            // this point without restoring; `session` is dropped on this
            // worker thread in either case.
        })
        .map_err(|error| format!("Clipboard worker spawn failed: {error}"))?;

    match ready_rx.recv() {
        Ok(Ok(())) => Ok(PendingRestore { restore_tx }),
        Ok(Err(error)) => Err(error),
        Err(_) => Err("Clipboard worker exited before ready".to_owned()),
    }
}

#[cfg(target_os = "windows")]
struct ClipboardOwner(std::ptr::NonNull<std::ffi::c_void>);

#[cfg(target_os = "windows")]
#[repr(C)]
struct NativePoint {
    x: i32,
    y: i32,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct NativeMessage {
    hwnd: *mut std::ffi::c_void,
    message: u32,
    w_param: usize,
    l_param: isize,
    time: u32,
    point: NativePoint,
    l_private: u32,
}

#[cfg(target_os = "windows")]
fn pump_messages() {
    const PM_REMOVE: u32 = 0x0001;
    const WM_QUIT: u32 = 0x0012;
    const MAX_MESSAGES_PER_CYCLE: usize = 16;

    let mut message = NativeMessage {
        hwnd: std::ptr::null_mut(),
        message: 0,
        w_param: 0,
        l_param: 0,
        time: 0,
        point: NativePoint { x: 0, y: 0 },
        l_private: 0,
    };
    for _ in 0..MAX_MESSAGES_PER_CYCLE {
        if unsafe { PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) } == 0 {
            break;
        }
        if message.message != WM_QUIT {
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}

#[cfg(target_os = "windows")]
impl ClipboardOwner {
    fn new() -> Result<Self, String> {
        // A built-in STATIC message-only window needs no registered class or
        // visible UI. It is created/destroyed on this paste worker thread;
        // immediate text formats need no delayed rendering, while the worker
        // pumps this thread's clipboard messages during the session.
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let handle = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                0,
                0,
                0,
                0,
                0,
                (-3isize) as *mut std::ffi::c_void,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        std::ptr::NonNull::new(handle).map(Self).ok_or_else(|| {
            format!(
                "Create clipboard owner failed: {}",
                std::io::Error::last_os_error()
            )
        })
    }
}

#[cfg(target_os = "windows")]
impl Drop for ClipboardOwner {
    fn drop(&mut self) {
        if unsafe { DestroyWindow(self.0.as_ptr()) } == 0 {
            tracing::warn!(
                "Destroy clipboard owner failed: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

#[cfg(target_os = "windows")]
#[link(name = "user32")]
extern "system" {
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: *mut std::ffi::c_void,
        menu: *mut std::ffi::c_void,
        instance: *mut std::ffi::c_void,
        parameter: *mut std::ffi::c_void,
    ) -> *mut std::ffi::c_void;
    fn DestroyWindow(window: *mut std::ffi::c_void) -> i32;
}

#[cfg(target_os = "windows")]
pub(super) struct NativeTemporaryText {
    saved: TemporaryText,
    owner: ClipboardOwner,
}

#[cfg(target_os = "windows")]
struct NativeTransaction {
    _guard: clipboard_win::Clipboard,
    // An open clipboard belongs to this thread, not to a Send worker payload.
    _thread: std::marker::PhantomData<*const ()>,
}

#[cfg(target_os = "windows")]
impl NativeTransaction {
    #[cfg(test)]
    fn open() -> Result<Self, String> {
        Self::open_for(std::ptr::null_mut())
    }

    fn open_for(owner: *mut std::ffi::c_void) -> Result<Self, String> {
        // Match arboard's bounded retries, including a real sleep rather than
        // clipboard-win's zero-ms retry loop. Guard is local and RAII-closed.
        for attempt in 0..=5 {
            match clipboard_win::Clipboard::new_for(owner) {
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
        use clipboard_win::Getter;
        if !clipboard_win::is_format_avail(13) {
            // CF_UNICODETEXT
            return Ok(None);
        }
        let mut text = String::new();
        clipboard_win::formats::Unicode
            .read_clipboard(&mut text)
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
#[link(name = "user32")]
extern "system" {
    fn PeekMessageW(
        message: *mut NativeMessage,
        window: *mut std::ffi::c_void,
        min_filter: u32,
        max_filter: u32,
        remove_message: u32,
    ) -> i32;
    fn TranslateMessage(message: *const NativeMessage) -> i32;
    fn DispatchMessageW(message: *const NativeMessage) -> isize;
}

#[cfg(target_os = "windows")]
fn set_temporary_text_on_thread(text: &str) -> Result<NativeTemporaryText, String> {
    let owner = ClipboardOwner::new()?;
    let saved = set_temporary_in(NativeTransaction::open_for(owner.0.as_ptr())?, text)?;
    // set_temporary_in has dropped its guard. Keep the owner alive, but never
    // keep the global clipboard locked during the paste delay or key dispatch.
    let mut transaction = NativeTransaction::open_for(owner.0.as_ptr())?;
    if clipboard_win::get_owner() != Some(owner.0) {
        return Err(
            "Clipboard changed before paste; recognized text remains in Sagascript".to_owned(),
        );
    }
    if transaction.text()?.as_deref() != Some(text) {
        return Err(
            "Clipboard changed before paste; recognized text remains in Sagascript".to_owned(),
        );
    }
    let owner_matches = true;
    let saved = finalize_generation_in(transaction, saved, text, owner_matches)?;
    Ok(NativeTemporaryText { saved, owner })
}

#[cfg(target_os = "windows")]
fn restore_if_unchanged_on_thread(temporary: NativeTemporaryText) -> Result<bool, String> {
    let transaction = NativeTransaction::open_for(temporary.owner.0.as_ptr())?;
    if clipboard_win::get_owner() != Some(temporary.owner.0) {
        tracing::debug!("Clipboard restore skipped: generation changed or no text snapshot");
        return Ok(false);
    }
    let restored = restore_in(transaction, temporary.saved)?;
    if !restored {
        tracing::debug!("Clipboard restore skipped: generation changed or no text snapshot");
    }
    Ok(restored)
}

#[cfg(target_os = "windows")]
fn restore_session_on_thread(temporary: NativeTemporaryText) -> Result<(), String> {
    let _ = restore_if_unchanged_on_thread(temporary)?;
    Ok(())
}

#[cfg(target_os = "windows")]
pub(super) fn set_temporary_text(text: &str) -> Result<PendingRestore, String> {
    let text = text.to_owned();
    spawn_restore_worker(
        move || set_temporary_text_on_thread(&text).map_err(|error| error.to_owned()),
        restore_session_on_thread,
        pump_messages,
    )
}

#[cfg(test)]
#[path = "windows_clipboard_tests.rs"]
mod tests;
