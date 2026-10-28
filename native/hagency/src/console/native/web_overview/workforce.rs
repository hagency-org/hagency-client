use super::super::{AgentRequest, ManagementCommand};
use super::*;
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestPage {
    observed_at_ms: i64,
    items: Vec<AgentRequest>,
    next_after: Option<String>,
}
fn validate_page(page: &RequestPage, after: Option<&str>) -> Result<(), NativeError> {
    if page.items.len() > 16 || page.observed_at_ms <= 0 {
        return Err(invalid());
    }
    let mut previous = after;
    for row in &page.items {
        super::super::key(&row.id).map_err(|_| invalid())?;
        if previous.is_some_and(|id| row.id.as_str() <= id) {
            return Err(invalid());
        }
        previous = Some(&row.id);
    }
    if page
        .next_after
        .as_ref()
        .is_some_and(|id| page.items.last().is_none_or(|r| r.id != *id))
    {
        return Err(invalid());
    }
    Ok(())
}
fn row_projection(
    row: AgentRequest,
    link: Option<service::WebWorkforceLink>,
    budget: Value,
    allocation: Value,
) -> Result<Value, NativeError> {
    if let Some(link) = &link {
        if link.request_id != row.id
            || link.agent_id != row.proposed_agent_id
            || link.project_id != row.project_id
        {
            return Err(invalid());
        }
    }
    let model = link
        .as_ref()
        .filter(|l| l.model_revision_matches)
        .map(|l| l.model.clone());
    let reasoning = link
        .as_ref()
        .filter(|l| l.model_revision_matches)
        .and_then(|l| l.reasoning.clone());
    Ok(
        json!({"id":row.id,"agentId":row.proposed_agent_id,"name":row.definition.name,"state":row.state,"projectId":row.project_id,"projectName":row.project_name,"managerMxid":row.manager_mxid,"capability":row.definition.capability,"requestedTokens":row.requested_tokens,"periodKey":row.period_key,"model":model,"reasoning":reasoning,"account":link.as_ref().and_then(|l|l.account_id.as_ref()),"localBinding":link,"budget":budget,"allocation":allocation,"source":"project","canOpenAgent":true}),
    )
}
enum Row {
    Project(AgentRequest),
    Owner(service::WebManagedUsageRow),
}
fn merge_rows(
    project: Vec<AgentRequest>,
    owned: Vec<service::WebManagedUsageRow>,
    after: Option<&str>,
    limit: usize,
    project_more: bool,
) -> (Vec<(String, Row)>, Option<String>) {
    let mut rows = BTreeMap::new();
    for row in owned {
        if after.is_none_or(|after| row.request_id.as_str() > after) {
            rows.insert(row.request_id.clone(), Row::Owner(row));
        }
    }
    for row in project {
        rows.insert(row.id.clone(), Row::Project(row));
    }
    let more = project_more || rows.len() > limit;
    let rows = rows.into_iter().take(limit).collect::<Vec<_>>();
    let next = if more {
        rows.last().map(|r| r.0.clone())
    } else {
        None
    };
    (rows, next)
}
fn owner_projection(
    row: service::WebManagedUsageRow,
    link: service::WebWorkforceLink,
) -> Result<Value, NativeError> {
    if row.request_id != link.request_id
        || row.agent_id != link.agent_id
        || row.project_id != link.project_id
        || row.allocation_id != link.allocation_id
    {
        return Err(invalid());
    }
    let budget = json!({"supported":true,"source":"original_managed_call_accounting","observation":{"effectiveCeilingTokens":link.allocated_tokens,"nativeRetainedTokens":link.retained_tokens,"knownConsumedTokens":link.known_consumed_tokens,"usageComplete":link.usage_complete}});
    let allocation = json!({"supported":true,"scope":"original_binding","observation":{"allocation":{"id":link.allocation_id,"projectId":link.project_id}}});
    let model = link.model_revision_matches.then_some(link.model.clone());
    let reasoning = link
        .model_revision_matches
        .then_some(link.reasoning.clone())
        .flatten();
    Ok(
        json!({"id":row.request_id,"agentId":row.agent_id,"name":row.name,"state":row.state,"projectId":row.project_id,"projectName":null,"managerMxid":row.manager_mxid,"capability":row.capability,"requestedTokens":row.allocated_tokens,"periodKey":row.period_key,"model":model,"reasoning":reasoning,"account":link.account_id,"localBinding":link,"budget":budget,"allocation":allocation,"source":"local_owner","canOpenAgent":false}),
    )
}
impl NativeManagement {
    pub async fn web_workforce(&self, query: WebOverviewQuery) -> Result<Value, NativeError> {
        query.validate()?;
        let permissions = [
            Permission::ReadAgentRequests,
            Permission::ReadAgentBudget,
            Permission::ReadAllocations,
            Permission::ReadResources,
            Permission::ReadAccounts,
        ];
        let (context, session) = self.authorized().await?;
        authorize(&session, &context, &permissions)?;
        let page: RequestPage = serde_json::from_value(
            self.central(ManagementCommand::AgentRequests {
                project_id: None,
                after: query.after.clone(),
                needs_my_action: false,
            })
            .await?,
        )
        .map_err(|_| invalid())?;
        validate_page(&page, query.after.as_deref())?;
        let limit = usize::from(query.limit.unwrap_or(16));
        let owned = session
            .web_owned_workforce(&context, query.after.clone())
            .await
            .map_err(super::super::management::store_error)?;
        let (rows, next_after) = merge_rows(
            page.items,
            owned.items,
            query.after.as_deref(),
            limit,
            page.next_after.is_some() || !owned.complete,
        );
        let mut links: BTreeMap<_, _> = session
            .web_workforce_links(&context, rows.iter().map(|r| r.0.clone()).collect())
            .await
            .map_err(super::super::management::store_error)?
            .into_iter()
            .map(|l| (l.request_id.clone(), l))
            .collect();
        let mut allocations = BTreeMap::new();
        let mut items = Vec::with_capacity(rows.len());
        for (id, row) in rows {
            let link = links.remove(&id);
            let row = match row {
                Row::Owner(row) => {
                    items.push(owner_projection(
                        row,
                        link.ok_or_else(|| failure(409, "management_revision_conflict"))?,
                    )?);
                    continue;
                }
                Row::Project(row) => row,
            };
            let allocation = if let Some(link) = &link {
                if !allocations.contains_key(&link.allocation_id) {
                    let value = self
                        .central(ManagementCommand::Allocation {
                            id: link.allocation_id.clone(),
                        })
                        .await?;
                    // The existing closed allocation projection strips receipts
                    // and credentials. This is the original allocation ID, not
                    // a newly inferred current-project allocation.
                    allocations.insert(
                        link.allocation_id.clone(),
                        json!({"supported":true,"observation":value["detail"].clone()}),
                    );
                }
                allocations[&link.allocation_id].clone()
            } else {
                json!({"supported":false,"observation":null,"reason":"local_binding_unavailable"})
            };
            let budget = if row.state == "approved" {
                match self.agent_budget(row.id.clone()).await {
                    Ok(value) => {
                        json!({"supported":value["supported"],"observation":value["budget"],"observedAtMs":value["observedAtMs"]})
                    }
                    Err(error) if error.status == 404 => {
                        json!({"supported":false,"observation":null,"reason":"not_assigned"})
                    }
                    Err(error) => return Err(error),
                }
            } else {
                json!({"supported":false,"observation":null,"reason":"request_not_approved"})
            };
            items.push(row_projection(row, link, budget, allocation)?);
        }
        self.web_finish(&context, &permissions).await?;
        bounded(
            json!({"protocol":"hagency-web-overview-v1","observedAtMs":page.observed_at_ms,"items":items,"nextAfter":next_after,"scope":"visible_agent_requests_and_owned_resources","ownerInventoryComplete":owned.complete}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn owned(id: &str) -> service::WebManagedUsageRow {
        service::WebManagedUsageRow {
            request_id: id.into(),
            agent_id: format!("agent_{id}"),
            name: "Same human label".into(),
            capability: "documentation".into(),
            manager_mxid: "@manager:test".into(),
            state: "reserved".into(),
            project_id: "project_original".into(),
            allocation_id: "allocation_original".into(),
            resource_id: "resource_original".into(),
            period_key: "2026-10".into(),
            allocated_tokens: 100,
            retained_tokens: 100,
            known_consumed_tokens: 7,
            usage_complete: true,
            call_count: 1,
        }
    }
    fn project(id: &str) -> AgentRequest {
        serde_json::from_value(json!({"id":id,"projectId":"project_original","projectName":"Original project","proposedAgentId":format!("agent_{id}"),"managerMxid":"@manager:test","administratorMxid":"@admin:test","authorityRevision":1,"definition":{"name":"Same human label","capability":"documentation","description":"","toolMode":"none"},"definitionDigest":"a".repeat(64),"requestedTokens":100,"periodKey":"2026-10","state":"approved","revision":2,"createdAtMs":1,"expiresAtMs":100,"canDecide":false,"canApprove":false,"canReject":false,"canCancel":false,"blockingReason":null,"executionAvailable":true})).unwrap()
    }
    #[test]
    fn web_workforce_merge_keeps_resource_owner_agents_and_exact_cursor() {
        let (first, next) = merge_rows(
            vec![project("r_b"), project("r_d")],
            vec![owned("r_c"), owned("r_b"), owned("r_a")],
            None,
            2,
            false,
        );
        assert_eq!(
            first.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
            vec!["r_a", "r_b"]
        );
        assert!(matches!(first[0].1, Row::Owner(_)));
        assert!(
            matches!(first[1].1, Row::Project(_)),
            "same exact request gets one row"
        );
        assert_eq!(next.as_deref(), Some("r_b"));
        let (second, next) = merge_rows(
            vec![project("r_d")],
            vec![owned("r_c"), owned("r_b"), owned("r_a")],
            Some("r_b"),
            2,
            false,
        );
        assert_eq!(
            second.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
            vec!["r_c", "r_d"]
        );
        assert!(next.is_none());
        let (owner, _) = merge_rows(vec![], vec![owned("r_a")], None, 16, false);
        assert_eq!(
            owner.len(),
            1,
            "empty manager-only server list must not erase owner workforce"
        );
    }
    #[test]
    fn web_owner_projection_does_not_invent_server_navigation_or_model_identity() {
        let link = service::WebWorkforceLink {
            request_id: "r_a".into(),
            agent_id: "agent_r_a".into(),
            project_id: "project_original".into(),
            allocation_id: "allocation_original".into(),
            resource_id: "resource_original".into(),
            account_id: Some("account_original".into()),
            model: "new-configured-model".into(),
            reasoning: None,
            model_revision_matches: false,
            period_key: "2026-10".into(),
            allocated_tokens: 100,
            retained_tokens: 100,
            known_consumed_tokens: 7,
            usage_complete: true,
            process_state: "stopped".into(),
        };
        let v = owner_projection(owned("r_a"), link).unwrap();
        assert_eq!(v["source"], "local_owner");
        assert_eq!(v["canOpenAgent"], false);
        assert_eq!(v["account"], "account_original");
        assert!(
            v["model"].is_null(),
            "later configuration is not original runtime identity"
        );
        assert_eq!(v["budget"]["observation"]["knownConsumedTokens"], 7);
        assert_eq!(
            v["allocation"]["observation"]["allocation"]["id"],
            "allocation_original"
        );
    }
}
