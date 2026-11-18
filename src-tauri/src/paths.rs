use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PathError {
    #[error("the folder could not be resolved")]
    Unresolvable,
    #[error("the path is not a folder")]
    NotAFolder,
    #[error("this folder cannot be watched")]
    Forbidden,
}

#[cfg(windows)]
const FORBIDDEN_ROOTS: [&str; 6] = [
    "windows",
    "program files",
    "program files (x86)",
    "programdata",
    "$recycle.bin",
    "system volume information",
];

#[cfg(not(windows))]
const FORBIDDEN_ROOTS: [&str; 17] = [
    "usr",
    "bin",
    "sbin",
    "etc",
    "var",
    "boot",
    "lib",
    "lib64",
    "opt",
    "proc",
    "sys",
    "dev",
    "root",
    "system",
    "library",
    "applications",
    "private",
];

pub fn accept_watched_folder(candidate: &Path) -> Result<PathBuf, PathError> {
    let resolved = candidate
        .canonicalize()
        .map_err(|_| PathError::Unresolvable)?;
    if !resolved.is_dir() {
        return Err(PathError::NotAFolder);
    }
    if is_forbidden(&resolved, home_folder().as_deref()) {
        return Err(PathError::Forbidden);
    }
    Ok(resolved)
}

pub fn is_forbidden(resolved: &Path, home: Option<&Path>) -> bool {
    if is_filesystem_root(resolved) {
        return true;
    }
    if home.is_some_and(|home| home.starts_with(resolved)) {
        return true;
    }
    if cfg!(unix) && is_user_space_under_a_system_root(resolved, home) {
        return false;
    }
    first_normal_component(resolved).is_some_and(|first| FORBIDDEN_ROOTS.contains(&first.as_str()))
}

// WHY: Fedora Atomic keeps homes in /var/home and mounts in /var/mnt, under the forbidden /var
fn is_user_space_under_a_system_root(resolved: &Path, home: Option<&Path>) -> bool {
    let mounts = Path::new("/var/mnt");
    let in_own_home =
        home.is_some_and(|home| is_kept_in_a_home_folder(home) && resolved.starts_with(home));
    in_own_home || (resolved.starts_with(mounts) && resolved != mounts)
}

fn is_kept_in_a_home_folder(home: &Path) -> bool {
    home.parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == OsStr::new("home"))
}

pub fn is_within(root: &Path, candidate: &Path) -> bool {
    candidate.starts_with(root)
}

fn is_filesystem_root(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, Component::Prefix(_) | Component::RootDir))
}

fn first_normal_component(path: &Path) -> Option<String> {
    path.components().find_map(|component| match component {
        Component::Normal(name) => name.to_str().map(str::to_ascii_lowercase),
        _ => None,
    })
}

fn home_folder() -> Option<PathBuf> {
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
        .and_then(|home| home.canonicalize().ok())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[cfg(windows)]
    fn system_examples() -> [&'static str; 4] {
        [
            r"C:\Windows",
            r"C:\Windows\System32",
            r"C:\Program Files\App",
            r"C:\ProgramData\Thing",
        ]
    }

    #[cfg(not(windows))]
    fn system_examples() -> [&'static str; 4] {
        ["/usr", "/usr/local/bin", "/etc/systemd", "/var/log"]
    }

    #[cfg(windows)]
    fn root_examples() -> [&'static str; 2] {
        [r"C:\", r"D:\"]
    }

    #[cfg(not(windows))]
    fn root_examples() -> [&'static str; 2] {
        ["/", "//"]
    }

    #[cfg(windows)]
    fn ordinary_examples() -> [&'static str; 3] {
        [
            r"C:\Users\someone\Downloads",
            r"D:\Archive\2026",
            r"C:\Users\someone\Documents\Invoices",
        ]
    }

    #[cfg(not(windows))]
    fn ordinary_examples() -> [&'static str; 3] {
        [
            "/home/someone/Downloads",
            "/mnt/archive/2026",
            "/home/someone/Documents/Invoices",
        ]
    }

    #[test]
    fn system_folders_are_refused() {
        for candidate in system_examples() {
            assert!(
                is_forbidden(Path::new(candidate), None),
                "{candidate} should be refused"
            );
        }
    }

    #[test]
    fn filesystem_roots_are_refused() {
        for candidate in root_examples() {
            assert!(
                is_forbidden(Path::new(candidate), None),
                "{candidate} should be refused"
            );
        }
    }

    #[test]
    fn ordinary_user_folders_are_accepted() {
        for candidate in ordinary_examples() {
            assert!(
                !is_forbidden(Path::new(candidate), None),
                "{candidate} should be accepted"
            );
        }
    }

    #[test]
    fn the_whole_home_folder_is_refused_but_its_children_are_not() {
        let home = Path::new(ordinary_examples()[0]).parent().unwrap();

        assert!(is_forbidden(home, Some(home)));
        assert!(!is_forbidden(&home.join("Downloads"), Some(home)));
    }

    fn is_user_space(folder: &str, home: &str) -> bool {
        is_user_space_under_a_system_root(Path::new(folder), Some(Path::new(home)))
    }

    #[test]
    fn a_home_kept_in_a_home_folder_is_user_space_even_under_var() {
        assert!(is_user_space(
            "/var/home/someone/Downloads",
            "/var/home/someone"
        ));
        assert!(is_user_space(
            "/usr/home/someone/Documents",
            "/usr/home/someone"
        ));
    }

    #[test]
    fn homes_of_root_services_or_odd_variables_unlock_nothing() {
        let refused = [
            ("/var/log", "/var"),
            ("/usr/local/bin", "/usr/local"),
            ("/private/var/db", "/private/var"),
            ("/private/var/root/Library", "/private/var/root"),
            ("/var/roothome/x", "/var/roothome"),
            ("/var/lib/postgresql/data", "/var/lib/postgresql"),
            ("/var/home/other/Downloads", "/var/home/someone"),
        ];

        for (folder, home) in refused {
            assert!(
                !is_user_space(folder, home),
                "{folder} must not count as user space with home {home}"
            );
        }
    }

    #[test]
    fn only_what_is_mounted_inside_var_mnt_is_user_space() {
        assert!(is_user_space_under_a_system_root(
            Path::new("/var/mnt/disk"),
            None
        ));
        assert!(!is_user_space_under_a_system_root(
            Path::new("/var/mnt"),
            None
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_fedora_atomic_home_can_be_watched_but_the_rest_of_var_cannot() {
        let home = Path::new("/var/home/someone");

        assert!(!is_forbidden(
            Path::new("/var/home/someone/Downloads"),
            Some(home)
        ));
        assert!(!is_forbidden(Path::new("/var/mnt/disk"), Some(home)));
        assert!(
            is_forbidden(home, Some(home)),
            "the whole home stays refused"
        );
        assert!(is_forbidden(Path::new("/var/home"), Some(home)));
        assert!(is_forbidden(Path::new("/var/log"), Some(home)));
    }

    #[test]
    fn a_folder_that_holds_the_home_is_refused_like_the_home_itself() {
        let home = Path::new(ordinary_examples()[0]).parent().unwrap();
        let above = home.parent().unwrap();

        assert!(
            is_forbidden(above, Some(home)),
            "{above:?} contains {home:?}"
        );
    }

    #[test]
    fn a_home_at_the_filesystem_root_does_not_unlock_the_system() {
        let root = Path::new(root_examples()[0]);

        for candidate in system_examples() {
            assert!(
                is_forbidden(Path::new(candidate), Some(root)),
                "{candidate} must stay refused with home at {root:?}"
            );
        }
    }

    #[test]
    fn the_forbidden_name_must_be_the_first_component() {
        let nested = Path::new(ordinary_examples()[0]).join(FORBIDDEN_ROOTS[0]);

        assert!(
            !is_forbidden(&nested, None),
            "a folder merely named like a system one is fine: {nested:?}"
        );
    }

    #[test]
    fn a_real_folder_resolves_to_an_absolute_path() {
        let folder = crate::testing::watchable_folder();

        let accepted = accept_watched_folder(folder.path()).unwrap();

        assert!(accepted.is_absolute());
        assert!(accepted.is_dir());
    }

    #[test]
    fn a_file_is_not_a_watchable_folder() {
        let folder = TempDir::new().unwrap();
        let file = folder.path().join("a.txt");
        std::fs::write(&file, "x").unwrap();

        assert_eq!(
            accept_watched_folder(&file).unwrap_err(),
            PathError::NotAFolder
        );
    }

    #[test]
    fn a_missing_folder_is_refused() {
        let folder = TempDir::new().unwrap();

        assert_eq!(
            accept_watched_folder(&folder.path().join("ghost")).unwrap_err(),
            PathError::Unresolvable
        );
    }

    #[test]
    fn containment_follows_the_resolved_root() {
        let root = Path::new(ordinary_examples()[0]);

        assert!(is_within(root, &root.join("sub").join("a.pdf")));
        assert!(!is_within(root, Path::new(ordinary_examples()[1])));
    }
}
