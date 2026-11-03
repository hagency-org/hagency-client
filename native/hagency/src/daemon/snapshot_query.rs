//! Read-only dashboard filters. Global resource/account totals remain global;
//! only the Usage destination reads this explicitly scoped report.
use super::dispatch::SnapshotParts;
use crate::native_owner::{OwnerTelemetry, TelemetryError};
use hagency_metering::dashboard::{UsageBreakdown, UsageFilter, collector::SnapshotOptions};

pub fn apply(telemetry: &OwnerTelemetry, parts: &mut SnapshotParts, now: u64,
    options: SnapshotOptions, filter: Option<UsageFilter>) -> Result<(), TelemetryError> {
    let filter = filter.unwrap_or_default();
    let all = telemetry.usage_report(now, options, None)?;
    let models = all.by_model.iter().filter_map(|(key, _)| key.clone()).collect();
    let requesters = all.by_requester.iter().filter_map(|(key, _)| key.clone()).collect();
    let report = if filter.is_empty() { all } else {
        telemetry.usage_report(now, options, Some(filter.query(all.range)))?
    };
    parts.usage_breakdown = Some(UsageBreakdown { filter, report, models, requesters });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hagency_metering::dashboard::{ledger::ExecutionBinding, period::UsagePeriod};
    use hagency_metering::dashboard::ledger::Runtime;

    #[test]
    fn dimensions_match_trusted_execution_bindings_and_preserve_global_totals() {
        let telemetry = OwnerTelemetry::new(0);
        for (id,model,requester,input) in [("a","model-a","@one:example.test",10),("b","model-b","@one:example.test",20),("c","model-a","@two:example.test",30)] {
            let binding=ExecutionBinding { execution_id:id.into(),resource_id:"personal-agent:one".into(),account_id:None,
                allocation_id:None,agent_id:Some("one".into()),requester_id:Some(requester.into()),room_id:None,
                runtime:Runtime::Codex,runtime_version:None,model:Some(model.into()),parent_execution_id:None,started_at_ms:0 };
            telemetry.personal_usage(&binding, &hagency_agent_local::Usage {
                input,output:2,cached_input:0,cache_write_input:Some(0),reasoning_output:0,accounting_version:"provider-v1".into(),
            }).unwrap();
        }
        let options=SnapshotOptions {period:UsagePeriod::All,..Default::default()};
        let mut parts=SnapshotParts::default();
        let filter=UsageFilter {model:Some("model-a".into()),requester_id:Some("@one:example.test".into()),..Default::default()};
        apply(&telemetry,&mut parts,u64::MAX/2,options,Some(filter.clone())).unwrap();
        let details=parts.usage_breakdown.unwrap();
        assert_eq!(details.filter,filter);
        assert_eq!(details.report.total.counts.input,Some(10));
        assert_eq!(details.report.total.executions,1);
        assert_eq!(details.models,vec!["model-a","model-b"]);
        assert_eq!(details.requesters.len(),2);
        assert!(parts.usage.is_none(),"Usage filters must not rewrite global overview totals");
        assert!(parts.resources.is_empty(),"Resource inventory must not be filtered by usage");
    }
}
