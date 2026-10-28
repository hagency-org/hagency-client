//! Compatibility adapters for the shared OS-private state implementation.
use crate::Error;
use hagency_platform::private_state as shared;
use std::{fs::File, path::Path};

impl From<shared::Error> for Error {
    fn from(error: shared::Error) -> Self {
        match error {
            shared::Error::Io(error) => Self::Io(error),
            shared::Error::Private => Self::Private,
            shared::Error::PlatformUnavailable => Self::PlatformUnavailable,
        }
    }
}

pub fn directory(path: &Path) -> Result<(), Error> {
    shared::directory(path).map_err(Into::into)
}
pub fn create_directory_new(path: &Path) -> Result<(), Error> {
    shared::create_directory_new(path).map_err(Into::into)
}
pub fn check_handle(file: &File) -> Result<(), Error> {
    shared::check_handle(file).map_err(Into::into)
}
pub(crate) fn sync_directory(dir: &cap_std::fs::Dir) -> Result<(), Error> {
    shared::sync_directory(dir).map_err(Into::into)
}
pub fn seal_created_file_handle(file: &File) -> Result<(), Error> {
    shared::seal_created_file_handle(file).map_err(Into::into)
}
pub fn open(path: &Path, create: bool) -> Result<File, Error> {
    shared::open(path, create).map_err(Into::into)
}
pub fn open_journal(path: &Path) -> Result<File, Error> {
    shared::open_journal(path).map_err(Into::into)
}
pub fn write_new(path: &Path, value: &[u8]) -> Result<(), Error> {
    shared::write_new(path, value).map_err(Into::into)
}
pub fn replace(path: &Path, value: &[u8]) -> Result<(), Error> {
    shared::replace(path, value).map_err(Into::into)
}
pub fn read_secret(path: &Path) -> Result<Vec<u8>, Error> {
    shared::read_secret(path).map_err(Into::into)
}

#[cfg(windows)]
pub struct WindowsDirectorySync(shared::WindowsDirectorySync);
#[cfg(windows)]
impl WindowsDirectorySync {
    pub fn open(dir: &cap_std::fs::Dir) -> Result<Option<Self>, Error> {
        shared::WindowsDirectorySync::open(dir)
            .map(|value| value.map(Self))
            .map_err(Into::into)
    }
    pub fn sync(&mut self) -> Result<(), Error> {
        self.0.sync().map_err(Into::into)
    }
}
