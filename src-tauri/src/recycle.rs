use std::path::{Component, Path, PathBuf, Prefix};

const GUID_LENGTH: usize = 36;
const BYTES_PER_MEGABYTE: u64 = 1024 * 1024;
const UNCONFIGURED_SHARE_OF_VOLUME: u64 = 20;
const USABLE_PERCENT_OF_BIN: u128 = 90;
const IN_DOUBT: Verdict = Verdict::Unknown {
    fits_default: false,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Recyclable,
    NoRecycleBin,
    TooBig,
    Unknown { fits_default: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bin {
    Present,
    Absent,
    Unknown,
}

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
    pub max_capacity: Setting,
    pub total_bytes: Option<u64>,
}

pub fn verdict(volume: &Volume, size: u64) -> Verdict {
    match bin_of(volume) {
        Bin::Absent => Verdict::NoRecycleBin,
        Bin::Unknown => Verdict::Unknown {
            fits_default: fits_in_recycle_bin(size, assumed_capacity(volume)),
        },
        Bin::Present if fits_in_recycle_bin(size, known_capacity(volume)) => Verdict::Recyclable,
        Bin::Present => Verdict::TooBig,
    }
}

// WHY: a "no" read from the drive or the registry is certain, while one that cannot be read leaves it in doubt
fn bin_of(volume: &Volume) -> Bin {
    let settings = [
        volume.nuke_on_delete,
        volume.user_policy,
        volume.machine_policy,
    ];
    if volume.drive != DriveKind::Fixed || volume.network_path || settings.into_iter().any(is_on) {
        return Bin::Absent;
    }
    if settings.contains(&Setting::Unreadable) {
        return Bin::Unknown;
    }
    Bin::Present
}

fn known_capacity(volume: &Volume) -> Option<u64> {
    capacity_bytes(volume.max_capacity, volume.total_bytes)
}

// WHY: with the bin in doubt a capacity that could be read still holds; otherwise the 5% default is assumed
fn assumed_capacity(volume: &Volume) -> Option<u64> {
    let readable = match volume.max_capacity {
        Setting::Unreadable => Setting::Absent,
        setting => setting,
    };
    capacity_bytes(readable, volume.total_bytes)
}

// WHY: asking a network or removable drive for its size can block for long while the service lock is held
pub fn needs_total(drive: DriveKind, network_path: bool, max_capacity: Setting) -> bool {
    drive == DriveKind::Fixed && !network_path && !matches!(max_capacity, Setting::Set(_))
}

// WHY: an unconfigured bin is sized from its volume; 5% stays under the Windows default, so it errs on refusing
pub fn capacity_bytes(max_capacity: Setting, total_bytes: Option<u64>) -> Option<u64> {
    match max_capacity {
        Setting::Set(megabytes) => Some(u64::from(megabytes).saturating_mul(BYTES_PER_MEGABYTE)),
        Setting::Absent => total_bytes.map(|total| total / UNCONFIGURED_SHARE_OF_VOLUME),
        Setting::Unreadable => None,
    }
}

// WHY: a file the bin cannot hold raises a blocking dialog; clusters and alternate streams make it take more than its length
pub fn fits_in_recycle_bin(size: u64, capacity: Option<u64>) -> bool {
    capacity.is_some_and(|capacity| {
        capacity > 0 && u128::from(size) * 100 <= u128::from(capacity) * USABLE_PERCENT_OF_BIN
    })
}

fn is_on(setting: Setting) -> bool {
    matches!(setting, Setting::Set(value) if value != 0)
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

pub fn judge(path: &Path, size: u64) -> Verdict {
    as_trash_resolves(path).map_or(IN_DOUBT, |resolved| judge_resolved(&resolved, size))
}

// WHY: trash resolves the folder before deleting, so a junction swapped in since the plan is judged where it leads
pub fn as_trash_resolves(path: &Path) -> Option<PathBuf> {
    if path.as_os_str().is_empty() {
        return None;
    }
    let absolute = absolute(path)?;
    let folder = absolute.parent()?.canonicalize().ok()?;
    Some(match absolute.file_name() {
        Some(name) => folder.join(name),
        None => folder,
    })
}

fn absolute(path: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    std::env::current_dir().ok().map(|folder| folder.join(path))
}

#[cfg(windows)]
fn judge_resolved(path: &Path, size: u64) -> Verdict {
    let path = dunce::simplified(path);
    system::volume_of(path).map_or(IN_DOUBT, |volume| verdict(&volume, size))
}

// TODO(fase-4): Linux makes a trash folder per mount or fails; confirm on a Mac (test E) it never deletes
#[cfg(not(windows))]
fn judge_resolved(_path: &Path, _size: u64) -> Verdict {
    Verdict::Recyclable
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
        GetDiskFreeSpaceExW, GetDriveTypeW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW,
    };
    use windows_sys::Win32::System::WindowsProgramming::{
        DRIVE_FIXED, DRIVE_REMOTE, DRIVE_REMOVABLE,
    };

    use super::{DriveKind, Setting, Volume, is_network_path, needs_total, volume_guid};

    const POLICY_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer";
    const VOLUMES_KEY: &str =
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume";
    const VOLUME_NAME_LENGTH: u32 = 50;
    const DWORD_SIZE: u32 = 4;

    pub fn volume_of(path: &Path) -> Option<Volume> {
        let root = volume_root(path)?;
        let bin = bin_key(&root);
        let (drive, network_path) = (drive_kind(&root), is_network_path(path));
        let max_capacity = bin_setting(bin.as_deref(), "MaxCapacity");
        let total_bytes = needs_total(drive, network_path, max_capacity)
            .then(|| total_bytes(&root))
            .flatten();
        Some(Volume {
            drive,
            network_path,
            nuke_on_delete: bin_setting(bin.as_deref(), "NukeOnDelete"),
            user_policy: dword(HKEY_CURRENT_USER, POLICY_KEY, "NoRecycleFiles"),
            machine_policy: dword(HKEY_LOCAL_MACHINE, POLICY_KEY, "NoRecycleFiles"),
            max_capacity,
            total_bytes,
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

    fn bin_key(root: &[u16]) -> Option<String> {
        let guid = volume_name(root).as_deref().and_then(volume_guid)?;
        Some(format!(r"{VOLUMES_KEY}\{guid}"))
    }

    fn bin_setting(key: Option<&str>, value: &str) -> Setting {
        key.map_or(Setting::Unreadable, |key| {
            dword(HKEY_CURRENT_USER, key, value)
        })
    }

    fn total_bytes(root: &[u16]) -> Option<u64> {
        let mut total: u64 = 0;
        // SAFETY: `root` ends in NUL, `total` is a live u64, and the two null outputs are optional
        let found = unsafe {
            GetDiskFreeSpaceExW(
                root.as_ptr(),
                std::ptr::null_mut(),
                &mut total,
                std::ptr::null_mut(),
            )
        };
        (found != 0).then_some(total)
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
            max_capacity: Setting::Set(1),
            total_bytes: Some(100 * MEGABYTE),
        }
    }

    const MEGABYTE: u64 = 1024 * 1024;

    #[test]
    fn the_capacity_is_max_capacity_in_megabytes() {
        assert_eq!(
            capacity_bytes(Setting::Set(1), Some(100 * MEGABYTE)),
            Some(MEGABYTE)
        );
        assert_eq!(capacity_bytes(Setting::Set(0), Some(MEGABYTE)), Some(0));
    }

    #[test]
    fn without_max_capacity_the_bin_is_taken_as_five_percent_of_the_volume() {
        assert_eq!(
            capacity_bytes(Setting::Absent, Some(1_000 * MEGABYTE)),
            Some(50 * MEGABYTE)
        );
    }

    #[test]
    fn a_capacity_that_cannot_be_worked_out_is_unknown() {
        for (setting, total) in [
            (Setting::Unreadable, Some(1_000 * MEGABYTE)),
            (Setting::Absent, None),
            (Setting::Unreadable, None),
        ] {
            assert_eq!(
                capacity_bytes(setting, total),
                None,
                "{setting:?} {total:?}"
            );
        }
    }

    #[test]
    fn a_file_fits_only_within_nine_tenths_of_the_bin() {
        for (size, capacity, expected) in [
            (500_000, Some(1_000_000), true),
            (900_000, Some(1_000_000), true),
            (900_001, Some(1_000_000), false),
            (1_000_000, Some(1_000_000), false),
            (3 * MEGABYTE, Some(MEGABYTE), false),
            (0, Some(0), false),
            (1, None, false),
        ] {
            assert_eq!(
                fits_in_recycle_bin(size, capacity),
                expected,
                "{size} bytes in {capacity:?}"
            );
        }
    }

    #[test]
    fn a_file_larger_than_the_bin_is_too_big() {
        assert_eq!(verdict(&local_fixed(), 3 * MEGABYTE), Verdict::TooBig);
        assert_eq!(verdict(&local_fixed(), MEGABYTE / 2), Verdict::Recyclable);
    }

    #[test]
    fn a_drive_without_bin_says_so_whatever_the_size() {
        let network = Volume {
            drive: DriveKind::Remote,
            ..local_fixed()
        };

        assert_eq!(verdict(&network, 1), Verdict::NoRecycleBin);
        assert_eq!(verdict(&network, 3 * MEGABYTE), Verdict::NoRecycleBin);
    }

    #[test]
    fn a_local_fixed_drive_keeps_deleted_files() {
        assert_eq!(bin_of(&local_fixed()), Bin::Present);
    }

    #[test]
    fn only_a_fixed_drive_has_a_recycle_bin() {
        for (drive, expected) in [
            (DriveKind::Fixed, Bin::Present),
            (DriveKind::Remote, Bin::Absent),
            (DriveKind::Removable, Bin::Absent),
            (DriveKind::Other, Bin::Absent),
        ] {
            let volume = Volume {
                drive,
                ..local_fixed()
            };

            assert_eq!(bin_of(&volume), expected, "{drive:?}");
        }
    }

    #[test]
    fn a_network_path_never_counts_even_when_reported_as_fixed() {
        let volume = Volume {
            network_path: true,
            ..local_fixed()
        };

        assert_eq!(bin_of(&volume), Bin::Absent);
    }

    #[test]
    fn deleting_for_good_on_the_volume_switches_the_bin_off() {
        let volume = Volume {
            nuke_on_delete: Setting::Set(1),
            ..local_fixed()
        };

        assert_eq!(bin_of(&volume), Bin::Absent);
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

        assert_eq!(bin_of(&by_user), Bin::Absent);
        assert_eq!(bin_of(&by_machine), Bin::Absent);
    }

    #[test]
    fn a_setting_that_cannot_be_read_leaves_the_bin_in_doubt() {
        for unreadable in [
            Volume {
                nuke_on_delete: Setting::Unreadable,
                ..local_fixed()
            },
            Volume {
                user_policy: Setting::Unreadable,
                ..local_fixed()
            },
            Volume {
                machine_policy: Setting::Unreadable,
                ..local_fixed()
            },
        ] {
            assert_eq!(bin_of(&unreadable), Bin::Unknown, "{unreadable:?}");
        }
    }

    #[test]
    fn a_drive_known_to_keep_nothing_says_so_even_with_other_settings_unreadable() {
        for known in [
            Volume {
                nuke_on_delete: Setting::Set(1),
                user_policy: Setting::Unreadable,
                ..local_fixed()
            },
            Volume {
                drive: DriveKind::Removable,
                nuke_on_delete: Setting::Unreadable,
                ..local_fixed()
            },
        ] {
            assert_eq!(bin_of(&known), Bin::Absent, "{known:?}");
        }
    }

    #[test]
    fn only_a_setting_other_than_zero_switches_the_bin_off() {
        for (setting, expected) in [
            (Setting::Absent, false),
            (Setting::Set(0), false),
            (Setting::Set(1), true),
            (Setting::Set(7), true),
            (Setting::Unreadable, false),
        ] {
            assert_eq!(is_on(setting), expected, "{setting:?}");
        }
    }

    fn in_doubt(total_bytes: Option<u64>) -> Volume {
        Volume {
            nuke_on_delete: Setting::Unreadable,
            max_capacity: Setting::Unreadable,
            total_bytes,
            ..local_fixed()
        }
    }

    #[test]
    fn a_bin_in_doubt_is_measured_as_five_percent_of_its_volume() {
        let volume = in_doubt(Some(100 * MEGABYTE));

        assert_eq!(
            verdict(&volume, 4 * MEGABYTE),
            Verdict::Unknown { fits_default: true }
        );
        assert_eq!(
            verdict(&volume, 5 * MEGABYTE),
            Verdict::Unknown {
                fits_default: false
            }
        );
    }

    #[test]
    fn a_bin_in_doubt_on_a_volume_of_unknown_size_holds_nothing() {
        assert_eq!(
            verdict(&in_doubt(None), 1),
            Verdict::Unknown {
                fits_default: false
            }
        );
    }

    #[test]
    fn a_bin_in_doubt_still_honours_a_capacity_it_could_read() {
        let volume = Volume {
            user_policy: Setting::Unreadable,
            max_capacity: Setting::Set(1),
            ..in_doubt(Some(100 * MEGABYTE))
        };

        assert_eq!(
            verdict(&volume, 2 * MEGABYTE),
            Verdict::Unknown {
                fits_default: false
            }
        );
    }

    #[test]
    fn the_volume_size_is_asked_only_of_a_local_fixed_drive_with_no_capacity_set() {
        for (drive, network_path, max_capacity, expected) in [
            (DriveKind::Fixed, false, Setting::Absent, true),
            (DriveKind::Fixed, false, Setting::Unreadable, true),
            (DriveKind::Fixed, false, Setting::Set(1), false),
            (DriveKind::Fixed, true, Setting::Absent, false),
            (DriveKind::Remote, false, Setting::Absent, false),
            (DriveKind::Removable, false, Setting::Absent, false),
            (DriveKind::Other, false, Setting::Absent, false),
        ] {
            assert_eq!(
                needs_total(drive, network_path, max_capacity),
                expected,
                "{drive:?} network={network_path} {max_capacity:?}"
            );
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

        assert_eq!(judge(&file, 7), Verdict::Recyclable);
        assert_eq!(
            judge(&file.canonicalize().unwrap(), 7),
            Verdict::Recyclable,
            "the canonical form the scan produces must be judged the same"
        );
    }

    #[test]
    fn the_file_is_judged_where_trash_will_find_it_through_a_link() {
        let base = tempfile::TempDir::new().unwrap();
        let real = base.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let swapped = base.path().join("swapped");
        crate::testing::link_folder(&swapped, &real);

        let resolved = as_trash_resolves(&swapped.join("probe.log")).unwrap();

        assert_eq!(resolved, real.canonicalize().unwrap().join("probe.log"));
    }

    #[test]
    fn a_file_whose_folder_cannot_be_resolved_leaves_the_bin_in_doubt() {
        let folder = tempfile::TempDir::new().unwrap();
        let ghost = folder.path().join("gone").join("probe.log");
        let holding_nothing = Verdict::Unknown {
            fits_default: false,
        };

        assert_eq!(as_trash_resolves(&ghost), None);
        assert_eq!(judge(&ghost, 7), holding_nothing);
        assert_eq!(judge(Path::new(""), 7), holding_nothing);
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
        assert_eq!(judge(&canonical, 7), Verdict::Recyclable);
    }
}
