//! Version-three native descriptor for a provider-free version-four store plan.
//! It uses the same create-once slot as existing migration, not a second root.
use super::*;
use hagency_store::owner_resources::adoption::{PreparedEmptyDeviceInitialization, EmptyDeviceTransfer};

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::console::native) struct EmptyDevicePrepare {
    pub source_binding: String,
    pub plan_digest: String,
    pub inventory_digest: String,
    pub personal_database_present: bool,
    pub completed_private_records: u32,
}
impl EmptyDevicePrepare {
    pub(in crate::console::native) fn matches_transfer(
        &self,
        transfer: &EmptyDeviceTransfer,
    ) -> Result<(), NativeError> {
        if self.source_binding != transfer.source_binding()
            || self.plan_digest
                != transfer
                    .plan_digest()
                    .map_err(super::super::management::store_error)?
            || self.inventory_digest != transfer.inventory_digest()
            || self.personal_database_present != transfer.personal_present()
            || self.completed_private_records != transfer.completed_private_records()
        {
            return Err(failure(409, "empty_device_review_changed"));
        }
        Ok(())
    }
    pub(in crate::console::native) fn from_prepared(
        binding: String,
        prepared: &PreparedEmptyDeviceInitialization,
    ) -> Result<Self, NativeError> {
        let summary = prepared.review_summary();
        let value = Self {
            source_binding: binding,
            plan_digest: prepared
                .plan_digest()
                .map_err(super::super::management::store_error)?,
            inventory_digest: prepared.inventory_digest().into(),
            personal_database_present: summary.personal_database_present,
            completed_private_records: summary.completed_private_records,
        };
        value.validate()?;
        Ok(value)
    }
    pub(super) fn validate(&self) -> Result<(), NativeError> {
        if [
            &self.source_binding,
            &self.plan_digest,
            &self.inventory_digest,
        ]
        .into_iter()
        .any(|s| {
            s.len() != 64
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }) {
            return Err(descriptor_error());
        }
        Ok(())
    }
}
impl AdoptedManagementSelection {
    pub(in crate::console::native) fn empty_preparation(&self) -> Option<&EmptyDevicePrepare> {
        self.empty_prepare.as_ref()
    }
    // The caller must separately prove the canonical plan is active before
    // opening management. Pending preparation still requires the original
    // device through saved_empty_prepare below. Old descriptors remain unchanged.
    pub(in crate::console::native) fn saved_empty_prepare_for_active_open(
        owner: &NativeOwner, registry: OwnerResourceRegistry, initialization: &str, profile: &str,
    ) -> Result<Option<EmptyDevicePrepare>, NativeError> {
        let Some(descriptor) = read_descriptor(owner.0.host.state_directory())? else { return Ok(None); };
        if descriptor.empty_prepare.is_none() { return Ok(None); }
        if descriptor.migration_id != initialization || descriptor.profile != profile { return Err(descriptor_error()); }
        let selected = Self::loaded(owner.0.host.state_directory(), &owner.0.expected_origin, &owner.0.expected_owner, registry, descriptor)?;
        Ok(selected.empty_prepare)
    }

    pub(in crate::console::native) fn saved_empty_prepare(
        owner: &NativeOwner,
        registry: OwnerResourceRegistry,
        initialization: &str,
        profile: &str,
        device: &str,
    ) -> Result<Option<EmptyDevicePrepare>, NativeError> {
        let Some(descriptor) = read_descriptor(owner.0.host.state_directory())? else {
            return Ok(None);
        };
        if descriptor.empty_prepare.is_none() {
            return Ok(None);
        }
        if descriptor.migration_id != initialization || descriptor.profile != profile {
            return Err(descriptor_error());
        }
        let selected = Self::loaded(
            owner.0.host.state_directory(),
            &owner.0.expected_origin,
            &owner.0.expected_owner,
            registry,
            descriptor,
        )?;
        selected.original_empty_prepare(device).cloned().map(Some)
    }
    pub(in crate::console::native) fn prepared_empty_device(
        owner: &NativeOwner,
        registry: OwnerResourceRegistry,
        initialization: String,
        profile: String,
        device: String,
        prepare: EmptyDevicePrepare,
    ) -> Result<Self, NativeError> {
        prepare.validate()?;
        let mut selected = Self::completed(owner, registry, initialization, profile, device);
        selected.empty_prepare = Some(prepare);
        Ok(selected)
    }
    pub(in crate::console::native) fn original_empty_prepare(
        &self,
        device: &str,
    ) -> Result<&EmptyDevicePrepare, NativeError> {
        if self.device != device {
            return Err(descriptor_error());
        }
        self.empty_prepare.as_ref().ok_or_else(descriptor_error)
    }
}

#[cfg(test)]
mod tests;
