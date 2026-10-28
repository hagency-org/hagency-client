//! OS-token-derived profile root. Never USERPROFILE, HOME or app configuration.
use std::{
    fs::OpenOptions,
    io,
    os::windows::{
        ffi::OsStringExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::PathBuf,
    ptr,
};
use windows_sys::Win32::{
    Foundation::ERROR_INSUFFICIENT_BUFFER,
    Security::TOKEN_QUERY,
    Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
    UI::Shell::GetUserProfileDirectoryW,
};

pub fn owner_home() -> io::Result<PathBuf> {
    let mut raw = ptr::null_mut();
    // SAFETY: Current process pseudo-handle; initialized owned token output.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: Successful OpenProcessToken transferred this owned handle.
    let token = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut size = 0;
    // SAFETY: Null-buffer sizing query has one initialized output scalar.
    let first =
        unsafe { GetUserProfileDirectoryW(token.as_raw_handle(), ptr::null_mut(), &mut size) };
    let error = io::Error::last_os_error();
    if first != 0
        || error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
        || !(2..=32768).contains(&size)
    {
        return Err(io::Error::other("OS owner profile unavailable"));
    }
    let mut buffer = vec![0u16; size as usize];
    // SAFETY: The token and bounded output buffer stay live until completion.
    if unsafe { GetUserProfileDirectoryW(token.as_raw_handle(), buffer.as_mut_ptr(), &mut size) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    if size < 2
        || size as usize > buffer.len()
        || buffer[size as usize - 1] != 0
        || buffer[..size as usize - 1].contains(&0)
    {
        return Err(io::Error::other("OS owner profile malformed"));
    }
    let path = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..size as usize - 1]));
    if !path.is_absolute()
        || std::fs::symlink_metadata(&path)?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(io::Error::other("OS owner profile mismatch"));
    }
    let retained = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&path)?;
    crate::private_state::check_os_profile_owner(&retained)
        .map_err(|_| io::Error::other("OS owner profile mismatch"))?;
    let canonical = path.canonicalize()?;
    let current = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&canonical)?;
    crate::private_state::check_os_profile_owner(&current)
        .map_err(|_| io::Error::other("OS owner profile mismatch"))?;
    if !crate::same_directory(&retained, &current)? {
        return Err(io::Error::other("OS owner profile changed"));
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    #[test]
    fn os_profile_ignores_environment_child() {
        let Some(expected) = std::env::var_os("HAGENCY_TEST_OS_PROFILE") else {
            return;
        };
        assert_eq!(
            super::owner_home().unwrap(),
            std::path::PathBuf::from(expected)
        );
        assert_ne!(
            super::owner_home().unwrap(),
            std::path::PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
        );
    }
    #[test]
    fn os_profile_uses_current_token_and_ignores_app_overrides() {
        let actual = super::owner_home().unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("owner_home::windows::tests::os_profile_ignores_environment_child")
            .arg("--exact")
            .env("HAGENCY_TEST_OS_PROFILE", actual)
            .env("USERPROFILE", temporary.path())
            .env("HOME", temporary.path())
            .env("APPDATA", temporary.path())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
    }
}
