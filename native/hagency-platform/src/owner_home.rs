//! OS account home, independent of application data-directory and HOME overrides.
#[cfg(not(windows))]
use std::{io, path::PathBuf};

#[cfg(unix)]
pub fn owner_home() -> io::Result<PathBuf> {
    use std::{
        ffi::CStr,
        os::unix::{ffi::OsStrExt, fs::MetadataExt},
    };
    let uid = rustix::process::geteuid().as_raw();
    let mut size = 16 * 1024;
    loop {
        let mut buffer = vec![0u8; size];
        let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        // getpwuid_r initializes entry and points its strings into buffer on
        // success. Both objects remain live until the directory is copied.
        let code = unsafe {
            libc::getpwuid_r(
                uid,
                entry.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if code == libc::ERANGE && size < 1024 * 1024 {
            size *= 2;
            continue;
        }
        if code != 0 {
            return Err(io::Error::from_raw_os_error(code));
        }
        if result.is_null() {
            return Err(io::Error::other("OS owner home unavailable"));
        }
        let entry = unsafe { entry.assume_init() };
        if entry.pw_uid != uid || entry.pw_dir.is_null() {
            return Err(io::Error::other("OS owner identity mismatch"));
        }
        let raw = unsafe { CStr::from_ptr(entry.pw_dir) }.to_bytes();
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(raw));
        if !path.is_absolute() {
            return Err(io::Error::other("OS owner home is not absolute"));
        }
        let path = path.canonicalize()?;
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.is_dir() || metadata.uid() != uid {
            return Err(io::Error::other("OS owner home mismatch"));
        }
        return Ok(path);
    }
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::owner_home;

#[cfg(not(any(unix, windows)))]
pub fn owner_home() -> io::Result<PathBuf> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "owner registry requires an OS-account home adapter",
    ))
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn owner_home_override_child() {
        let Some(expected) = std::env::var_os("HAGENCY_OWNER_HOME_EXPECTED") else {
            return;
        };
        assert_eq!(
            super::owner_home().unwrap(),
            std::path::PathBuf::from(expected)
        );
        assert_ne!(
            super::owner_home().unwrap(),
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
        );
    }

    #[test]
    fn owner_home_is_os_bound_across_application_data_and_home_overrides() {
        let home = super::owner_home().unwrap();
        let alternate = tempfile::tempdir().unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("owner_home::tests::owner_home_override_child")
            .arg("--exact")
            .arg("--nocapture")
            .env("HAGENCY_OWNER_HOME_EXPECTED", home)
            .env("HOME", alternate.path())
            .env("XDG_DATA_HOME", alternate.path())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
    }
}

/// Codex still selects account credentials and configuration by its explicit
/// CODEX_HOME. On macOS Security.framework additionally resolves the default
/// keychain from HOME, which must name the OS user rather than a managed account
/// namespace. This does not select a Codex account or change the working directory.
pub fn codex_process_home(namespace_home: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let _ = namespace_home;
        owner_home()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(namespace_home.to_owned())
    }
}

#[cfg(test)]
mod codex_home_tests {
    #[test]
    fn codex_home_keeps_platform_credential_store_discoverable() {
        let namespace = tempfile::tempdir().unwrap();
        let selected = super::codex_process_home(namespace.path()).unwrap();
        #[cfg(target_os = "macos")]
        {
            assert_eq!(selected, super::owner_home().unwrap());
            assert_ne!(selected, namespace.path());
        }
        #[cfg(not(target_os = "macos"))]
        assert_eq!(selected, namespace.path());
    }
}
