use crate::{Error, Result, ShareOptions};

pub(crate) fn share(_: ShareOptions) -> Result<()> {
    Err(Error::Unsupported)
}

pub(crate) struct PreparedShare;
pub(crate) fn prepare(_: ShareOptions) -> Result<PreparedShare> {
    Err(Error::Unsupported)
}
pub(crate) fn present_guarded(_: PreparedShare, _: crate::PresentationGuard) -> Result<()> {
    Err(Error::Unsupported)
}
