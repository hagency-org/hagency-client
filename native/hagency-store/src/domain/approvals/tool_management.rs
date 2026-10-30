//! Scoped management uses retained rows; it does not submit approval verdicts.
use super::*;
use super::super::{APPROVAL_SELECT, approval_row, approval_tuple};
use crate::management::{
    CommandReceipt, RevokeToolGrant, RevokedToolGrant, ToolApproval, ToolApprovalLookup,
    ToolApprovalPage, ToolApprovalQuery, ToolGrant, ToolGrantMode, ToolGrantPage, ToolGrantQuery,
    ToolRule, ToolRuleDetails, ToolRulePermissions, ToolApprovalSubject, ToolApprovalSnapshot,
};

const GRANT_SELECT: &str = "SELECT g.id,e.resource_id,e.id,json_extract(e.projection,'$.agentName'),g.scope_kind,g.mode,g.task_id,g.revoked,g.binding_generation,g.scope_key,g.context_key,EXISTS(SELECT 1 FROM current_approval_bindings current JOIN approval_bindings b ON b.engagement_id=current.engagement_id WHERE b.engagement_id=g.engagement_id AND b.incarnation=g.binding_generation) FROM approval_grants g JOIN engagements e ON e.id=g.engagement_id";

struct StoredGrant {
    item: ToolGrant,
    scope_key: String,
    kind_json: String,
    context_key: String,
    binding_generation: u64,
}

fn grant_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredGrant> {
    let mode: String = row.get(5)?;
    Ok(StoredGrant {
        item: ToolGrant {
            id: row.get(0)?,
            resource_id: row.get(1)?,
            engagement_id: row.get(2)?,
            agent: row.get(3)?,
            kind: String::new(),
            mode: match mode.as_str() {
                "task" => ToolGrantMode::Task,
                "always" => ToolGrantMode::Always,
                _ => return Err(rusqlite::Error::InvalidQuery),
            },
            task_id: row.get(6)?,
            revoked: row.get(7)?,
            binding_current: row.get(11)?,
            details: None,
            revision: String::new(),
        },
        kind_json: row.get(4)?,
        binding_generation: row.get(8)?,
        scope_key: row.get(9)?,
        context_key: row.get(10)?,
    })
}

fn project_grant(db: &Connection, mut grant: StoredGrant) -> Result<ToolGrant, Error> {
    grant.item.kind = serde_json::from_str(&grant.kind_json)?;
    if ![
        "exact_command",
        "network_host",
        "permission_profile",
        "exact_tool_call",
    ]
    .contains(&grant.item.kind.as_str())
    {
        return Err(Error::Schema);
    }
    // Requests reusing a grant carry the same scope and context. Check their
    // retained relationship before exposing the saved rule's human description.
    // Neither the raw request config nor the context itself leaves this method.
    let detail: Option<(Option<String>, String, String)> = db.query_row(
        "SELECT a.description,c.config,a.config FROM owner_approvals a JOIN approval_contexts c ON c.id=a.context_id WHERE a.grant_id=?1 AND a.scope_key=?2 AND a.scope_kind=?3 AND c.engagement_id=?4 ORDER BY a.id LIMIT 1",
        params![grant.item.id, grant.scope_key, grant.kind_json, grant.item.engagement_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).optional()?;
    if let Some((Some(description), encoded, request)) = detail {
        let context: Context = serde_json::from_str(&encoded)?;
        if context.binding.engagement != grant.item.engagement_id
            || context.binding.generation != grant.binding_generation
            || grant_context(&context)? != grant.context_key
        {
            return Err(Error::Schema);
        }
        // Match the retained execution scope contract (UTF-16 units), not a
        // smaller byte limit that would reject valid Chinese descriptions.
        if description.contains('\0') || description.encode_utf16().count() > 12_000 {
            return Err(Error::Capacity);
        }
        matrix_user(&context.binding.owner, &context.binding.server)?;
        identifier(&context.binding.project, 128)?;
        let rule = saved_rule(&context, &request, &grant, &description)?;
        grant.item.details = Some(ToolRuleDetails {
            description,
            project_id: context.binding.project,
            owner_mxid: context.binding.owner,
            rule,
        });
    }
    // The structured rule repeats the description's facts. A rule too large to
    // ship beside a maximal description is omitted, never failing the read.
    if serde_json::to_vec(&grant.item)?.len() > 80 * 1024
        && let Some(details) = grant.item.details.as_mut()
    {
        details.rule = None;
    }
    grant.item.revision = canonical::digest(&serde_json::to_value(&grant.item)?)?;
    bounded(&grant.item, 80 * 1024)?;
    Ok(grant.item)
}

/// Re-derive a saved scope from its original request with the same pure
/// function that derived it at admission. Structured fields are returned only
/// when the recomputed key, kind and description equal the stored ones, so
/// they state exactly what the description states and nothing that changed.
fn saved_rule(
    context: &Context,
    request: &str,
    grant: &StoredGrant,
    description: &str,
) -> Result<Option<ToolRule>, Error> {
    let request: Request = serde_json::from_str(request)?;
    let Some(derived) = policy::derive(HostRequest {
        agent_id: &context.binding.engagement,
        workspace: &context.workspace,
        task_id: Some(&context.task),
        may_write: context.may_write,
        method: &request.method,
        params: &request.params,
        path_flavor: if context.windows {
            PathFlavor::Windows
        } else {
            PathFlavor::Posix
        },
    }) else {
        return Ok(None);
    };
    let scope = derived.scope;
    if scope.key != grant.scope_key
        || scope.description != description
        || serde_json::to_value(scope.kind)? != Value::String(grant.item.kind.clone())
    {
        return Ok(None);
    }
    let value = &scope.value;
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    let runtime = match request.method.as_str() {
        policy::CLAUDE_TOOL_METHOD => "claude",
        policy::OCTOS_APPROVAL_METHOD => "octos",
        _ => "codex",
    };
    let mut rule = ToolRule {
        runtime: runtime.into(),
        tool: None,
        command: None,
        cwd: None,
        host: None,
        protocol: None,
        input: None,
        permissions: None,
    };
    match scope.kind {
        ScopeKind::ExactCommand => {
            rule.command = text("command");
            rule.cwd = text("cwd");
            rule.permissions = value
                .get("additionalPermissions")
                .filter(|v| !v.is_null())
                .map(rule_permissions);
        }
        ScopeKind::NetworkHost => {
            rule.host = text("host");
            rule.protocol = text("protocol");
        }
        ScopeKind::PermissionProfile => {
            rule.cwd = text("cwd");
            rule.permissions = value.get("permissions").map(rule_permissions);
        }
        ScopeKind::ExactToolCall => {
            rule.tool = text("tool");
            // The scope holds the canonical input text the description shows.
            rule.input = value
                .get("input")
                .and_then(Value::as_str)
                .map(serde_json::from_str)
                .transpose()?;
        }
    }
    Ok(Some(rule))
}

/// A normalized permission profile as the network flag and sorted path lists
/// the description lists line by line.
fn rule_permissions(profile: &Value) -> ToolRulePermissions {
    let fs = &profile["fileSystem"];
    let paths = |mode: &str| {
        let mut paths: Vec<String> = fs[mode]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .chain(
                fs["entries"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|entry| entry["access"] == mode)
                    .filter_map(|entry| entry["path"]["path"].as_str()),
            )
            .map(str::to_owned)
            .collect();
        paths.sort();
        paths.dedup();
        paths
    };
    ToolRulePermissions {
        network: profile["network"]["enabled"].as_bool(),
        read: paths("read"),
        write: paths("write"),
    }
}

fn bounded(value: &impl Serialize, limit: usize) -> Result<(), Error> {
    if serde_json::to_vec(value)?.len() > limit {
        return Err(Error::Capacity);
    }
    Ok(())
}

fn require_engagement(db: &Connection, resource: &str, engagement: &str) -> Result<(), Error> {
    if !db.query_row(
        "SELECT EXISTS(SELECT 1 FROM engagements WHERE id=?1 AND resource_id=?2)",
        params![engagement, resource],
        |row| row.get::<_, bool>(0),
    )? {
        return Err(Error::NotFound);
    }
    Ok(())
}

fn approval_subject(
    db: &Connection,
    resource: &str,
    engagement: &str,
) -> Result<ToolApprovalSubject, Error> {
    require_engagement(db, resource, engagement)?;
    let value = super::super::read_engagement(db, engagement)?;
    if value.id != engagement || value.resource_id != resource {
        return Err(Error::Schema);
    }
    let subject = ToolApprovalSubject {
        engagement_id: value.id,
        agent: value.agent_name.as_str().to_owned(),
        project_id: value.project_id,
        project_name: value.project_name,
    };
    bounded(&subject, 4096)?;
    Ok(subject)
}

impl DomainRepository {
    pub(crate) fn management_tool_approvals(
        &self,
        query: &ToolApprovalQuery,
        check: impl Fn(&Connection) -> Result<(), Error>,
    ) -> Result<ToolApprovalPage, Error> {
        query.validate()?;
        let tx = self.db.unchecked_transaction()?;
        check(&tx)?;
        if !query.engagement_id.is_empty() {
            require_engagement(&tx, &query.resource_id, &query.engagement_id)?;
        }
        let mut statement = tx.prepare(&format!("{APPROVAL_SELECT} WHERE e.resource_id=?1 AND (?2='' OR e.id=?2) AND a.id>?3 ORDER BY a.id LIMIT 17"))?;
        let rows = statement
            .query_map(
                params![query.resource_id, query.engagement_id, query.after],
                approval_tuple,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        let more = rows.len() > 16;
        let approvals = rows
            .into_iter()
            .take(16)
            .map(|row| {
                let value: ToolApproval = serde_json::from_value(approval_row(row)?)?;
                clock(value.expires_at)?;
                Ok(value)
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let next_after = if more {
            approvals.last().map(|row| row.id.clone())
        } else {
            None
        };
        let subjects = approvals
            .iter()
            .map(|row| row.engagement_id.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(|engagement| approval_subject(&tx, &query.resource_id, engagement))
            .collect::<Result<Vec<_>, _>>()?;
        let result = ToolApprovalPage {
            resource_id: query.resource_id.clone(),
            engagement_id: query.engagement_id.clone(),
            approvals,
            subjects,
            next_after,
            observed_at_ms: crate::domain::graphs::now_ms()?,
        };
        bounded(&result, 128 * 1024)?;
        check(&tx)?;
        tx.commit()?;
        Ok(result)
    }

    pub(crate) fn management_tool_approval(
        &self,
        query: &ToolApprovalLookup,
        check: impl Fn(&Connection) -> Result<(), Error>,
    ) -> Result<ToolApprovalSnapshot, Error> {
        query.validate()?;
        let tx = self.db.unchecked_transaction()?;
        check(&tx)?;
        let row = tx
            .query_row(
                &format!("{APPROVAL_SELECT} WHERE e.resource_id=?1 AND a.id=?2"),
                params![query.resource_id, query.approval_id],
                approval_tuple,
            )
            .optional()?
            .ok_or(Error::NotFound)?;
        let approval: ToolApproval = serde_json::from_value(approval_row(row)?)?;
        clock(approval.expires_at)?;
        let subject = approval_subject(&tx, &query.resource_id, &approval.engagement_id)?;
        let result = ToolApprovalSnapshot {
            resource_id: query.resource_id.clone(),
            approval,
            subject,
            observed_at_ms: crate::domain::graphs::now_ms()?,
        };
        bounded(&result, 8 * 1024)?;
        check(&tx)?;
        tx.commit()?;
        Ok(result)
    }

    pub(crate) fn management_tool_grants(
        &self,
        query: &ToolGrantQuery,
        check: impl Fn(&Connection) -> Result<(), Error>,
    ) -> Result<ToolGrantPage, Error> {
        query.validate()?;
        let tx = self.db.unchecked_transaction()?;
        check(&tx)?;
        require_engagement(&tx, &query.resource_id, &query.engagement_id)?;
        let mut statement = tx.prepare(&format!(
            "{GRANT_SELECT} WHERE e.resource_id=?1 AND e.id=?2 AND g.id>?3 ORDER BY g.id LIMIT 17"
        ))?;
        let rows = statement
            .query_map(
                params![query.resource_id, query.engagement_id, query.after],
                grant_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        let mut more = rows.len() > 16;
        let mut grants = Vec::new();
        // A valid description can contain 12,000 UTF-16 units and need JSON
        // escaping. Page on both rows and encoded bytes without truncating it.
        let mut remaining = 126 * 1024;
        for row in rows.into_iter().take(16) {
            let grant = project_grant(&tx, row)?;
            let bytes = serde_json::to_vec(&grant)?.len() + 1;
            if bytes > remaining {
                more = true;
                break;
            }
            remaining -= bytes;
            grants.push(grant);
        }
        let next_after = if more {
            grants.last().map(|row| row.id.clone())
        } else {
            None
        };
        let result = ToolGrantPage {
            resource_id: query.resource_id.clone(),
            engagement_id: query.engagement_id.clone(),
            grants,
            next_after,
            observed_at_ms: crate::domain::graphs::now_ms()?,
        };
        bounded(&result, 128 * 1024)?;
        check(&tx)?;
        tx.commit()?;
        Ok(result)
    }

    pub(crate) fn management_revoke_tool_grant(
        &mut self,
        input: &RevokeToolGrant,
        receipt: CommandReceipt,
        check: impl Fn(&Connection) -> Result<(), Error>,
    ) -> Result<RevokedToolGrant, Error> {
        input.validate()?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        check(&tx)?;
        if let Some(original) = receipt.lookup::<RevokedToolGrant>(&tx)? {
            check(&tx)?;
            return Ok(original);
        }
        let row = tx
            .query_row(
                &format!("{GRANT_SELECT} WHERE e.resource_id=?1 AND e.id=?2 AND g.id=?3"),
                params![input.resource_id, input.engagement_id, input.grant_id],
                grant_row,
            )
            .optional()?
            .ok_or(Error::NotFound)?;
        let grant = project_grant(&tx, row)?;
        if grant.revision != input.expected_revision {
            return Err(Error::Conflict);
        }
        tx.execute(
            "UPDATE approval_grants SET revoked=1 WHERE id=?1",
            [&input.grant_id],
        )?;
        let result = RevokedToolGrant {
            resource_id: grant.resource_id,
            engagement_id: grant.engagement_id,
            grant_id: grant.id,
            reviewed_revision: grant.revision,
            revoked: true,
        };
        receipt.record(&tx, &result)?;
        check(&tx)?;
        tx.commit().map_err(|_| Error::OutcomeUnknown)?;
        Ok(result)
    }
}
