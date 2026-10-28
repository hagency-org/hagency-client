//! Cooperative installation-wide resource writer exclusion.
//! This does not discover old ledgers, stop downlevel writers or authorize import.
//! NativeManagement uses these leases for its three resource-definition writes.
//! Normal domain/personal connections separately retain platform lifetime guards
//! on supported targets; finite authorization remains additive. Irreversible refusal
//! publication is feature-gated; runtime-qualified adoption is limited to macOS.
use crate::Error;
#[cfg(test)]
use crate::private;
use hagency_platform::legacy_writer::{ExclusiveWriterGuard, LegacyWriterGuard};
#[cfg(any(test, feature = "test-support"))]
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
#[cfg(test)]
const LOCK: &str = "resource-authority.lock";
#[cfg(test)]
const REFUSAL: &str = "legacy-resource-refusal.json";
const MAX_LEASE: Duration = Duration::from_secs(300);
impl From<hagency_platform::legacy_writer::Error> for Error {
    fn from(error: hagency_platform::legacy_writer::Error) -> Self {
        use hagency_platform::legacy_writer::Error as E;
        match error {
            E::Io(e) => Self::Io(e),
            E::Private => Self::Private,
            E::Refused => Self::LocalAuthority,
            E::Quarantined => Self::Quarantined,
            E::Locked => Self::Locked,
            E::Unavailable => Self::PlatformUnavailable,
            E::TestRoot => Self::LocalAuthority,
        }
    }
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn deadline(expires: Instant) -> Result<(), Error> {
    let now = Instant::now();
    if expires <= now || expires > now + MAX_LEASE {
        Err(Error::LocalAuthority)
    } else {
        Ok(())
    }
}

struct LegacyInner {
    _files: LegacyWriterGuard,
    live: AtomicBool,
    expires: Instant,
    gate: Arc<dyn Fn() -> bool + Send + Sync>,
}

/// Retain through every queued resource effect, including its final commit.
/// Retirement prevents effects but releases no lock until all copies are gone.
#[derive(Clone)]
pub struct LegacyResourceLease(Arc<LegacyInner>);
impl LegacyResourceLease {
    /// Blocking native acquisition, using the OS account rather than app paths.
    /// The additional gate must be synchronous and must perform no I/O.
    pub fn acquire(
        expires: Instant,
        gate: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<Self, Error> {
        deadline(expires)?;
        let gate: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(gate);
        if !gate() {
            return Err(Error::LocalAuthority);
        }
        Self::from_guard(LegacyWriterGuard::acquire()?, expires, gate)
    }
    #[cfg(any(test, feature = "test-support"))]
    fn at(
        root: &Path,
        expires: Instant,
        gate: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Self, Error> {
        deadline(expires)?;
        if !gate() {
            return Err(Error::LocalAuthority);
        }
        Self::from_guard(LegacyWriterGuard::isolated_for_tests(root)?, expires, gate)
    }
    fn from_guard(
        files: LegacyWriterGuard,
        expires: Instant,
        gate: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Self, Error> {
        let lease = Self(Arc::new(LegacyInner {
            _files: files,
            live: AtomicBool::new(true),
            expires,
            gate,
        }));
        lease.check()?;
        Ok(lease)
    }
    /// Synchronous writer fence; the retained OS lock excludes cooperating
    /// adoption writers without doing filesystem I/O inside a transaction.
    pub fn check(&self) -> Result<(), Error> {
        if !self.0.live.load(Ordering::Acquire)
            || Instant::now() >= self.0.expires
            || !(self.0.gate)()
        {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
    pub fn retire(&self) {
        self.0.live.store(false, Ordering::Release);
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn isolated_for_tests(
        root: &Path,
        expires: Instant,
        gate: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<Self, Error> {
        Self::at(root, expires, Arc::new(gate))
    }
}

/// A finite, non-cloneable exclusion handle, not proof of successful migration.
/// Acquire only after selected writers drain; keep it through source inspection
/// and eventual migration. The disabled adoption feature can publish only a refusal
/// marker; that marker never grants capacity or confirms an import.
pub struct AdoptionExclusiveLease {
    files: ExclusiveWriterGuard,
    registry_id: String,
    expires: Instant,
    gate: Arc<dyn Fn() -> bool + Send + Sync>,
}
impl AdoptionExclusiveLease {
    #[cfg(feature = "resource-adoption")]
    pub fn publish_refusal(&self, migration_id: &str, inventory_digest: &str) -> Result<(), Error> {
        self.check()?;
        self.files
            .publish_refusal(&self.registry_id, migration_id, inventory_digest)?;
        self.check().map_err(|_| Error::OutcomeUnknown)
    }
    pub fn acquire(
        registry_id: String,
        expires: Instant,
        gate: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<Self, Error> {
        deadline(expires)?;
        let gate: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(gate);
        if !digest(&registry_id) || !gate() {
            return Err(Error::LocalAuthority);
        }
        Self::from_guard(ExclusiveWriterGuard::acquire()?, registry_id, expires, gate)
    }
    #[cfg(any(test, feature = "test-support"))]
    fn at(
        root: &Path,
        registry_id: String,
        expires: Instant,
        gate: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Self, Error> {
        deadline(expires)?;
        if !digest(&registry_id) || !gate() {
            return Err(Error::LocalAuthority);
        }
        Self::from_guard(
            ExclusiveWriterGuard::isolated_for_tests(root)?,
            registry_id,
            expires,
            gate,
        )
    }
    fn from_guard(
        files: ExclusiveWriterGuard,
        registry_id: String,
        expires: Instant,
        gate: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Self, Error> {
        if files
            .refusal_registry()
            .is_some_and(|original| original != registry_id)
        {
            return Err(Error::Conflict);
        }
        let lease = Self {
            files,
            registry_id,
            expires,
            gate,
        };
        lease.check()?;
        Ok(lease)
    }
    pub fn registry_id(&self) -> &str {
        &self.registry_id
    }
    /// Called at each native phase boundary; includes retained-file identity.
    pub fn check(&self) -> Result<(), Error> {
        if Instant::now() >= self.expires || !(self.gate)() {
            return Err(Error::LocalAuthority);
        }
        self.files.check().map_err(Into::into)
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn isolated_for_tests(
        root: &Path,
        registry_id: String,
        expires: Instant,
        gate: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<Self, Error> {
        Self::at(root, registry_id, expires, Arc::new(gate))
    }
}

#[cfg(test)]
mod tests;
