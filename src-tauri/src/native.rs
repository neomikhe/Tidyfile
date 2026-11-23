use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

pub type Identity = (u64, u64);

// WHY: sharing read and delete only, so no other program can hold the file open for writing meanwhile
#[cfg(windows)]
pub fn open_guarded(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ};
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .open(path)
}

// WHY: Unix has no mandatory locks to deny a writer, so the snapshot taken before and after is the check
#[cfg(not(windows))]
pub fn open_guarded(path: &Path) -> io::Result<File> {
    File::open(path)
}

#[cfg(windows)]
pub fn is_in_use(error: &io::Error) -> bool {
    use windows_sys::Win32::Foundation::{ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION};
    matches!(
        code_of(error),
        Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
    )
}

#[cfg(windows)]
fn code_of(error: &io::Error) -> Option<u32> {
    error
        .raw_os_error()
        .and_then(|code| u32::try_from(code).ok())
}

#[cfg(windows)]
pub fn open_slot(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
    OpenOptions::new()
        .write(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

// WHY: a FIFO swapped into the slot would block the open until a reader came, so the open never waits
#[cfg(unix)]
pub fn open_slot(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

#[cfg(windows)]
pub fn identity_at(path: &Path) -> Option<Identity> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let file = OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .ok()?;
    identity_of(&file)
}

#[cfg(unix)]
pub fn identity_at(path: &Path) -> Option<Identity> {
    use std::os::unix::fs::MetadataExt;
    let data = std::fs::symlink_metadata(path).ok()?;
    Some((data.dev(), data.ino()))
}

#[cfg(windows)]
pub fn identity_of(file: &File) -> Option<Identity> {
    let info = by_handle::information(file)?;
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Some((u64::from(info.dwVolumeSerialNumber), index))
}

#[cfg(unix)]
pub fn identity_of(file: &File) -> Option<Identity> {
    use std::os::unix::fs::MetadataExt;
    let data = file.metadata().ok()?;
    Some((data.dev(), data.ino()))
}

#[cfg(windows)]
pub fn link_count(file: &File) -> Option<u64> {
    by_handle::information(file).map(|info| u64::from(info.nNumberOfLinks))
}

#[cfg(unix)]
pub fn link_count(file: &File) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    file.metadata().ok().map(|data| data.nlink())
}

#[cfg(windows)]
pub fn is_encrypted(file: &File) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_ENCRYPTED;
    file.metadata()
        .is_ok_and(|data| data.file_attributes() & FILE_ATTRIBUTE_ENCRYPTED != 0)
}

#[cfg(windows)]
pub fn ends_in_space_or_dot(name: &std::ffi::OsStr) -> bool {
    use std::os::windows::ffi::OsStrExt;
    matches!(name.encode_wide().last(), Some(0x20 | 0x2E))
}

#[cfg(windows)]
pub fn units_of(character: char) -> usize {
    character.len_utf16()
}

#[cfg(not(windows))]
pub fn units_of(character: char) -> usize {
    character.len_utf8()
}

#[cfg(windows)]
pub fn carry_mark_of_the_web(from: &Path, to: &Path) {
    if let Some(mark) = mark_of_the_web::read(from) {
        let _ = std::fs::write(mark_of_the_web::stream_of(to), mark);
    }
}

#[cfg(not(windows))]
pub fn carry_mark_of_the_web(_from: &Path, _to: &Path) {}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub fn carry_extended_attributes(source: &File, duplicate: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let (from, to) = (source.as_raw_fd(), duplicate.as_raw_fd());
    let state = std::ptr::null_mut();
    // SAFETY: both descriptors stay open while the files are borrowed, and copyfile(3) accepts a null state
    let copied = unsafe { libc::fcopyfile(from, to, state, libc::COPYFILE_XATTR) };
    if copied == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    // WHY: a share that keeps no extended attributes cannot hold them anyway, as FAT cannot hold Windows streams
    if matches!(error.raw_os_error(), Some(libc::ENOTSUP | libc::EOPNOTSUPP)) {
        return Ok(());
    }
    Err(error)
}

#[cfg(not(target_os = "macos"))]
pub fn carry_extended_attributes(_source: &File, _duplicate: &File) -> io::Result<()> {
    Ok(())
}

#[cfg(windows)]
mod mark_of_the_web {
    use std::io::Read;
    use std::path::{Path, PathBuf};

    const MAX_MARK: u64 = 64 * 1024;
    const INTERNET_MARK: &[u8] = b"[ZoneTransfer]\r\nZoneId=3\r\n";

    pub fn stream_of(path: &Path) -> PathBuf {
        let mut stream = path.as_os_str().to_os_string();
        stream.push(":Zone.Identifier");
        PathBuf::from(stream)
    }

    // WHY: a mark too big or unreadable is still a mark, so the copy keeps the plain Internet one
    pub fn read(path: &Path) -> Option<Vec<u8>> {
        let stream = match std::fs::File::open(stream_of(path)) {
            Ok(stream) => stream,
            Err(error) if has_no_stream(&error) => return None,
            Err(_) => return Some(INTERNET_MARK.to_vec()),
        };
        let mut mark = Vec::new();
        let read = stream.take(MAX_MARK + 1).read_to_end(&mut mark);
        let fits = u64::try_from(mark.len()).is_ok_and(|length| length <= MAX_MARK);
        if read.is_err() || !fits {
            return Some(INTERNET_MARK.to_vec());
        }
        Some(mark)
    }

    // WHY: FAT has no streams at all and answers with an invalid name, which is no mark rather than a hidden one
    fn has_no_stream(error: &std::io::Error) -> bool {
        use windows_sys::Win32::Foundation::ERROR_INVALID_NAME;
        error.kind() == std::io::ErrorKind::NotFound
            || super::code_of(error) == Some(ERROR_INVALID_NAME)
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod by_handle {
    use std::fs::File;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    pub fn information(file: &File) -> Option<BY_HANDLE_FILE_INFORMATION> {
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the handle stays open while `file` is borrowed, and `info` is a writable struct of the expected type
        let found = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
        (found != 0).then_some(info)
    }
}
