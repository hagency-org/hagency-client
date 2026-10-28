use super::*;

pub(super) fn account_projection(
    account: service::AccountSummary,
    resources: Vec<service::ResourceSummary>,
    next: Option<String>,
) -> Result<Value, NativeError> {
    // A shared allowance is a single seat observation, never the sum of each
    // resource's copy. Mixed/unreported seat facts cannot imply more quota.
    let mut capacity = Value::Null;
    let mut consistent = true;
    let mut items = Vec::with_capacity(resources.len());
    let mut declared = std::collections::BTreeMap::<String, u64>::new();
    let mut undeclared = 0usize;
    for resource in resources {
        let quota = resource.budget.seat.quota.map(u64::from);
        let committed = u64::from(resource.budget.seat.committed);
        let seat = json!({"quotaTokens":quota,"period":resource.budget.seat.period,"committedTokens":committed,"remainingTokens":resource.budget.seat.remaining,"overcommittedTokens":quota.map(|n|committed.saturating_sub(n)),"status":resource.budget.seat.status});
        if capacity.is_null() {
            capacity = seat;
        } else if capacity != seat {
            consistent = false;
        }
        if let Some(tokens) = resource.usage.ceiling_tokens {
            let period = serde_json::to_value(resource.usage.period)
                .map_err(|_| invalid())?
                .as_str()
                .ok_or_else(invalid)?
                .to_owned();
            let sum = declared.entry(period).or_default();
            *sum = sum
                .checked_add(tokens)
                .filter(|n| *n <= hagency_core::JSON_SAFE_MAX)
                .ok_or_else(invalid)?;
        } else {
            undeclared += 1;
        }
        items.push(json!({"id":resource.id,"label":label(&resource),"model":resource.model,"ceilingTokens":resource.usage.ceiling_tokens,"drawnTokens":resource.usage.drawn}));
    }
    if !consistent {
        capacity = Value::Null;
    }
    let aggregate_complete = next.is_none() && items.len() as u64 == account.resource_count;
    let period = capacity["period"].as_str();
    let declared_same_period = period.and_then(|p| declared.get(p)).copied();
    let oversubscription = if aggregate_complete && undeclared == 0 {
        capacity["quotaTokens"]
            .as_u64()
            .zip(declared_same_period)
            .map(|(quota, total)| total.saturating_sub(quota))
    } else {
        None
    };
    Ok(
        json!({"account":account,"resources":items,"nextResourceAfter":next,"capacity":capacity,"capacityAvailable":!capacity.is_null(),"declaredByPeriod":declared,"undeclaredResourceCount":undeclared,"aggregateComplete":aggregate_complete,"oversubscribedTokens":oversubscription,"resourceTotalsScope":"page","providerPlanVerified":false}),
    )
}

impl NativeManagement {
    pub async fn web_account_capacity(
        &self,
        query: WebAccountCapacityQuery,
    ) -> Result<Value, NativeError> {
        query.validate()?;
        let permissions = [Permission::ReadAccounts];
        let (context, session) = self.authorized().await?;
        authorize(&session, &context, &permissions)?;
        let mut items = Vec::new();
        let (next, observed) = if let Some(id) = query.id {
            let detail = session
                .account_inventory(
                    &context,
                    service::AccountInventoryQuery {
                        id: Some(id),
                        after: query.resource_after,
                    },
                )
                .await
                .map_err(super::super::management::store_error)?;
            items.push(account_projection(
                detail.account.ok_or_else(invalid)?,
                detail.resources,
                detail.next_after,
            )?);
            (None, detail.observed_at_ms)
        } else {
            let page = session
                .account_inventory(
                    &context,
                    service::AccountInventoryQuery {
                        id: None,
                        after: query.after,
                    },
                )
                .await
                .map_err(super::super::management::store_error)?;
            for account in page.accounts {
                let detail = session
                    .account_inventory(
                        &context,
                        service::AccountInventoryQuery {
                            id: Some(account.id),
                            after: None,
                        },
                    )
                    .await
                    .map_err(super::super::management::store_error)?;
                items.push(account_projection(
                    detail.account.ok_or_else(invalid)?,
                    detail.resources,
                    detail.next_after,
                )?);
            }
            (page.next_after, page.observed_at_ms)
        };
        self.web_finish(&context, &permissions).await?;
        bounded(
            json!({"protocol":"hagency-web-overview-v1","observedAtMs":observed,"items":items,"nextAfter":next}),
        )
    }
}
