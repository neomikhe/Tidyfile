use std::path::{Component, Path, Prefix};

const GUID_LENGTH: usize = 36;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveKind {
    Fixed,
    Removable,
    Remote,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Absent,
    Set(u32),
    Unreadable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Volume {
    pub drive: DriveKind,
    pub network_path: bool,
    pub nuke_on_delete: Setting,
    pub user_policy: Setting,
    pub machine_policy: Setting,
}

pub fn keeps_deleted_files(volume: &Volume) -> bool {
    let settings = [
        volume.nuke_on_delete,
        volume.user_policy,
        volume.machine_policy,
    ];
    volume.drive == DriveKind::Fixed && !volume.network_path && settings.into_iter().all(is_off)
}

// WHY: a setting we cannot read may be the one that deletes for good, so it counts as switched on
pub fn is_off(setting: Setting) -> bool {
    matches!(setting, Setting::Absent | Setting::Set(0))
}

pub fn volume_guid(volume_name: &str) -> Option<String> {
    let (_, rest) = volume_name.split_once('{')?;
    let (inner, _) = rest.split_once('}')?;
    let is_guid = inner.len() == GUID_LENGTH
        && inner
            .chars()
            .all(|character| character.is_ascii_hexdigit() || character == '-');
    is_guid.then(|| format!("{{{inner}}}"))
}

pub fn is_network_path(path: &Path) -> bool {
    match path.components().next() {
        Some(Component::Prefix(prefix)) => {
            matches!(prefix.kind(), Prefix::UNC(..) | Prefix::VerbatimUNC(..))
        }
        _ => false,
    }
}

#[cfg(windows)]
pub fn has_recycle_bin(path: &Path) -> bool {
    let path = dunce::simplified(path);
    system::volume_of(path).is_some_and(|volume| keeps_deleted_files(&volume))
}

// TODO(fase-4): Linux makes a trash folder per mount or fails; confirm on a Mac (test E) it never deletes
#[cfg(not(windows))]
pub fn has_recycle_bin(_path: &Path) -> bool {
    true
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod system {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS, MAX_PATH,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        GetDriveTypeW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW,
    };
    use windows_sys::Win32::System::WindowsProgramming::{
        DRIVE_FIXED, DRIVE_REMOTE, DRIVE_REMOVABLE,
    };

    use super::{DriveKind, Setting, Volume, is_network_path, volume_guid};

    const POLICY_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer";
    const VOLUMES_KEY: &str =
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume";
    const VOLUME_NAME_LENGTH: u32 = 50;
    const DWORD_SIZE: u32 = 4;

    pub fn volume_of(path: &Path) -> Option<Volume> {
        let root = volume_root(path)?;
        Some(Volume {
            drive: drive_kind(&root),
            network_path: is_network_path(path),
            nuke_on_delete: nuke_on_delete(&root),
            user_policy: dword(HKEY_CURRENT_USER, POLICY_KEY, "NoRecycleFiles"),
            machine_policy: dword(HKEY_LOCAL_MACHINE, POLICY_KEY, "NoRecycleFiles"),
        })
    }

    fn volume_root(path: &Path) -> Option<Vec<u16>> {
        let name = wide(path.as_os_str());
        let mut root = vec![0u16; name.len().max(MAX_PATH as usize + 1)];
        let capacity = u32::try_from(root.len()).ok()?;
        // SAFETY: `name` ends in NUL and `root` has `capacity` writable units for the whole call
        let found = unsafe { GetVolumePathNameW(name.as_ptr(), root.as_mut_ptr(), capacity) };
        (found != 0).then_some(root)
    }

    fn drive_kind(root: &[u16]) -> DriveKind {
        // SAFETY: `root` is the NUL-terminated volume path that GetVolumePathNameW wrote
        match unsafe { GetDriveTypeW(root.as_ptr()) } {
            DRIVE_FIXED => DriveKind::Fixed,
            DRIVE_REMOVABLE => DriveKind::Removable,
            DRIVE_REMOTE => DriveKind::Remote,
            _ => DriveKind::Other,
        }
    }

    fn nuke_on_delete(root: &[u16]) -> Setting {
        let Some(guid) = volume_name(root).as_deref().and_then(volume_guid) else {
            return Setting::Unreadable;
        };
        dword(
            HKEY_CURRENT_USER,
            &format!(r"{VOLUMES_KEY}\{guid}"),
            "NukeOnDelete",
        )
    }

    fn volume_name(root: &[u16]) -> Option<String> {
        let mut name = [0u16; VOLUME_NAME_LENGTH as usize];
        // SAFETY: `root` ends in NUL and `name` has VOLUME_NAME_LENGTH writable units for the call
        let found = unsafe {
            GetVolumeNameForVolumeMountPointW(root.as_ptr(), name.as_mut_ptr(), VOLUME_NAME_LENGTH)
        };
        if found == 0 {
            return None;
        }
        let text = name.split(|unit| *unit == 0).next()?;
        String::from_utf16(text).ok()
    }

    fn dword(hive: HKEY, key: &str, value: &str) -> Setting {
        let (key, value) = (wide(OsStr::new(key)), wide(OsStr::new(value)));
        let mut data: u32 = 0;
        let mut size = DWORD_SIZE;
        // SAFETY: both names end in NUL and `data` is a live u32 whose byte size `size` states
        let status = unsafe {
            RegGetValueW(
                hive,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&raw mut data).cast(),
                &mut size,
            )
        };
        match status {
            ERROR_SUCCESS => Setting::Set(data),
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Setting::Absent,
            _ => Setting::Unreadable,
        }
    }

    fn wide(text: &OsStr) -> Vec<u16> {
        text.encode_wide().chain(std::iter::once(0)).collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn local_fixed() -> Volume {
        Volume {
            drive: DriveKind::Fixed,
            network_path: false,
            nuke_on_delete: Setting::Absent,
            user_policy: Setting::Absent,
            machine_policy: Setting::Absent,
        }
    }

    #[test]
    fn a_local_fixed_drive_keeps_deleted_files() {
        assert!(keeps_deleted_files(&local_fixed()));
    }

    #[test]
    fn only_a_fixed_drive_has_a_recycle_bin() {
        for (drive, expected) in [
            (DriveKind::Fixed, true),
            (DriveKind::Remote, false),
            (DriveKind::Removable, false),
            (DriveKind::Other, false),
        ] {
            let volume = Volume {
                drive,
                ..local_fixed()
            };

            assert_eq!(keeps_deleted_files(&volume), expected, "{drive:?}");
        }
    }

    #[test]
    fn a_network_path_never_counts_even_when_reported_as_fixed() {
        let volume = Volume {
            network_path: true,
            ..local_fixed()
        };

        assert!(!keeps_deleted_files(&volume));
    }

    #[test]
    fn deleting_for_good_on_the_volume_switches_the_bin_off() {
        let volume = Volume {
            nuke_on_delete: Setting::Set(1),
            ..local_fixed()
        };

        assert!(!keeps_deleted_files(&volume));
    }

    #[test]
    fn the_no_recycle_policy_switches_it_off_for_the_user_or_the_machine() {
        let by_user = Volume {
            user_policy: Setting::Set(1),
            ..local_fixed()
        };
        let by_machine = Volume {
            machine_policy: Setting::Set(1),
            ..local_fixed()
        };

        assert!(!keeps_deleted_files(&by_user));
        assert!(!keeps_deleted_files(&by_machine));
    }

    #[test]
    fn a_setting_that_cannot_be_read_counts_as_no_recycle_bin() {
        let volume = Volume {
            nuke_on_delete: Setting::Unreadable,
            ..local_fixed()
        };

        assert!(!keeps_deleted_files(&volume));
    }

    #[test]
    fn only_an_absent_or_zero_setting_is_off() {
        for (setting, expected) in [
            (Setting::Absent, true),
            (Setting::Set(0), true),
            (Setting::Set(1), false),
            (Setting::Set(7), false),
            (Setting::Unreadable, false),
        ] {
            assert_eq!(is_off(setting), expected, "{setting:?}");
        }
    }

    #[test]
    fn the_volume_guid_is_taken_from_the_volume_name() {
        let name = r"\\?\Volume{2f6a7c1e-0b1d-4c2e-9a3f-5d6e7f8a9b0c}\";

        assert_eq!(
            volume_guid(name).as_deref(),
            Some("{2f6a7c1e-0b1d-4c2e-9a3f-5d6e7f8a9b0c}")
        );
    }

    #[test]
    fn a_volume_name_without_a_well_formed_guid_gives_none() {
        for name in [
            r"\\?\Volume\",
            r"\\?\Volume{not-a-guid}\",
            r"\\?\Volume{2f6a7c1e-0b1d-4c2e-9a3f-5d6e7f8a9b0c\..\x}\",
            "",
        ] {
            assert_eq!(volume_guid(name), None, "{name}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn shared_folders_are_network_paths_and_drive_letters_are_not() {
        for (path, expected) in [
            (r"\\server\share\file.log", true),
            (r"\\?\UNC\server\share\file.log", true),
            (r"C:\Data\file.log", false),
            (r"\\?\C:\Data\file.log", false),
            (r"Z:\file.log", false),
        ] {
            assert_eq!(is_network_path(Path::new(path)), expected, "{path}");
        }
    }

    #[test]
    fn a_temp_folder_on_the_system_drive_has_a_recycle_bin() {
        let folder = tempfile::TempDir::new().unwrap();
        let file = folder.path().join("probe.log");
        std::fs::write(&file, "content").unwrap();

        assert!(has_recycle_bin(&file));
        assert!(
            has_recycle_bin(&file.canonicalize().unwrap()),
            "the canonical form the scan produces must be judged the same"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_path_longer_than_the_old_limit_is_judged_on_its_volume() {
        let folder = tempfile::TempDir::new().unwrap();
        let deep = (0..30).fold(folder.path().to_path_buf(), |path, level| {
            path.join(format!("level-{level:02}"))
        });
        std::fs::create_dir_all(&deep).unwrap();
        let file = deep.join("probe.log");
        std::fs::write(&file, "content").unwrap();
        let canonical = file.canonicalize().unwrap();

        assert!(canonical.as_os_str().len() > 260);
        assert!(has_recycle_bin(&canonical));
    }
}
