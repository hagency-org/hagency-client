//! Bounded discovery of existing pending work; never opens a provider or lease.
use super::super::native::pending_approvals::Summary;
use super::*;

impl OwnedRuntime {
    pub(in crate::console) async fn pending_page(
        &self,
        device: &AuthorizedDevice,
        after: Option<&(String, String)>,
        limit: usize,
    ) -> Result<(Vec<Summary>, usize, bool), RuntimeError> {
        let entries = self.entries_for(device).await?;
        let pin = DevicePin::from(device);
        let mut selected = BTreeMap::new();
        let mut total = 0usize;
        for entry in entries.values() {
            if entry.device != pin || *entry.cancel.borrow() || entry.task.is_finished() {
                continue;
            }
            for slot in entry.approvals.values() {
                let proposal = &slot.view.proposal;
                if slot.device != pin
                    || proposal.scope.agent != entry.status.borrow().agent_id
                    || proposal.expires <= now()
                    || slot.pending.is_closed()
                    || *slot.turn_cancel.borrow()
                    || !entry.binding_active(&proposal.scope.binding)
                {
                    continue;
                }
                total = total.saturating_add(1);
                let key = (proposal.scope.agent.clone(), slot.view.proposal_id.clone());
                if after.is_some_and(|after| &key <= after) {
                    continue;
                }
                if selected.len() > limit
                    && selected
                        .last_key_value()
                        .is_some_and(|(last, _)| &key >= last)
                {
                    continue;
                }
                selected.insert(
                    key,
                    Summary {
                        agent_id: proposal.scope.agent.clone(),
                        binding_id: proposal.scope.binding.clone(),
                        room_id: proposal.scope.room.clone(),
                        requester: proposal.scope.requester.clone(),
                        proposal_id: slot.view.proposal_id.clone(),
                        args_digest: slot.view.args_digest.clone(),
                        tool: proposal.tool.clone(),
                        expires: proposal.expires,
                    },
                );
                if selected.len() > limit + 1 {
                    selected.pop_last();
                }
            }
        }
        device.bearer().map_err(|_| RuntimeError::Authorization)?;
        let more = selected.len() > limit;
        Ok((selected.into_values().take(limit).collect(), total, more))
    }
}
