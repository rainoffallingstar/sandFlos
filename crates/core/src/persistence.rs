//! Atomic replacement shared by CLI configuration and credential persistence.
use std::{io, path::Path};

#[cfg(not(windows))]
pub(crate) fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
pub(crate) fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(source: *const u16, destination: *const u16, flags: u32) -> i32;
    }
    fn wide(path: &Path) -> io::Result<Vec<u16>> {
        let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
        if value.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path contains NUL",
            ));
        }
        value.push(0);
        Ok(value)
    }
    let source = wide(source)?;
    let destination = wide(destination)?;
    // Both buffers are NUL terminated and live through the call. Replacement
    // stays on the same volume; failure does not first delete the old file.
    let succeeded = unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 1 | 8) };
    if succeeded == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
