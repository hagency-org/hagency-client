use super::*;

fn add(sum: &mut u64, n: u64) -> Result<(), NativeError> {
    *sum = sum
        .checked_add(n)
        .filter(|n| *n <= hagency_core::JSON_SAFE_MAX)
        .ok_or_else(invalid)?;
    Ok(())
}
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Totals {
    resource_count: usize,
    measured_resource_count: usize,
    known_fresh_tokens: u64,
    known_total_tokens: u64,
    reserved_tokens: u64,
    drawn_tokens: u64,
    declared_ceiling_tokens: u64,
    undeclared_resource_count: usize,
}
fn accumulate(
    totals: &mut std::collections::BTreeMap<String, Totals>,
    resource: &service::ResourceSummary,
) -> Result<(), NativeError> {
    let period = serde_json::to_value(resource.usage.period)
        .map_err(|_| invalid())?
        .as_str()
        .ok_or_else(invalid)?
        .to_owned();
    let item = totals.entry(period).or_default();
    item.resource_count += 1;
    if let Some(n) = resource.usage.spent {
        item.measured_resource_count += 1;
        add(&mut item.known_fresh_tokens, n)?;
    }
    if let Some(n) = resource.usage.consumed {
        add(&mut item.known_total_tokens, n)?;
    }
    if let Some(n) = resource.usage.ceiling_tokens {
        add(&mut item.declared_ceiling_tokens, n)?;
    } else {
        item.undeclared_resource_count += 1;
    }
    add(&mut item.reserved_tokens, resource.usage.reserved)?;
    add(&mut item.drawn_tokens, resource.usage.drawn)
}
pub(super) fn project(page: service::ResourcePage) -> Result<Value, NativeError> {
    let mut rows = Vec::with_capacity(page.resources.len());
    let (mut fresh, mut total, mut reserved, mut drawn, mut measured) = (0, 0, 0, 0, 0);
    for resource in page.resources {
        let usage = &resource.usage;
        add(&mut reserved, usage.reserved)?;
        add(&mut drawn, usage.drawn)?;
        if let Some(n) = usage.spent {
            add(&mut fresh, n)?;
            measured += 1;
        }
        if let Some(n) = usage.consumed {
            add(&mut total, n)?;
        }
        let cache = usage
            .consumed
            .zip(usage.spent)
            .map(|(all, spent)| all.checked_sub(spent).ok_or_else(invalid))
            .transpose()?;
        rows.push(json!({"resourceId":resource.id,"label":label(&resource),"model":resource.model,"period":usage.period,"periodKey":usage.spend_period_key,"ceilingTokens":usage.ceiling_tokens,"reservedTokens":usage.reserved,"knownFreshTokens":usage.spent,"knownTotalTokens":usage.consumed,"cacheReadTokens":cache,"drawnTokens":usage.drawn,"busyMs":null,"tasks":null,"evidence":usage.evidence}));
    }
    let complete = measured == rows.len();
    bounded(
        json!({"protocol":"hagency-web-overview-v1","observedAtMs":page.observed_at_ms,"items":rows,"nextAfter":page.next_after,"totals":{"scope":"page","resourceCount":rows.len(),"measuredResourceCount":measured,"usageComplete":complete,"knownFreshTokens":(measured>0).then_some(fresh),"knownTotalTokens":(measured>0).then_some(total),"reservedTokens":reserved,"drawnTokens":drawn},"unavailable":["busy_time","task_counts","historical_series"],"source":"retained_resource_usage","measurementIsLowerBound":true}),
    )
}
impl NativeManagement {
    pub async fn web_usage(&self, query: WebOverviewQuery) -> Result<Value, NativeError> {
        query.validate()?;
        let (context, session) = self.authorized().await?;
        let page = session
            .resources(&context, query.resource_query())
            .await
            .map_err(super::super::management::store_error)?;
        let mut totals = std::collections::BTreeMap::<String, Totals>::new();
        // A bounded whole-scope read supplies chart buckets without treating
        // the currently displayed sixteen rows as the entire fleet. Every
        // page still passes through the original profile grant checks.
        let mut after = String::new();
        let mut scan_next = None;
        for scan in 0..16 {
            let fleet = session
                .resources(
                    &context,
                    service::ResourceQuery {
                        after,
                        ..Default::default()
                    },
                )
                .await
                .map_err(super::super::management::store_error)?;
            for resource in &fleet.resources {
                accumulate(&mut totals, resource)?;
            }
            scan_next = fleet.next_after;
            let Some(next) = &scan_next else {
                break;
            };
            after = next.clone();
            if scan == 15 {
                break;
            }
        }
        let mut result = project(page)?;
        result["pageTotals"] = result["totals"].clone();
        let resource_count = totals.values().map(|t| t.resource_count).sum::<usize>();
        let measured = totals
            .values()
            .map(|t| t.measured_resource_count)
            .sum::<usize>();
        let mut by_period = Vec::new();
        for (period, t) in totals {
            by_period.push(json!({"period":period,"resourceCount":t.resource_count,"measuredResourceCount":t.measured_resource_count,"knownFreshTokens":(t.measured_resource_count>0).then_some(t.known_fresh_tokens),"knownTotalTokens":(t.measured_resource_count>0).then_some(t.known_total_tokens),"reservedTokens":t.reserved_tokens,"drawnTokens":t.drawn_tokens,"declaredCeilingTokens":t.declared_ceiling_tokens,"undeclaredResourceCount":t.undeclared_resource_count}));
        }
        let only = (by_period.len() == 1)
            .then(|| by_period[0].clone())
            .unwrap_or(Value::Null);
        result["totals"] = json!({"scope":"authorized_resources","complete":scan_next.is_none(),"scanNextAfter":scan_next,"resourceCount":resource_count,"measuredResourceCount":measured,"usageComplete":scan_next.is_none()&&resource_count==measured,"byPeriod":by_period,"knownFreshTokens":only["knownFreshTokens"],"knownTotalTokens":only["knownTotalTokens"],"reservedTokens":only["reservedTokens"],"drawnTokens":only["drawnTokens"]});
        let managed = session
            .web_managed_usage(&context)
            .await
            .map_err(super::super::management::store_error)?;
        let mut projects =
            std::collections::BTreeMap::<(String, String), (u64, u64, u64, u64, bool)>::new();
        for row in &managed.items {
            let totals = projects
                .entry((row.project_id.clone(), row.period_key.clone()))
                .or_insert((0, 0, 0, 0, true));
            add(&mut totals.0, row.allocated_tokens)?;
            add(&mut totals.1, row.retained_tokens)?;
            add(&mut totals.2, row.known_consumed_tokens)?;
            add(&mut totals.3, row.call_count)?;
            totals.4 &= row.usage_complete;
        }
        result["managedUsage"] = json!({"supported":managed.supported,"complete":managed.complete,"items":managed.items,"byProject":projects.into_iter().map(|((project,period),(allocated,retained,consumed,calls,complete))|json!({"projectId":project,"periodKey":period,"allocatedTokens":allocated,"retainedTokens":retained,"knownConsumedTokens":consumed,"callCount":calls,"usageComplete":complete})).collect::<Vec<_>>(),"source":"original_managed_call_accounting","measurementIsLowerBound":true,"separateFromResourceTranscriptUsage":true});
        self.web_finish(&context, &[Permission::ReadResources])
            .await?;
        bounded(result)
    }
}
