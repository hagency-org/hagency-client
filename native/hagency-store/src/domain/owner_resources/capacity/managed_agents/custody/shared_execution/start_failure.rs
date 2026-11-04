//! Finish the exact admitted call even when authority expires before capture.
//! Only the original retained runtime can prove that dispatch never began.
use super::*;

impl SharedCallCustody {
    pub async fn finish_failed_start(self) -> Result<SharedWorkReceipt, Error> {
        // The runtime seals under the same mutex used by begin_turn. Either
        // dispatch won (usage remains unknown), or no future dispatch can occur.
        let not_dispatched = self.sink.source().seal_unstarted_turn();
        let process_digest = self.process_digest()?;
        let scope = self.call.scope;
        let work = self.work;
        let registry = scope.registry.clone();
        registry.store.call(64 * 1024, move |db| {
            let tx = db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // Cleanup checks immutable custody, never expired login/activity.
            observations::check_scope(&tx, &scope, "usage")?;
            let id = format!("swo_{}", canonical::digest(&json!({
                "workId":work.id,"callId":scope.target,"startFailed":true,
                "notDispatched":not_dispatched
            }))?);
            let recorded = if not_dispatched {
                observations::not_dispatched(&tx, &scope)?
            } else {
                observations::usage(&tx, &scope.child, &scope.target,
                    ManagedUsageObservation::shared_capture(format!("{id}_usage"), None, true, false, false))?
            };
            let h = calls::history(&tx, &scope.child, &scope.target)?;
            let unrepresentable:bool=tx.query_row("SELECT unrepresentable_usage FROM owner_managed_calls WHERE id=?1",[&scope.target],|r|r.get(0))?;
            let receipt = SharedWorkReceipt {
                version: 1, observation_id: id.clone(), work_id: work.id.clone(),
                work_digest: canonical::digest(&serde_json::to_value(&work)?)?,
                execution_id: work.execution_id.clone(), call_id: scope.target,
                runtime_command_digest: work.runtime_command_digest.clone(), process_digest: process_digest.clone(),
                ledger_revision: recorded.ledger_revision,
                state: if not_dispatched {"failed"} else {"unknown"}.into(),
                known_consumed_tokens: h.known_consumed_tokens,
                retained_tokens: (!unrepresentable).then_some(if h.usage_complete {h.known_consumed_tokens} else {h.reserved_tokens.max(h.known_consumed_tokens)}),
                usage_complete: h.usage_complete, body: None, body_digest: None,
            };
            receipt.validate_for(&work, &process_digest)?;
            bounded_row(&tx,"owner_shared_work_observations","id",&id,1_000_000)?;
            tx.execute("INSERT INTO owner_shared_work_observations VALUES(?1,?2,?3,?4,?5)",
                params![id,work.id,canonical::encode(&serde_json::to_value(&receipt)?)?,canonical::digest(&serde_json::to_value(&receipt)?)?,receipt.ledger_revision])?;
            tx.commit()?;
            Ok(receipt)
        }).await
    }
}
