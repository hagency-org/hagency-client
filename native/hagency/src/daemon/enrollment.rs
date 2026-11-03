//! Owner enrollment and the Desktop → daemon handover journal.
//!
//! The journal names exactly one owner (server origin and Matrix ID) and the
//! owner root the daemon hosts. It carries no credentials. Handover is
//! explicit and resumable:
//!
//! 1. `stage_handover` records Desktop's existing owner root as `staged`. The
//!    daemon does not host it yet.
//! 2. Desktop drains and fences its executor and releases the root. Desktop
//!    owns that step; the root's exclusive lock enforces it either way.
//! 3. The daemon opens the root (taking the lock), authorizes, starts workers
//!    and only then records `active`.
//! 4. `release` (rollback) records `released`. The daemon stops opening the
//!    root, and Desktop may take it back once the daemon has drained.
//!
//! An interrupted step resumes from the saved state. A `staged` root that is
//! still locked by its source is reported as waiting; it is never forced.
use super::control::now_ms;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const LIMIT: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoverSource {
    Desktop,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Handover {
    /// The daemon created this owner root at enrollment.
    Fresh,
    /// An existing root is named but not yet verified as hosted here.
    #[serde(rename_all = "camelCase")]
    Staged {
        source: HandoverSource,
        staged_at_ms: u64,
    },
    /// The daemon opened, authorized and published hosting for the root.
    #[serde(rename_all = "camelCase")]
    Active {
        source: Option<HandoverSource>,
        since_ms: u64,
    },
    /// Rolled back: the daemon must not open this root.
    #[serde(rename_all = "camelCase")]
    Released { at_ms: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Enrollment {
    pub version: u8,
    pub origin: String,
    pub owner: String,
    #[serde(default)]
    pub display_name: Option<String>,
    pub owner_root: PathBuf,
    pub handover: Handover,
}

#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
pub enum EnrollmentError {
    #[error("the enrollment journal is unavailable")]
    Storage,
    #[error("a different owner is already enrolled")]
    OtherOwner,
    #[error("the owner root must be an absolute path outside the daemon home")]
    Root,
    #[error("this handover transition is not allowed from the current state")]
    Transition,
    #[error("no owner is enrolled")]
    Missing,
}

impl EnrollmentError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Storage => "daemon_enrollment_storage_unavailable",
            Self::OtherOwner => "daemon_other_owner_enrolled",
            Self::Root => "daemon_owner_root_invalid",
            Self::Transition => "daemon_handover_transition_refused",
            Self::Missing => "daemon_enrollment_required",
        }
    }
}

fn path(home: &Path) -> PathBuf {
    home.join(super::ENROLLMENT)
}

/// Fresh roots live under the daemon home, keyed by server and owner.
pub fn fresh_root(home: &Path, origin: &str, owner: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let key = serde_json::to_vec(&(origin, owner)).unwrap_or_default();
    home.join("owners").join(format!("{:x}", Sha256::digest(key)))
}

impl Enrollment {
    pub fn load(home: &Path) -> Result<Option<Self>, EnrollmentError> {
        let Some(raw) = super::read_private(&path(home), LIMIT).map_err(|_| EnrollmentError::Storage)?
        else {
            return Ok(None);
        };
        let value: Self = serde_json::from_slice(&raw).map_err(|_| EnrollmentError::Storage)?;
        if value.version != 1
            || !value.owner_root.is_absolute()
            || !super::protocol::valid_owner_id(&value.owner)
        {
            return Err(EnrollmentError::Storage);
        }
        Ok(Some(value))
    }

    fn save(&self, home: &Path) -> Result<(), EnrollmentError> {
        let raw = serde_json::to_vec(self).map_err(|_| EnrollmentError::Storage)?;
        hagency_store::private::replace(&path(home), &raw).map_err(|_| EnrollmentError::Storage)
    }

    /// Whether the daemon may open the owner root in this state.
    pub fn hostable(&self) -> bool {
        !matches!(self.handover, Handover::Released { .. })
    }

    fn check_root(home: &Path, root: &Path) -> Result<(), EnrollmentError> {
        if !root.is_absolute() {
            return Err(EnrollmentError::Root);
        }
        // The owner root is a sibling custody domain; only `owners/` under
        // the daemon home is allowed, never the home itself or its files.
        if root.starts_with(home) && !root.starts_with(home.join("owners")) {
            return Err(EnrollmentError::Root);
        }
        Ok(())
    }

    /// Record a fresh enrollment after the daemon's own SDK sign-in.
    /// Idempotent for the same owner and root.
    pub fn enroll_fresh(
        home: &Path,
        origin: &str,
        owner: &str,
        display_name: Option<String>,
    ) -> Result<Self, EnrollmentError> {
        let root = fresh_root(home, origin, owner);
        Self::create(home, origin, owner, display_name, root, Handover::Fresh)
    }

    /// Name Desktop's existing owner root for adoption. The daemon hosts it
    /// only after the source released the root and readiness is verified.
    pub fn stage_handover(
        home: &Path,
        origin: &str,
        owner: &str,
        display_name: Option<String>,
        desktop_root: &Path,
    ) -> Result<Self, EnrollmentError> {
        Self::create(
            home,
            origin,
            owner,
            display_name,
            desktop_root.into(),
            Handover::Staged {
                source: HandoverSource::Desktop,
                staged_at_ms: now_ms(),
            },
        )
    }

    fn create(
        home: &Path,
        origin: &str,
        owner: &str,
        display_name: Option<String>,
        root: PathBuf,
        handover: Handover,
    ) -> Result<Self, EnrollmentError> {
        if !super::protocol::valid_owner_id(owner) {
            return Err(EnrollmentError::OtherOwner);
        }
        Self::check_root(home, &root)?;
        if let Some(existing) = Self::load(home)? {
            return if existing.origin == origin && existing.owner == owner && existing.owner_root == root {
                Ok(existing)
            } else {
                Err(EnrollmentError::OtherOwner)
            };
        }
        let value = Self {
            version: 1,
            origin: origin.into(),
            owner: owner.into(),
            display_name,
            owner_root: root,
            handover,
        };
        value.save(home)?;
        Ok(value)
    }

    /// Called by the runtime only after the owner root is open, the SDK
    /// authorized it and the workers started.
    pub fn mark_active(home: &Path) -> Result<Self, EnrollmentError> {
        let mut value = Self::load(home)?.ok_or(EnrollmentError::Missing)?;
        value.handover = match value.handover {
            Handover::Active { .. } => return Ok(value),
            Handover::Fresh => Handover::Active {
                source: None,
                since_ms: now_ms(),
            },
            Handover::Staged { source, .. } => Handover::Active {
                source: Some(source),
                since_ms: now_ms(),
            },
            Handover::Released { .. } => return Err(EnrollmentError::Transition),
        };
        value.save(home)?;
        Ok(value)
    }

    /// Roll back: the daemon stops opening the root. The caller must have
    /// stopped hosting (drained and released the root lock) first.
    pub fn release(home: &Path) -> Result<Self, EnrollmentError> {
        let mut value = Self::load(home)?.ok_or(EnrollmentError::Missing)?;
        if !matches!(value.handover, Handover::Released { .. }) {
            value.handover = Handover::Released { at_ms: now_ms() };
            value.save(home)?;
        }
        Ok(value)
    }

    /// Re-stage a released root, for example to retry a rolled-back handover.
    pub fn restage(home: &Path) -> Result<Self, EnrollmentError> {
        let mut value = Self::load(home)?.ok_or(EnrollmentError::Missing)?;
        if let Handover::Released { .. } = value.handover {
            value.handover = if value.owner_root == fresh_root(home, &value.origin, &value.owner) {
                Handover::Fresh
            } else { Handover::Staged {
                source: HandoverSource::Desktop,
                staged_at_ms: now_ms(),
            }};
            value.save(home)?;
            Ok(value)
        } else {
            Err(EnrollmentError::Transition)
        }
    }

    /// Remove the enrollment after explicit owner sign-out. The owner root,
    /// receipts and registry remain on disk for recovery or another host.
    pub fn withdraw(home: &Path) -> Result<(), EnrollmentError> {
        match std::fs::remove_file(path(home)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(EnrollmentError::Storage),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        temp
    }

    #[test]
    fn handover_is_staged_then_active_only_by_explicit_steps_and_survives_reload() {
        let temp = home();
        let desktop = temp.path().parent().unwrap().join("desktop-native-owner-root");
        let staged = Enrollment::stage_handover(temp.path(), "https://s.example/", "@o:s.example", None, &desktop).unwrap();
        assert!(matches!(staged.handover, Handover::Staged { source: HandoverSource::Desktop, .. }));
        assert!(staged.hostable());
        // Same request again is idempotent; another owner or root is refused.
        assert_eq!(
            Enrollment::stage_handover(temp.path(), "https://s.example/", "@o:s.example", None, &desktop).unwrap(),
            staged
        );
        assert_eq!(
            Enrollment::stage_handover(temp.path(), "https://s.example/", "@p:s.example", None, &desktop).unwrap_err(),
            EnrollmentError::OtherOwner
        );
        assert_eq!(
            Enrollment::enroll_fresh(temp.path(), "https://s.example/", "@o:s.example", None).unwrap_err(),
            EnrollmentError::OtherOwner
        );
        let active = Enrollment::mark_active(temp.path()).unwrap();
        assert!(matches!(active.handover, Handover::Active { source: Some(HandoverSource::Desktop), .. }));
        assert_eq!(Enrollment::load(temp.path()).unwrap().unwrap(), active);
        // Rollback stops hosting and cannot be silently reactivated.
        let released = Enrollment::release(temp.path()).unwrap();
        assert!(!released.hostable());
        assert_eq!(Enrollment::mark_active(temp.path()).unwrap_err(), EnrollmentError::Transition);
        let restaged = Enrollment::restage(temp.path()).unwrap();
        assert!(matches!(restaged.handover, Handover::Staged { .. }));
        Enrollment::withdraw(temp.path()).unwrap();
        assert!(Enrollment::load(temp.path()).unwrap().is_none());
    }

    #[test]
    fn fresh_roots_are_keyed_and_inside_owners_only() {
        let temp = home();
        let fresh = Enrollment::enroll_fresh(temp.path(), "https://s.example/", "@o:s.example", Some("O".into())).unwrap();
        assert!(fresh.owner_root.starts_with(temp.path().join("owners")));
        assert_ne!(
            fresh_root(temp.path(), "https://s.example/", "@o:s.example"),
            fresh_root(temp.path(), "https://t.example/", "@o:s.example")
        );
        Enrollment::withdraw(temp.path()).unwrap();
        for bad in [temp.path().to_path_buf(), temp.path().join("matrix"), PathBuf::from("relative")] {
            assert_eq!(
                Enrollment::stage_handover(temp.path(), "https://s.example/", "@o:s.example", None, &bad).unwrap_err(),
                EnrollmentError::Root,
                "{bad:?}"
            );
        }
        hagency_store::private::replace(&temp.path().join(super::super::ENROLLMENT), b"{\"version\":9}").unwrap();
        assert_eq!(Enrollment::load(temp.path()).unwrap_err(), EnrollmentError::Storage);
    }
}
