//! Live provider reports do not rewrite historical execution bindings.
use super::*;
use crate::console::owner_provider::identity::ObservedQuota;
impl OwnerTelemetry {
    pub(in crate::console::native) fn observe_codex_quota(
        &self, resource_id:&str, observation:ObservedQuota,
    )->Result<String,TelemetryError> {
        let mut inner=self.lock()?;
        if inner.current_accounts.len()>=4096 && !inner.current_accounts.contains_key(resource_id) {
            return Err(TelemetryError::Unavailable);
        }
        let registration=codex_account(observation.subject_digest.as_deref(),resource_id,Some(&observation.plan));
        let id=inner.account(registration)?;
        let value=serde_json::to_value(observation.limits).map_err(|_|TelemetryError::Unavailable)?;
        inner.quota(&id,CODEX_READ,value,now_ms());
        inner.current_accounts.insert(resource_id.to_owned(),id.clone());
        Ok(id)
    }
    pub(in crate::console::native) fn forget_current_account(&self,resource_id:&str) {
        if let Ok(mut inner)=self.lock() {inner.current_accounts.remove(resource_id);}
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn observation(subject:Option<&str>)->ObservedQuota {
        ObservedQuota {subject_digest:subject.map(str::to_owned),plan:"pro".into(),limits:serde_json::from_value(json!({"rateLimits":{"primary":{"usedPercent":42,"windowDurationMins":300,"resetsAt":1800000000}}})).unwrap()}
    }
    #[test]
    fn idle_and_running_resources_share_exact_account_identity_without_rewriting_history() {
        let telemetry=OwnerTelemetry::new(1);
        let digest=hagency_runtime::codex::session::quota_account_digest("acct_original").unwrap();
        // Running turn's registration uses the execution driver's digest.
        let active=telemetry.register_account(codex_account(Some(&digest),"terra",Some("pro"))).unwrap();
        let idle=telemetry.observe_codex_quota("astra",observation(Some(&digest))).unwrap();
        assert_eq!(active,idle);
        let binding=ExecutionBinding {execution_id:"original".into(),resource_id:"astra".into(),account_id:Some(idle.clone()),allocation_id:None,agent_id:None,requester_id:None,room_id:None,runtime:Runtime::Codex,runtime_version:None,model:None,parent_execution_id:None,started_at_ms:1};
        telemetry.bind_execution(binding).unwrap();
        let replacement="b".repeat(64);
        let new=telemetry.observe_codex_quota("astra",observation(Some(&replacement))).unwrap();
        assert_ne!(new,idle);
        let historical=telemetry.read(|collector|collector.ledger().bindings().next().unwrap().account_id.clone()).unwrap();
        assert_eq!(historical,Some(idle));
        telemetry.forget_current_account("astra");
        assert_eq!(telemetry.read(|c|c.ledger().execution_count()).unwrap(),1);
    }
    #[test]
    fn missing_subject_keeps_accounts_separate_and_does_not_invent_usage() {
        let telemetry=OwnerTelemetry::new(1);
        let first=telemetry.observe_codex_quota("terra",observation(None)).unwrap();
        let second=telemetry.observe_codex_quota("astra",observation(None)).unwrap();
        assert_ne!(first,second);
        let parts=telemetry.parts(now_ms(),SnapshotOptions::default()).unwrap();
        assert_eq!(parts.accounts.len(),2);
        assert_eq!(parts.resources.len(),2);
        assert!(parts.resources.iter().all(|r|r.executions==0 && r.counts.input.is_none()));
    }
    #[test]
    fn rental_usage_filters_the_bound_model_without_splitting_shared_plan_quota() {
        use hagency_metering::dashboard::ledger::{CountMode,CountScope,UsageSource};
        use hagency_metering::dashboard::period::TimeRange;
        let telemetry=OwnerTelemetry::new(0);
        let digest=hagency_runtime::codex::session::quota_account_digest("shared-account").unwrap();
        let account=telemetry.observe_codex_quota("terra",observation(Some(&digest))).unwrap();
        assert_eq!(telemetry.observe_codex_quota("astra",observation(Some(&digest))).unwrap(),account);
        for (resource,model,tokens) in [("terra","fixture-terra",17),("astra","fixture-astra",29)] {
            let binding=ExecutionBinding{execution_id:resource.into(),resource_id:resource.into(),account_id:Some(account.clone()),allocation_id:None,agent_id:None,requester_id:None,room_id:None,runtime:Runtime::Codex,runtime_version:None,model:Some(model.into()),parent_execution_id:None,started_at_ms:1};
            telemetry.bind_execution(binding).unwrap();
            let event=UsageEvent{execution_id:resource.into(),stream:"thread".into(),event_id:None,sequence:Some(1),observed_at_ms:2,measured_at_ms:None,mode:CountMode::Cumulative{epoch:"original".into()},scope:CountScope::Own,counts:TokenCounts{input:Some(tokens),output:Some(0),cache_read:Some(0),cache_write:Some(0)},reasoning_output:None,complete:true,source:UsageSource::CodexRuntime};
            telemetry.lock().unwrap().event(event,"original").unwrap();
        }
        for (model,expected) in [("fixture-terra",17),("fixture-astra",29)] {
            let mut query=UsageQuery::range(TimeRange::new(0,900_000).unwrap());
            query.model=Some(model.into());
            query.account_id=Some(account.clone());
            assert_eq!(telemetry.read(|c|c.ledger().aggregate(&query).counts.input).unwrap(),Some(expected));
        }
        assert_eq!(telemetry.read(|c|c.quotas().project(now_ms()).len()).unwrap(),1);
    }

}
