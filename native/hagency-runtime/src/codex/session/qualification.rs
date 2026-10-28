//! Closed metadata-only reads for qualifying an owned process before any thread.
use super::*;
#[derive(Clone, Copy)]
pub enum QualificationRead {
    Configuration,
    Account,
    QuotaAccount,
}
impl QualificationRead {
    pub(super) fn request(self) -> (&'static str, Value) {
        match self {
            Self::Configuration => ("config/read", json!({"includeLayers":false})),
            Self::Account => ("account/read", json!({"refreshToken":false})),
            Self::QuotaAccount => ("account/rateLimits/read", json!({})),
        }
    }
}
