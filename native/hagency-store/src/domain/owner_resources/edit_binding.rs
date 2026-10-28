//! Native provider observation plus original namespace custody for a local edit.
use super::*;

/// Neither this proof nor its namespace can be constructed from serialized data.
pub struct ResourceEditBinding {
    binding: VerifiedOwnerResourceBinding,
    namespace: Option<ResourceNamespace>,
}
impl ResourceEditBinding {
    pub fn new(
        binding: VerifiedOwnerResourceBinding,
        namespace: Option<ResourceNamespace>,
    ) -> Result<Self, Error> {
        binding.check()?;
        if let Some(namespace) = &namespace {
            namespace.matches_edit_binding(&binding)?;
        }
        Ok(Self { binding, namespace })
    }
    pub(super) fn expires(&self) -> Instant {
        self.binding.expires
    }
    pub(super) fn check_on(
        &self,
        db: &Connection,
        registry: &str,
        profile: &ProfileAuthority,
        id: &str,
        revision: &str,
    ) -> Result<(), Error> {
        if self.binding.profile.digest != profile.digest
            || self.binding.profile.account != profile.account
            || self.binding.resource_id != id
            || self.binding.resource_revision != revision
        {
            return Err(Error::LocalAuthority);
        }
        self.binding.check_canonical(db, registry)?;
        match &self.namespace {
            Some(namespace) => namespace.check_edit_on(db, &self.binding)?,
            None => {
                let has_namespace: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM owner_resource_namespaces WHERE resource_id=?1)",
                    [id],
                    |r| r.get(0),
                )?;
                if has_namespace || read_resource(db, id)?.preset_id.starts_with("owner_ns_") {
                    return Err(Error::LocalAuthority);
                }
            }
        }
        Ok(())
    }
    // The immediate transaction already pins DB identity. After changing the
    // resource revision, check live authority and original filesystem custody.
    pub(super) fn check_live(&self) -> Result<(), Error> {
        self.binding.check()?;
        if let Some(namespace) = &self.namespace {
            namespace.matches_edit_binding(&self.binding)?;
        }
        Ok(())
    }
}
