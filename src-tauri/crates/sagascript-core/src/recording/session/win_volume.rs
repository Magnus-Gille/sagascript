//! Windows only: what kind of volume a folder is on.
//!
//! `GetVolumePathNameW` finds the volume's root (a drive letter or a UNC
//! share), `GetDriveTypeW` says whether it is a network drive and
//! `GetVolumeInformationW` names its file system (`NTFS`, `FAT32`, `exFAT`).
//! The mapping to warnings is in `fsclass.rs`.
//!
//! This file depends on `windows-sys` and `std` only, so it can be compiled
//! on its own for a Windows target.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::{
    GetDriveTypeW, GetVolumeInformationW, GetVolumePathNameW,
};
use windows_sys::Win32::System::WindowsProgramming::DRIVE_REMOTE;

/// What Windows says about the volume of a path.
pub(super) struct VolumeInfo {
    /// `NTFS`, `FAT32`, `exFAT`, `ReFS`, ...
    pub fs_name: Option<String>,
    /// A mapped network drive or a UNC share.
    pub remote: bool,
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

fn from_wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// Look at the volume that holds `path` (which must exist).
pub(super) fn volume_info(path: &Path) -> Option<VolumeInfo> {
    let path = wide(path.as_os_str());
    let mut root = vec![0u16; 1024];
    // SAFETY: `path` is NUL terminated and `root` has the length passed.
    if unsafe { GetVolumePathNameW(path.as_ptr(), root.as_mut_ptr(), root.len() as u32) } == 0 {
        return None;
    }
    // SAFETY: `root` was NUL terminated by the call above.
    let remote = unsafe { GetDriveTypeW(root.as_ptr()) } == DRIVE_REMOTE;
    let mut fs = vec![0u16; 64];
    // SAFETY: the buffers are valid for the lengths passed; the unused
    // out-parameters are null, which the function allows.
    let ok = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            fs.as_mut_ptr(),
            fs.len() as u32,
        )
    };
    Some(VolumeInfo {
        fs_name: (ok != 0).then(|| from_wide(&fs)),
        remote,
    })
}
