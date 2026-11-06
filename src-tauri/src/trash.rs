//! Moving a recording folder to the Trash or the Recycle Bin (SAGA-564).
//!
//! A recording is hours of someone's voice that cannot be made again, so the
//! app never deletes one for good: `delete_recording` puts the folder where
//! the user can get it back. The CLI's `recordings delete --yes` removes the
//! folder (its own confirmation); the app does not.
//!
//! [`move_to_trash`] is the platform call: `NSFileManager`'s
//! `trashItemAtURL:resultingItemURL:error:` on macOS, `SHFileOperationW` with
//! `FOF_ALLOWUNDO` on Windows, and an `unavailable` error elsewhere (a
//! freedesktop trash is not implemented). Callers go through the
//! [`RecordingTrash`] seam so tests never touch a real trash.
//!
//! [`recording_folder`] decides which folder may be trashed at all: a direct
//! child of the recordings folder, a real directory (never a link), holding a
//! `recording.json`. Nothing else the user keeps next to a recording can be
//! reached through a recording's name.

use std::path::{Component, Path, PathBuf};

use sagascript_core::recording::MANIFEST_FILE;

use crate::command_error::CommandError;

/// Where a recording folder goes to be deleted.
pub trait RecordingTrash: Send + Sync {
    /// Move `folder` (already checked by [`recording_folder`]) to the trash.
    fn move_to_trash(&self, folder: &Path) -> Result<(), CommandError>;
}

/// The operating system's trash: [`move_to_trash`].
pub struct SystemTrash;

impl RecordingTrash for SystemTrash {
    fn move_to_trash(&self, folder: &Path) -> Result<(), CommandError> {
        move_to_trash(folder)
    }
}

/// The folder `folder_name` of the recordings folder `root`, if it is one that
/// may be trashed: `folder_name` is one plain path component, the folder is a
/// real directory (not a symbolic link) directly inside `root`, and it holds a
/// `recording.json` file. Messages carry no path.
pub(crate) fn recording_folder(root: &Path, folder_name: &str) -> Result<PathBuf, CommandError> {
    let mut components = Path::new(folder_name).components();
    let plain = matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(name)), None) if name == std::ffi::OsStr::new(folder_name)
    );
    if !plain || folder_name.contains(['/', '\\']) {
        return Err(CommandError::InvalidInput(
            "A recording is named by one plain folder name.".into(),
        ));
    }
    let folder = root.join(folder_name);
    // `symlink_metadata` does not follow a link: a link to a folder elsewhere
    // is refused rather than trashed (or followed out of the recordings folder).
    let metadata = std::fs::symlink_metadata(&folder).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CommandError::NotFound("That recording is no longer there.".into())
        } else {
            CommandError::Io("Could not look at the recording folder.".into()).because(error)
        }
    })?;
    if !metadata.is_dir() {
        return Err(CommandError::InvalidInput(
            "That is not a recording folder.".into(),
        ));
    }
    let manifest = std::fs::symlink_metadata(folder.join(MANIFEST_FILE));
    if !manifest.is_ok_and(|metadata| metadata.is_file()) {
        return Err(CommandError::InvalidInput(
            "That folder is not a recording (it has no recording.json).".into(),
        ));
    }
    Ok(folder)
}

/// Move `path` to the Trash or Recycle Bin. Blocking: call it from
/// `spawn_blocking`.
pub fn move_to_trash(path: &Path) -> Result<(), CommandError> {
    platform::move_to_trash(path)
}

#[cfg(target_os = "macos")]
mod platform {
    use std::path::Path;

    use objc2_foundation::{NSFileManager, NSString, NSURL};

    use crate::command_error::CommandError;

    pub(super) fn move_to_trash(path: &Path) -> Result<(), CommandError> {
        let Some(text) = path.to_str() else {
            return Err(CommandError::InvalidInput(
                "The recording folder has a name that cannot be moved to the Trash.".into(),
            ));
        };
        let url = NSURL::fileURLWithPath(&NSString::from_str(text));
        // `NSFileManager.defaultManager` is documented as usable from any thread
        // for this call; nothing here needs the main thread (ADR 0003).
        NSFileManager::defaultManager()
            .trashItemAtURL_resultingItemURL_error(&url, None)
            .map_err(|error| {
                CommandError::Io("Could not move the recording to the Trash.".into())
                    .because(error.localizedDescription())
            })
    }
}

/// `GetDriveTypeW`'s answer for a fixed local disk. The shell sends an item to
/// the Recycle Bin only where the volume has one, and `SHFileOperationW` with
/// `FOF_ALLOWUNDO` *deletes for good* where it has none (a network share or
/// UNC path, a USB stick, a RAM disk), without an error. The app promises a
/// reversible move, so only a fixed disk is trusted to have a bin.
#[cfg_attr(not(windows), allow(dead_code))]
const DRIVE_FIXED: u32 = 3;

/// Whether a volume of this `GetDriveTypeW` type is expected to have a
/// Recycle Bin that takes a recording folder.
#[cfg_attr(not(windows), allow(dead_code))]
fn recycle_bin_expected(drive_type: u32) -> bool {
    drive_type == DRIVE_FIXED
}

#[cfg(windows)]
mod platform {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumePathNameW};
    use windows_sys::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT,
        FOF_WANTNUKEWARNING, FO_DELETE, SHFILEOPSTRUCTW,
    };

    use super::recycle_bin_expected;
    use crate::command_error::CommandError;

    fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
        text.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn not_recyclable() -> CommandError {
        CommandError::Unavailable(
            "This recording is on a drive without a Recycle Bin (a network share, a USB drive or similar), so it was not deleted. Delete the folder in File Explorer if you want it gone."
                .into(),
        )
    }

    /// The `GetDriveTypeW` type of the volume that holds `path`, or `None`
    /// when Windows does not say.
    fn drive_type(path: &Path) -> Option<u32> {
        let path = wide(path.as_os_str());
        let mut root = vec![0u16; 1024];
        // SAFETY: `path` is NUL terminated and `root` has the length passed.
        if unsafe { GetVolumePathNameW(path.as_ptr(), root.as_mut_ptr(), root.len() as u32) } == 0 {
            return None;
        }
        // SAFETY: `root` was NUL terminated by the call above.
        Some(unsafe { GetDriveTypeW(root.as_ptr()) })
    }

    pub(super) fn move_to_trash(path: &Path) -> Result<(), CommandError> {
        // The shell needs a full path, and does not take the `\\?\` form.
        let full = std::path::absolute(path).map_err(|error| {
            CommandError::Io("Could not move the recording to the Recycle Bin.".into())
                .because(error)
        })?;
        let text = full.to_string_lossy();
        let plain = text.strip_prefix(r"\\?\").unwrap_or(&text);
        // Fail closed: a volume that is not known to have a Recycle Bin is
        // refused before the shell is called, never trusted to "undo".
        if !drive_type(Path::new(plain)).is_some_and(recycle_bin_expected) {
            return Err(not_recyclable());
        }
        // `pFrom` is a list of names, each ended by a NUL, and the list by another.
        let mut from: Vec<u16> = std::ffi::OsStr::new(plain).encode_wide().collect();
        from.extend([0, 0]);
        // `FOF_WANTNUKEWARNING` makes the shell warn (and let the user refuse)
        // where it would delete for good even on a fixed disk (the bin is
        // switched off, or the folder is larger than the bin allows); it
        // overrides `FOF_NOCONFIRMATION` for that case only. Refusing it
        // aborts the operation, which is reported below, and nothing is lost.
        let mut operation = SHFILEOPSTRUCTW {
            hwnd: std::ptr::null_mut(),
            wFunc: FO_DELETE,
            pFrom: from.as_ptr(),
            pTo: std::ptr::null(),
            fFlags: (FOF_ALLOWUNDO
                | FOF_NOCONFIRMATION
                | FOF_NOERRORUI
                | FOF_SILENT
                | FOF_WANTNUKEWARNING) as u16,
            fAnyOperationsAborted: 0,
            hNameMappings: std::ptr::null_mut(),
            lpszProgressTitle: std::ptr::null(),
        };
        // SAFETY: `operation` is fully initialised, `from` is a NUL-NUL
        // terminated UTF-16 list that outlives the call, and the call is
        // synchronous (no `FOF_ASYNC`).
        let code = unsafe { SHFileOperationW(&mut operation) };
        if operation.fAnyOperationsAborted != 0 {
            return Err(CommandError::Unavailable(
                "The recording was not moved: Windows could not send it to the Recycle Bin and the delete was declined."
                    .into(),
            ));
        }
        if code != 0 {
            return Err(CommandError::Io(
                "Could not move the recording to the Recycle Bin.".into(),
            )
            .because(format!("SHFileOperationW returned {code:#x}")));
        }
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    use std::path::Path;

    use crate::command_error::CommandError;

    pub(super) fn move_to_trash(_path: &Path) -> Result<(), CommandError> {
        Err(CommandError::Unavailable(
            "Moving a recording to the trash is not available on this system.".into(),
        ))
    }
}

#[cfg(test)]
#[path = "trash_tests.rs"]
mod tests;
