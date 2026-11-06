//! What kind of folder a recording is going into (design §8, decision D9).
//!
//! [`folder_warnings`] returns the reasons to warn the user before recording
//! into a folder; the UI of phase 1c shows them, this module only knows them.
//!
//! | Warning | Why it matters | How it is found |
//! |---|---|---|
//! | `Fat32` | a file stops at 4 GiB (about 14 h of 48 kHz / 24-bit mono speech, far less for noisy or multichannel audio) | macOS: `statfs` type `msdos`; Linux: `/proc/mounts` `vfat` |
//! | `ExFat` | no size limit, but no journal: a crash can lose more | macOS `exfat`; Linux `exfat` |
//! | `NetworkVolume` | writes stall and fail with the network; fsync is a promise only the server keeps | macOS: not `MNT_LOCAL`, or `smbfs`/`nfs`/`afpfs`/`webdav`; UNC paths; Linux mount types |
//! | `CloudSynced` | the sync client can lock or upload a growing folder | the folder is inside OneDrive, iCloud Drive, Dropbox, Google Drive or a macOS CloudStorage provider (by path) |
//!
//! **Not implemented here:** on Windows only the UNC check and the cloud
//! folders are detected; the volume's file system (`GetVolumeInformationW`) is
//! a seam for a Windows run, and nothing here could be compiled or run for
//! Windows on the development Mac. A folder this module cannot classify gets
//! no warning.

use std::path::Path;

/// A reason to warn before recording into a folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FolderWarning {
    /// FAT32: a file cannot exceed 4 GiB.
    Fat32,
    /// exFAT: no practical size limit, no journal.
    ExFat,
    /// A network volume.
    NetworkVolume,
    /// A folder a cloud client keeps in sync.
    CloudSynced,
}

/// The size a file on FAT32 cannot reach: 4 GiB minus one byte.
pub const FAT32_MAX_FILE_BYTES: u64 = 4 * 1024 * 1024 * 1024 - 1;
/// Where the session ends a track on FAT32: a margin below the limit, so the
/// write that would cross it never happens.
pub const FAT32_TRACK_LIMIT_BYTES: u64 = FAT32_MAX_FILE_BYTES - 64 * 1024 * 1024;

/// The warning a file-system type name implies (`statfs` or `/proc/mounts`).
pub fn classify_fs_type(name: &str) -> Option<FolderWarning> {
    match name.to_ascii_lowercase().as_str() {
        "msdos" | "vfat" | "fat" | "fat32" | "fat16" => Some(FolderWarning::Fat32),
        "exfat" | "fuseblk.exfat" => Some(FolderWarning::ExFat),
        "smbfs" | "smb" | "smb3" | "cifs" | "nfs" | "nfs4" | "afpfs" | "webdav" | "davfs"
        | "9p" | "ncpfs" | "fuse.sshfs" | "sshfs" => Some(FolderWarning::NetworkVolume),
        _ => None,
    }
}

/// Whether the path names a network location by its form (Windows UNC).
pub fn is_unc_path(path: &str) -> bool {
    let path = path.replace('/', "\\");
    let device_path = path.starts_with("\\\\?\\") || path.starts_with("\\\\.\\");
    (path.starts_with("\\\\") && !device_path)
        || path.to_ascii_lowercase().starts_with("\\\\?\\unc\\")
}

/// Whether a folder is inside one that a cloud client syncs, judged by the
/// names on the path.
pub fn is_cloud_synced_path(path: &Path) -> bool {
    // Both separators, so a Windows path is read the same on every platform.
    let text = path.to_string_lossy().replace('\\', "/");
    text.split('/').any(|component| {
        let name = component.to_lowercase();
        name == "onedrive"
            || name.starts_with("onedrive -")
            || name.starts_with("onedrive-")
            || name == "icloud drive"
            || name == "iclouddrive"
            || name == "mobile documents"
            || name == "dropbox"
            || name.starts_with("dropbox (")
            || name == "google drive"
            || name == "googledrive"
            || name == "cloudstorage"
            || name == "box sync"
            || name == "pcloud drive"
    })
}

/// The file-system type of the mount that holds `path`, from the text of
/// `/proc/mounts`: the longest mount point that is a prefix of the path.
pub fn fs_type_from_mounts(mounts: &str, path: &Path) -> Option<String> {
    let mut best: Option<(usize, &str)> = None;
    for line in mounts.lines() {
        let mut fields = line.split_whitespace();
        let (_device, Some(mount_point), Some(fs_type)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        // /proc/mounts escapes spaces as \040.
        let mount_point = mount_point.replace("\\040", " ");
        if path.starts_with(&mount_point) && best.is_none_or(|(len, _)| mount_point.len() >= len) {
            best = Some((mount_point.len(), fs_type));
        }
    }
    best.map(|(_, fs_type)| fs_type.to_string())
}

/// Every reason to warn about recording into `folder`, most serious first,
/// without duplicates. The folder need not exist yet: its nearest existing
/// parent is looked at.
pub fn folder_warnings(folder: &Path) -> Vec<FolderWarning> {
    let mut warnings = Vec::new();
    if is_unc_path(&folder.to_string_lossy()) {
        warnings.push(FolderWarning::NetworkVolume);
    }
    if is_cloud_synced_path(folder) {
        warnings.push(FolderWarning::CloudSynced);
    }
    warnings.extend(volume_warnings(folder));
    warnings.sort();
    warnings.dedup();
    warnings
}

fn existing_ancestor(folder: &Path) -> Option<&Path> {
    folder.ancestors().find(|path| path.exists())
}

#[cfg(target_vendor = "apple")]
fn volume_warnings(folder: &Path) -> Vec<FolderWarning> {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;

    let Some(path) = existing_ancestor(folder) else {
        return Vec::new();
    };
    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return Vec::new();
    };
    // SAFETY: `statfs` is plain old data for which all-zero is valid, and
    // `c_path` is a NUL-terminated string that outlives the call.
    let mut info: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the duration of the call.
    if unsafe { libc::statfs(c_path.as_ptr(), &mut info) } != 0 {
        return Vec::new();
    }
    // SAFETY: the kernel NUL-terminates `f_fstypename` within its 16 bytes.
    let name = unsafe { CStr::from_ptr(info.f_fstypename.as_ptr()) }.to_string_lossy();
    let mut warnings: Vec<FolderWarning> = classify_fs_type(&name).into_iter().collect();
    if info.f_flags & (libc::MNT_LOCAL as u32) == 0 {
        warnings.push(FolderWarning::NetworkVolume);
    }
    warnings
}

#[cfg(target_os = "linux")]
fn volume_warnings(folder: &Path) -> Vec<FolderWarning> {
    let Some(path) = existing_ancestor(folder).and_then(|p| p.canonicalize().ok()) else {
        return Vec::new();
    };
    std::fs::read_to_string("/proc/mounts")
        .ok()
        .and_then(|mounts| fs_type_from_mounts(&mounts, &path))
        .and_then(|name| classify_fs_type(&name))
        .into_iter()
        .collect()
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
fn volume_warnings(folder: &Path) -> Vec<FolderWarning> {
    // The Windows seam: GetVolumeInformationW for the file-system name and
    // GetDriveTypeW for remote drives, behind this function.
    let _ = existing_ancestor(folder);
    Vec::new()
}

#[cfg(test)]
#[path = "fsclass_tests.rs"]
mod tests;
