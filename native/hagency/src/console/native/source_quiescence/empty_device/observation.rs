//! Read-only native observation before a human confirms device setup. This
//! cannot create a canonical registry or authorize source shutdown by itself.
use super::*;

pub(in crate::console::native) struct EmptyDeviceObservation {
    binding: String,
    inventory: EmptyDeviceInventory,
}
impl EmptyDeviceObservation {
    pub(in crate::console::native) fn personal_present(&self) -> bool {
        self.inventory.personal_present()
    }
    pub(in crate::console::native) fn completed_private_records(&self) -> u32 {
        self.inventory.completed_private_records()
    }
}
impl NativeOwner {
    pub(in crate::console::native) async fn observe_empty_device(
        &self,
        profile: String,
    ) -> Result<EmptyDeviceObservation, NativeError> {
        self.0
            .host
            .check_state_lock()
            .map_err(|_| err("source_root_changed"))?;
        let root = self.0.host.state_directory().to_owned();
        let owner = self.0.expected_owner.clone();
        let (binding, inventory) = tokio::task::spawn_blocking(move || {
            files::Sources::capture(&root, &profile)?.empty_device_inventory(&profile, &owner)
        })
        .await
        .map_err(|_| err("source_quiescence_unknown"))?
        .map_err(err)?;
        self.0
            .host
            .check_state_lock()
            .map_err(|_| err("source_root_changed"))?;
        Ok(EmptyDeviceObservation { binding, inventory })
    }
}
impl EmptyDeviceReview {
    pub(in crate::console::native) fn matches_observation(
        &self,
        original: &EmptyDeviceObservation,
    ) -> Result<(), NativeError> {
        self.scope.gate.check().map_err(err)?;
        if self.binding != original.binding
            || self
                .inventory
                .digest()
                .map_err(super::super::super::management::store_error)?
                != original
                    .inventory
                    .digest()
                    .map_err(super::super::super::management::store_error)?
        {
            return Err(err("empty_device_review_changed"));
        }
        Ok(())
    }
}
