use std::path::Path;

use super::*;

#[test]
fn file_system_type_names_map_to_warnings() {
    for name in ["msdos", "vfat", "FAT32"] {
        assert_eq!(classify_fs_type(name), Some(FolderWarning::Fat32), "{name}");
    }
    assert_eq!(classify_fs_type("exfat"), Some(FolderWarning::ExFat));
    for name in ["smbfs", "nfs", "afpfs", "webdav", "cifs", "NFS4"] {
        assert_eq!(
            classify_fs_type(name),
            Some(FolderWarning::NetworkVolume),
            "{name}"
        );
    }
    for name in ["apfs", "hfs", "ntfs", "ext4", "btrfs", "xfs", "tmpfs", ""] {
        assert_eq!(classify_fs_type(name), None, "{name}");
    }
}

#[test]
fn unc_paths_are_network_locations_and_device_paths_are_not() {
    assert!(is_unc_path(r"\\server\share\Recordings"));
    assert!(is_unc_path("//server/share/Recordings"));
    assert!(is_unc_path(r"\\?\UNC\server\share"));
    assert!(!is_unc_path(r"C:\Users\me\Music"));
    assert!(!is_unc_path(r"\\?\C:\Users\me"));
    assert!(!is_unc_path("/Users/me/Music"));
}

#[test]
fn cloud_synced_folders_are_found_by_name() {
    for path in [
        "/Users/me/Library/Mobile Documents/com~apple~CloudDocs/Rec",
        "/Users/me/Library/CloudStorage/OneDrive-Contoso/Rec",
        "/Users/me/OneDrive - Contoso/Music/Sagascript",
        "C:/Users/me/OneDrive/Music",
        "/Users/me/Dropbox/Rec",
        "/Users/me/Dropbox (Personal)/Rec",
        "/Volumes/GoogleDrive/My Drive",
        "/Users/me/Google Drive/Rec",
        "/Users/me/iCloud Drive/Rec",
    ] {
        assert!(is_cloud_synced_path(Path::new(path)), "{path}");
    }
    for path in [
        "/Users/me/Music/Sagascript",
        "/Users/me/Documents/OneDriveNotes",
        "/Users/me/droplets",
        "/tmp/rec",
    ] {
        assert!(!is_cloud_synced_path(Path::new(path)), "{path}");
    }
}

#[test]
fn the_longest_mount_point_decides_in_a_proc_mounts_listing() {
    let mounts = "\
/dev/sda1 / ext4 rw 0 0
/dev/sdb1 /media/usb vfat rw 0 0
server:/export /media/usb/nas nfs4 rw 0 0
/dev/sdc1 /media/My\\040Disk exfat rw 0 0
";
    let fs = |path| fs_type_from_mounts(mounts, Path::new(path));
    assert_eq!(fs("/home/me/Music").as_deref(), Some("ext4"));
    assert_eq!(fs("/media/usb/Rec").as_deref(), Some("vfat"));
    assert_eq!(fs("/media/usb/nas/Rec").as_deref(), Some("nfs4"));
    assert_eq!(fs("/media/My Disk/Rec").as_deref(), Some("exfat"));
}

#[test]
fn an_ordinary_local_folder_has_no_warning_even_if_it_does_not_exist_yet() {
    let temp = sagascript_testsupport::TempDir::new();
    let folder = temp.path().join("Sagascript");
    // A temp dir is a local disk (APFS, ext4, NTFS, tmpfs...).
    assert_eq!(folder_warnings(&folder), Vec::<FolderWarning>::new());
}

#[test]
fn warnings_combine_and_are_sorted_without_duplicates() {
    // A UNC path inside OneDrive: both, once each.
    let warnings = folder_warnings(Path::new(r"\\server\share\OneDrive\Rec"));
    assert_eq!(
        warnings,
        vec![FolderWarning::NetworkVolume, FolderWarning::CloudSynced]
    );
}

#[test]
fn the_fat32_limit_is_below_four_gibibytes() {
    assert_eq!(FAT32_MAX_FILE_BYTES, 4_294_967_295);
    const { assert!(FAT32_TRACK_LIMIT_BYTES < FAT32_MAX_FILE_BYTES - 60_000_000) };
}

#[test]
fn what_windows_reports_about_a_volume_becomes_the_same_warnings() {
    assert_eq!(warnings_for_volume(Some("NTFS"), false), vec![]);
    assert_eq!(warnings_for_volume(Some("ReFS"), false), vec![]);
    assert_eq!(
        warnings_for_volume(Some("FAT32"), false),
        vec![FolderWarning::Fat32]
    );
    assert_eq!(
        warnings_for_volume(Some("FAT"), false),
        vec![FolderWarning::Fat32]
    );
    assert_eq!(
        warnings_for_volume(Some("exFAT"), false),
        vec![FolderWarning::ExFat]
    );
    assert_eq!(
        warnings_for_volume(Some("NTFS"), true),
        vec![FolderWarning::NetworkVolume]
    );
    assert_eq!(
        warnings_for_volume(Some("FAT32"), true),
        vec![FolderWarning::Fat32, FolderWarning::NetworkVolume]
    );
    assert_eq!(warnings_for_volume(None, false), vec![]);
}
