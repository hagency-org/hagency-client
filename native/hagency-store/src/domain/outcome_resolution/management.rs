//! Reviewed native commands reuse the console's domain transactions. The
//! inspection credential never leaves this module or enters command journals.
use super::*;
use crate::management::{
    CommandReceipt, ResolveStopped, ReviewStopped, StoppedAttempt, StoppedDecision,
    StoppedInspection, StoppedInventoryEntry, StoppedListQuery, StoppedPage, StoppedQuery,
    StoppedResolution, StoppedReview,
};

fn subject(
    db: &Connection,
    q: &StoppedQuery,
    check: &impl Fn(&Connection, &str) -> Result<(), Error>,
) -> Result<(execution::Dispatch, bool), Error> {
    check(db, &q.resource_id)?;
    if !db.query_row("SELECT EXISTS(SELECT 1 FROM runner_dispatches d JOIN runner_sessions s ON s.id=d.session_id JOIN engagements e ON e.id=s.engagement_id WHERE d.id=?1 AND e.id=?2 AND e.resource_id=?3)",params![q.dispatch_id,q.engagement_id,q.resource_id],|r|r.get::<_,bool>(0))?{return Err(Error::NotFound);}
    let d = bound(db, &q.engagement_id, &q.dispatch_id)?;
    // Settlement may release dirty marks shared with historical users. The
    // selected resource alone is not authority over those other owners.
    let mut all = true;
    let mut statement=db.prepare("SELECT DISTINCT e.resource_id FROM dispatch_resources current JOIN dispatch_resources history ON history.resource_id=current.resource_id JOIN runner_dispatches d ON d.id=history.dispatch_id JOIN runner_sessions s ON s.id=d.session_id JOIN engagements e ON e.id=s.engagement_id WHERE current.dispatch_id=?1 ORDER BY e.resource_id")?;
    for resource in statement.query_map([&q.dispatch_id], |r| r.get::<_, String>(0))? {
        match check(db, &resource?) {
            Ok(()) => {}
            Err(Error::LocalAuthority) => all = false,
            Err(e) => return Err(e),
        }
    }
    check(db, &q.resource_id)?;
    Ok((d, all))
}
fn attempt(
    db: &Connection,
    q: &StoppedQuery,
    d: &execution::Dispatch,
    authority: bool,
) -> Result<StoppedAttempt, Error> {
    let agent = super::super::read_engagement(db, &q.engagement_id)?;
    let reason = db
        .query_row(
            "SELECT reason FROM dispatch_stops WHERE dispatch_id=?1 AND fence=?2",
            params![q.dispatch_id, d.fence],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .unwrap_or_else(|| "runner_lost".into());
    let reason = match reason.as_str() {
        "owned_runner_failure" | "runner_lost" | "operator_stop" => reason,
        _ => "stopped".into(),
    };
    let available=db.query_row("SELECT EXISTS(SELECT 1 FROM owned_stop_inspections WHERE dispatch_id=?1 AND fence=?2) OR EXISTS(SELECT 1 FROM agent_fences WHERE dispatch_id=?1 AND fence=?2 AND cleared_at IS NULL) OR EXISTS(SELECT 1 FROM runner_attempt_events WHERE dispatch_id=?1 AND fence=?2 AND phase='lost')",params![q.dispatch_id,d.fence],|r|r.get(0))?;
    Ok(StoppedAttempt {
        resource_id: q.resource_id.clone(),
        engagement_id: q.engagement_id.clone(),
        dispatch_id: q.dispatch_id.clone(),
        agent: agent.agent_name.as_str().into(),
        project_id: agent.project_id,
        task_id: d.task_id.clone(),
        fence: d.fence,
        reason,
        inspection_available: available,
        shared_resource_authority: authority,
    })
}
fn inventory(
    observation: &Value,
    offset: usize,
) -> Result<(usize, Vec<StoppedInventoryEntry>, Option<usize>), Error> {
    let Some(entries) = observation
        .get("inventory")
        .and_then(|v| v.get("entries"))
        .and_then(Value::as_array)
    else {
        return Ok((0, vec![], None));
    };
    if entries.len() > 1024 || offset > entries.len() {
        return Err(Error::Capacity);
    }
    let limit = entries.len().min(offset + 8);
    let mut end = offset;
    let mut encoded_bytes = 2;
    let mut page = Vec::new();
    for entry in &entries[offset..limit] {
        let path = entry
            .get("path")
            .and_then(Value::as_str)
            .ok_or(Error::State)?;
        if path.is_empty()
            || path.len() > 4096
            || path.chars().any(char::is_control)
            || !std::path::Path::new(path)
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
        {
            return Err(Error::State);
        }
        let kind = entry
            .get("kind")
            .and_then(Value::as_str)
            .ok_or(Error::State)?;
        if !matches!(kind, "file" | "directory" | "symlink") {
            return Err(Error::State);
        }
        let bytes = entry.get("bytes").and_then(Value::as_u64);
        let hash = entry
            .get("sha256")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if kind == "file"
            && (bytes.is_none()
                || hash
                    .as_ref()
                    .is_none_or(|h| h.len() != 64 || !h.bytes().all(|c| c.is_ascii_hexdigit())))
        {
            return Err(Error::State);
        }
        // Do not expose host root identities or symlink targets. These are
        // display-only relative names, never paths a mini-app can open.
        let projected = StoppedInventoryEntry {
            relative_path: path.into(),
            kind: kind.into(),
            bytes: if kind == "file" { bytes } else { None },
            sha256: if kind == "file" { hash } else { None },
        };
        // Count encoded bytes as well as entries: quotes in valid filenames
        // expand in JSON, while the native saved-result envelope is 64 KiB.
        let size = serde_json::to_vec(&projected)?.len() + 1;
        if encoded_bytes + size > 44 * 1024 {
            if page.is_empty() { return Err(Error::Capacity); }
            break;
        }
        encoded_bytes += size;
        page.push(projected);
        end += 1;
    }
    Ok((entries.len(), page, (end < entries.len()).then_some(end)))
}
fn projection(
    db: &Connection,
    q: &StoppedQuery,
    d: &execution::Dispatch,
    current: Option<&Snapshot>,
) -> Result<StoppedInspection, Error> {
    let row = attempt(db, q, d, true)?;
    let task = d
        .task_id
        .as_deref()
        .map(|id| execution::task(db, id))
        .transpose()?;
    let historical = if current.is_none() {
        db.query_row(
            "SELECT config FROM owned_stop_inspections WHERE dispatch_id=?1 AND fence=?2",
            params![q.dispatch_id, d.fence],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .map(|s| serde_json::from_str::<Value>(&s))
        .transpose()?
    } else {
        None
    };
    let observation = current
        .map(|s| &s.value["observation"])
        .or(historical.as_ref())
        .ok_or(Error::StoppedNotResolvable)?;
    let (entry_count, entries, next_entry) = inventory(observation, q.entry_offset)?;
    let revision = if let Some(snapshot) = current {
        snapshot.digest.clone()
    } else {
        canonical::digest(&json!([
            "historical-stopped-inspection",
            row,
            observation,
            task
        ]))?
    };
    let result = StoppedInspection {
        attempt: row,
        task_title: task.as_ref().map(|t| t.title.clone()),
        task_status: task
            .as_ref()
            .map(|t| serde_json::to_value(t.status).map(|v| v.as_str().unwrap_or_default().into()))
            .transpose()?,
        revision,
        resolvable: current.is_some(),
        can_continue: current.is_some_and(|s| s.value["fenced"] == false),
        entry_count,
        entries,
        next_entry,
    };
    if serde_json::to_vec(&result)?.len() > 128 * 1024 {
        return Err(Error::Capacity);
    }
    Ok(result)
}
fn unresolved(error: Error) -> Error {
    match error {
        Error::State | Error::Quarantined | Error::RunnerAuthority | Error::Generation => {
            Error::StoppedNotResolvable
        }
        error => error,
    }
}

impl DomainRepository {
    pub(crate) fn management_stopped_tasks(
        &self,
        q: &StoppedListQuery,
        check: impl Fn(&Connection, &str) -> Result<(), Error>,
    ) -> Result<StoppedPage, Error> {
        q.validate()?;
        let tx = self.db.unchecked_transaction()?;
        check(&tx, &q.resource_id)?;
        if !tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM resources WHERE id=?1)",
            [&q.resource_id],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(Error::NotFound);
        }
        let mut statement=tx.prepare("SELECT d.id,e.id FROM runner_dispatches d JOIN runner_sessions s ON s.id=d.session_id JOIN engagements e ON e.id=s.engagement_id LEFT JOIN dispatch_stops stop ON stop.dispatch_id=d.id AND stop.fence=d.fence WHERE e.resource_id=?1 AND d.id>?2 AND d.state='outcome_unknown' AND stop.settled_at IS NULL AND NOT EXISTS(SELECT 1 FROM outcome_resolutions r WHERE r.dispatch_id=d.id) AND (?3='' OR EXISTS(SELECT 1 FROM dispatch_resources w WHERE w.dispatch_id=d.id AND w.resource_id=?3)) ORDER BY d.id LIMIT 17")?;
        let ids = statement
            .query_map(params![q.resource_id, q.after, q.workspace_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        let more = ids.len() > 16;
        let mut dispatches = Vec::new();
        for (dispatch_id, engagement_id) in ids.into_iter().take(16) {
            let query = StoppedQuery {
                resource_id: q.resource_id.clone(),
                engagement_id,
                dispatch_id,
                entry_offset: 0,
            };
            let (d, all) = subject(&tx, &query, &check)?;
            dispatches.push(attempt(&tx, &query, &d, all)?);
        }
        let next_after = if more {
            dispatches.last().map(|r| r.dispatch_id.clone())
        } else {
            None
        };
        let result = StoppedPage {
            resource_id: q.resource_id.clone(),
            dispatches,
            next_after,
        };
        if serde_json::to_vec(&result)?.len() > 128 * 1024 {
            return Err(Error::Capacity);
        }
        check(&tx, &q.resource_id)?;
        tx.commit()?;
        Ok(result)
    }
    pub(crate) fn management_stopped_inspection(
        &self,
        q: &StoppedQuery,
        check: impl Fn(&Connection, &str) -> Result<(), Error>,
    ) -> Result<StoppedInspection, Error> {
        q.validate()?;
        let tx = self.db.unchecked_transaction()?;
        let (d, all) = subject(&tx, q, &check)?;
        if !all {
            return Err(Error::LocalAuthority);
        }
        let current = match snapshot(&tx, &q.dispatch_id, &d) {
            Ok(s) => Some(s),
            Err(Error::State | Error::Quarantined | Error::RunnerAuthority | Error::Generation) => {
                None
            }
            Err(e) => return Err(e),
        };
        let result = projection(&tx, q, &d, current.as_ref())?;
        if !subject(&tx, q, &check)?.1 {
            return Err(Error::LocalAuthority);
        }
        tx.commit()?;
        Ok(result)
    }
    pub(crate) fn management_review_stopped(
        &mut self,
        input: &ReviewStopped,
        receipt: CommandReceipt,
        check: impl Fn(&Connection, &str) -> Result<(), Error>,
    ) -> Result<StoppedReview, Error> {
        input.validate()?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let q = input.query();
        let (d, all) = subject(&tx, &q, &check)?;
        if !all {
            return Err(Error::LocalAuthority);
        }
        if let Some(old) = receipt.lookup(&tx)? {
            return Ok(old);
        }
        let current = snapshot(&tx, &q.dispatch_id, &d).map_err(unresolved)?;
        if current.digest != input.expected_revision {
            return Err(Error::Conflict);
        }
        let now = super::super::graphs::now_ms()?;
        let credential =
            begin_inspection_in_transaction(&tx, &q.engagement_id, &q.dispatch_id, 900_000, now)
                .map_err(unresolved)?;
        let result = StoppedReview {
            review_command_id: input.command_id.clone(),
            inspection_id: credential["inspectionId"]
                .as_str()
                .ok_or(Error::State)?
                .into(),
            expires_at: credential["expiresAt"].as_u64().ok_or(Error::State)?,
            inspection: projection(&tx, &q, &d, Some(&current))?,
        };
        // The one-use console token is deliberately discarded. Native recovery
        // proves its original scoped review using the atomic command receipt.
        receipt.record(&tx, &result)?;
        if !subject(&tx, &q, &check)?.1 {
            return Err(Error::LocalAuthority);
        }
        tx.commit().map_err(|_| Error::OutcomeUnknown)?;
        Ok(result)
    }
    pub(crate) fn management_resolve_stopped(
        &mut self,
        input: &ResolveStopped,
        receipt: CommandReceipt,
        inspected: CommandReceipt,
        request_id: &str,
        check: impl Fn(&Connection, &str) -> Result<(), Error>,
    ) -> Result<StoppedResolution, Error> {
        input.validate()?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let q = input.review().query();
        let (d, all) = subject(&tx, &q, &check)?;
        if !all {
            return Err(Error::LocalAuthority);
        }
        if let Some(old) = receipt.lookup(&tx)? {
            return Ok(old);
        }
        // Receipt lookup binds account, installation, profile and all reviewed
        // input. An inspection ID by itself grants no native authority.
        let review = inspected
            .lookup::<StoppedReview>(&tx)?
            .ok_or(Error::StoppedNotResolvable)?;
        let now = super::super::graphs::now_ms()?;
        if now >= review.expires_at {
            return Err(Error::StoppedReviewExpired);
        }
        if review.inspection.revision != input.review_revision
            || review.inspection.attempt.dispatch_id != input.dispatch_id
        {
            return Err(Error::Conflict);
        }
        let hash:String=tx.query_row("SELECT token_hash FROM outcome_inspections WHERE id=?1 AND dispatch_id=?2 AND snapshot_digest=?3",params![review.inspection_id,input.dispatch_id,input.review_revision],|r|r.get(0)).optional()?.ok_or(Error::StoppedNotResolvable)?;
        let replacement = if input.action == StoppedDecision::Continue {
            if !review.inspection.can_continue {
                return Err(Error::StoppedNotResolvable);
            }
            let mut next: DispatchInput = serde_json::from_str(&d.input)?;
            next.id = format!("native_continue_{request_id}");
            let object = next.payload.as_object_mut().ok_or(Error::State)?;
            // Input custody is rebuilt from persisted inbox rows by the recovery
            // kernel. Reusing the old serialized envelopes is not a new grant.
            for key in [
                "inbox",
                "agentInbox",
                "recoveryInbox",
                "peerInbox",
                "recoveryPeerInbox",
            ] {
                object.remove(key);
            }
            object.insert("instruction".into(), json!(input.continuation_instruction));
            next.validate()?;
            Some(next)
        } else {
            None
        };
        let data = Resolution {
            original: &input.dispatch_id,
            request_id,
            inspection_id: &review.inspection_id,
            action: input.action.action(),
            operator_note: &input.operator_note,
            replacement: replacement.as_ref(),
        };
        let digest = canonical::digest(&json!([
            "native-reviewed-outcome-v1",
            request_id,
            input,
            review.inspection_id
        ]))?;
        let outcome = resolve_in_transaction(
            &tx,
            &input.engagement_id,
            &data,
            &digest,
            now,
            |candidate| Ok(candidate == hash),
        )
        .map_err(unresolved)?;
        let result = StoppedResolution {
            command_id: input.command_id.clone(),
            resource_id: input.resource_id.clone(),
            engagement_id: input.engagement_id.clone(),
            dispatch_id: input.dispatch_id.clone(),
            action: input.action,
            task_id: outcome["task"]["id"].as_str().ok_or(Error::State)?.into(),
            task_status: outcome["task"]["status"]
                .as_str()
                .ok_or(Error::State)?
                .into(),
            replacement_id: replacement.map(|d| d.id),
            resolved_at: outcome["resolvedAt"].as_u64().ok_or(Error::State)?,
        };
        receipt.record(&tx, &result)?;
        if !subject(&tx, &q, &check)?.1 {
            return Err(Error::LocalAuthority);
        }
        tx.commit().map_err(|_| Error::OutcomeUnknown)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inventory_pagination_bounds_encoded_bytes_for_saved_review() {
        let component="\"".repeat(203);
        let path=vec![component.as_str();20].join("/");
        let entries=(0..8).map(|_|json!({"path":path,"kind":"file","bytes":1,"sha256":"a".repeat(64)})).collect::<Vec<_>>();
        let observation=json!({"inventory":{"entries":entries}});
        let (_,page,next)=inventory(&observation,0).unwrap();
        assert!(serde_json::to_vec(&page).unwrap().len()<48*1024,"escaped paths must fit the native saved-result envelope");
        assert!(next.is_some_and(|n|n>0 && n<8));
        let (_,rest,last)=inventory(&observation,next.unwrap()).unwrap();
        assert_eq!(page.len()+rest.len(),8);assert_eq!(last,None);
    }
}
