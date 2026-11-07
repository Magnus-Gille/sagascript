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

/// `GetDriveTypeW`'s answer for a fixed local disk.
#[cfg_attr(not(windows), allow(dead_code))]
const DRIVE_FIXED: u32 = 3;

/// What is known about the Recycle Bin that would take a recording folder
/// (Windows). The app promises a *reversible* move, and the shell deletes for
/// good, without an error, when the bin cannot take an item: a volume without
/// one (a network share, a USB stick), a bin switched off, an item larger
/// than the bin's quota. Every one of those is refused up front.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct BinFacts {
    /// `GetDriveTypeW` of the volume, or `None` when Windows does not say.
    drive_type: Option<u32>,
    /// "Don't move files to the Recycle Bin" for the volume (or for all), or
    /// the policy that removes the bin.
    bin_off: bool,
    /// The volume's bin quota in bytes, when it has an explicit one.
    quota_bytes: Option<u64>,
    /// The volume's size, to estimate the default quota.
    volume_bytes: Option<u64>,
    /// What the folder holds.
    folder_bytes: u64,
    /// A read that decides the answer failed (a registry value that exists
    /// but cannot be read, a file of the folder whose size is unknown, the
    /// volume's settings key that cannot be named). Never "no setting" or
    /// "0 bytes".
    unreadable: bool,
}

/// Why a recording is not moved to the Recycle Bin.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BinRefusal {
    NoBin,
    BinOff,
    TooLarge,
    Unknown,
    Unreadable,
}

/// The share of a volume Windows keeps for its bin when no quota is set. The
/// real default is not documented; a low estimate refuses more, never less.
#[cfg_attr(not(windows), allow(dead_code))]
const DEFAULT_QUOTA_PERCENT: u64 = 5;

/// Whether the Recycle Bin is guaranteed to take the folder. Anything that
/// cannot be established is a refusal.
#[cfg_attr(not(windows), allow(dead_code))]
fn recycle_guaranteed(facts: &BinFacts) -> Result<(), BinRefusal> {
    if facts.drive_type != Some(DRIVE_FIXED) {
        return Err(BinRefusal::NoBin);
    }
    if facts.bin_off {
        return Err(BinRefusal::BinOff);
    }
    if facts.unreadable {
        return Err(BinRefusal::Unreadable);
    }
    let quota = facts.quota_bytes.or_else(|| {
        facts
            .volume_bytes
            .map(|total| total / 100 * DEFAULT_QUOTA_PERCENT)
    });
    match quota {
        None => Err(BinRefusal::Unknown),
        Some(quota) if facts.folder_bytes > quota => Err(BinRefusal::TooLarge),
        Some(_) => Ok(()),
    }
}

/// The refusal as the error the window shows (no path).
#[cfg_attr(not(windows), allow(dead_code))]
fn refusal_error(refusal: BinRefusal) -> CommandError {
    let why = match refusal {
        BinRefusal::NoBin => {
            "This recording is on a drive without a Recycle Bin (a network share, a USB drive or similar)."
        }
        BinRefusal::BinOff => "The Recycle Bin is switched off for this drive.",
        BinRefusal::TooLarge => "This recording is larger than the Recycle Bin of its drive can hold.",
        BinRefusal::Unknown => "Windows did not say whether the Recycle Bin can hold this recording.",
        BinRefusal::Unreadable => {
            "The Recycle Bin settings of this drive or the size of the recording could not be read."
        }
    };
    CommandError::Unavailable(format!(
        "{why} It was not deleted. Delete the folder in File Explorer if you want it gone for good."
    ))
}

#[cfg(windows)]
mod platform {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows::core::HSTRING;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD,
    };
    use windows::Win32::UI::Shell::{
        FileOperation, IFileOperation, IShellItem, SHCreateItemFromParsingName,
        FOFX_RECYCLEONDELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT,
        FOF_WANTNUKEWARNING,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
    };

    use super::{recycle_guaranteed, refusal_error, BinFacts};
    use crate::command_error::CommandError;

    const BIN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket";
    const POLICY_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer";
    const MIB: u64 = 1024 * 1024;

    fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
        text.encode_wide().chain(std::iter::once(0)).collect()
    }

    /// What reading one registry DWORD gave.
    enum Reg {
        Value(u32),
        /// The key or value does not exist: the setting is at its default.
        Absent,
        /// Anything else (access denied, a value of another type, ...): the
        /// setting may exist and is not known.
        Failed,
    }

    fn reg_dword(root: HKEY, subkey: &str, name: &str) -> Reg {
        let subkey = HSTRING::from(subkey);
        let name = HSTRING::from(name);
        let mut value: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: the strings are NUL terminated `HSTRING`s, and `value` and
        // `size` are valid for the sizes passed.
        let status = unsafe {
            RegGetValueW(
                root,
                &subkey,
                &name,
                RRF_RT_REG_DWORD,
                None,
                Some(std::ptr::from_mut(&mut value).cast()),
                Some(&mut size),
            )
        };
        if status == ERROR_SUCCESS {
            Reg::Value(value)
        } else if status == ERROR_FILE_NOT_FOUND || status == ERROR_PATH_NOT_FOUND {
            Reg::Absent
        } else {
            Reg::Failed
        }
    }

    /// The bytes in `dir`, files only, links not followed; `None` when any
    /// part of the folder could not be read (an unknown size is not 0).
    fn folder_bytes(dir: &Path) -> Option<u64> {
        let mut total = 0;
        let mut pending = vec![dir.to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).ok()? {
                let entry = entry.ok()?;
                let kind = entry.file_type().ok()?;
                if kind.is_dir() {
                    pending.push(entry.path());
                } else if kind.is_file() {
                    total += entry.metadata().ok()?.len();
                }
            }
        }
        Some(total)
    }

    /// What Windows says about the Recycle Bin for the volume of `path`. A
    /// read that fails where the answer matters sets `unreadable`: the caller
    /// refuses, it does not assume the default.
    fn bin_facts(path: &Path) -> BinFacts {
        let mut facts = BinFacts::default();
        match folder_bytes(path) {
            Some(bytes) => facts.folder_bytes = bytes,
            None => facts.unreadable = true,
        }
        let wide_path = wide(path.as_os_str());
        let mut root = vec![0u16; 1024];
        // SAFETY: `wide_path` is NUL terminated and `root` has the length passed.
        if unsafe { GetVolumePathNameW(wide_path.as_ptr(), root.as_mut_ptr(), root.len() as u32) }
            == 0
        {
            return facts;
        }
        // SAFETY: `root` was NUL terminated by the call above.
        facts.drive_type = Some(unsafe { GetDriveTypeW(root.as_ptr()) });
        let mut total: u64 = 0;
        // SAFETY: `root` is NUL terminated; the unused out-parameters are null,
        // which the function allows.
        let have_total = unsafe {
            GetDiskFreeSpaceExW(
                root.as_ptr(),
                std::ptr::null_mut(),
                &mut total,
                std::ptr::null_mut(),
            )
        } != 0;
        facts.volume_bytes = have_total.then_some(total);

        // The bin's settings are kept per volume GUID (`\\?\Volume{...}\`).
        let mut name = vec![0u16; 64];
        // SAFETY: `root` is NUL terminated and `name` has the length passed.
        let named = unsafe {
            GetVolumeNameForVolumeMountPointW(root.as_ptr(), name.as_mut_ptr(), name.len() as u32)
        } != 0;
        let volume_key = named.then(|| {
            let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
            let text = String::from_utf16_lossy(&name[..end]);
            let guid = text
                .trim_start_matches(r"\\?\Volume")
                .trim_end_matches('\\')
                .to_owned();
            format!(r"{BIN_KEY}\Volume\{guid}")
        });
        if volume_key.is_none() {
            // Without the volume's key its settings cannot be looked at.
            facts.unreadable = true;
        }
        let unreadable = std::cell::Cell::new(false);
        let read = |hive: HKEY, key: &str, name: &str| match reg_dword(hive, key, name) {
            Reg::Value(value) => Some(value),
            Reg::Absent => None,
            Reg::Failed => {
                unreadable.set(true);
                None
            }
        };
        let nuke = |key: &str| read(HKEY_CURRENT_USER, key, "NukeOnDelete") == Some(1);
        facts.bin_off = nuke(BIN_KEY)
            || volume_key.as_deref().is_some_and(nuke)
            || [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE]
                .into_iter()
                .any(|hive| read(hive, POLICY_KEY, "NoRecycleFiles") == Some(1));
        facts.quota_bytes = volume_key
            .as_deref()
            .and_then(|key| read(HKEY_CURRENT_USER, key, "MaxCapacity"))
            .map(|megabytes| u64::from(megabytes) * MIB);
        facts.unreadable |= unreadable.get();
        facts
    }

    /// `IFileOperation::DeleteItem` with `FOFX_RECYCLEONDELETE`: the item is
    /// meant to be recycled or the operation to fail. That behaviour is not
    /// documented well enough to rely on alone, and it has not been observed on
    /// Windows from here, so `FOF_WANTNUKEWARNING` stays: if the shell ever
    /// means to delete for good anyway it asks instead of doing it silently.
    /// Remove it once the hardware checklist has shown that the flag alone
    /// fails closed.
    /// Returns whether the shell aborted the operation.
    fn recycle(path: &str) -> windows::core::Result<bool> {
        // SAFETY: COM is initialised for this thread (and released below), and
        // every interface is dropped before `CoUninitialize`.
        unsafe {
            let initialised = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
            let result = (|| -> windows::core::Result<bool> {
                let operation: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;
                operation.SetOperationFlags(
                    FOF_ALLOWUNDO
                        | FOF_NOCONFIRMATION
                        | FOF_NOERRORUI
                        | FOF_SILENT
                        | FOF_WANTNUKEWARNING
                        | FOFX_RECYCLEONDELETE,
                )?;
                let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path), None)?;
                operation.DeleteItem(&item, None)?;
                operation.PerformOperations()?;
                Ok(operation.GetAnyOperationsAborted()?.as_bool())
            })();
            if initialised {
                CoUninitialize();
            }
            result
        }
    }

    pub(super) fn move_to_trash(path: &Path) -> Result<(), CommandError> {
        // The shell needs a full path, and does not take the `\\?\` form.
        let full = std::path::absolute(path).map_err(|error| {
            CommandError::Io("Could not move the recording to the Recycle Bin.".into())
                .because(error)
        })?;
        let text = full.to_string_lossy();
        let plain = text.strip_prefix(r"\\?\").unwrap_or(&text);
        // Fail closed: unless the bin is known to be able to take the folder,
        // nothing is called that could delete it for good.
        if let Err(refusal) = recycle_guaranteed(&bin_facts(Path::new(plain))) {
            return Err(refusal_error(refusal));
        }
        match recycle(plain) {
            Ok(false) => Ok(()),
            Ok(true) => Err(CommandError::Unavailable(
                "The recording was not moved: Windows could not send it to the Recycle Bin.".into(),
            )),
            Err(error) => Err(CommandError::Io(
                "Could not move the recording to the Recycle Bin.".into(),
            )
            .because(error)),
        }
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
