//! Publication of a v3 successor receipt to `grants/{predecessor}/successors`.
//! It mirrors root publication: one durable intent per original command, the
//! original command is only ever retried, and only an audited pre-effect
//! rejection on the first attempt ends custody.
use super::*;
use hagency_store::management::{Intent, IntentOperation, IntentReply};

fn predecessor(grant: &NativeGrantReceipt) -> Result<&GrantSuccession, NativeError> {
    grant
        .succession
        .as_ref()
        .filter(|_| grant.version == 3 && grant.supported_shape())
        .ok_or_else(|| failure(409, "native_grant_receipt_required"))
}

impl AllocationTransport {
    /// Contribution revision, predecessor revision and observation time for a
    /// native successor publication review. The predecessor must still be
    /// the server's active grant with exactly the receipt the successor names.
    pub(in crate::console::native) async fn review_successor_publication(
        &self,
        registry: &OwnerResourceRegistry,
        grant_id: &str,
    ) -> Result<(u64, u64, u64), NativeError> {
        let scope = self.scope(Permission::PublishGrants).await?;
        let grant = self.original_grant(&scope, registry, grant_id).await?;
        let link = predecessor(&grant)?.clone();
        let (enrollment, revision, observed) =
            self.enrollment_observation(&scope, &grant, true).await?;
        self.require_publishable(&scope, registry, enrollment, grant.clone())
            .await?;
        let view = self.predecessor_view(&scope, &link).await?;
        if view.state != "active" {
            return Err(failure(409, "owner_grant_predecessor_required"));
        }
        self.check_scope(&scope, false).await?;
        Ok((revision, view.revision, observed))
    }

    async fn predecessor_view(
        &self,
        scope: &Scope,
        link: &GrantSuccession,
    ) -> Result<ResourceGrantView, NativeError> {
        let reply: GrantReply = self
            .request(scope, Operation::Grant(link.predecessor_grant_id.clone()), false)
            .await?;
        positive(reply.observed_at_ms)?;
        positive(reply.grant.revision)?;
        if reply.grant.id != link.predecessor_grant_id
            || reply.grant.receipt_digest != link.predecessor_receipt_digest
            || digest(&reply.grant.receipt)? != link.predecessor_receipt_digest
        {
            return Err(invalid());
        }
        Ok(reply.grant)
    }

    /// Observe an existing successor command without preparing, retrying or
    /// recording it, like `observe_publication`.
    pub(in crate::console::native) async fn observe_successor_publication(
        &self,
        registry: &OwnerResourceRegistry,
        command_id: &str,
        grant_id: &str,
        expected_contribution_revision: u64,
        expected_predecessor_revision: u64,
    ) -> Result<PublicationObservation, NativeError> {
        key(command_id)?;
        positive(expected_contribution_revision)?;
        positive(expected_predecessor_revision)?;
        let scope = self.scope(Permission::ReadIntents).await?;
        scope
            .session
            .authorize(&scope.context, Permission::PublishGrants)
            .map_err(storage)?;
        let grant = self.original_grant(&scope, registry, grant_id).await?;
        predecessor(&grant)?;
        // Exact lookup: a replied publication is kept history, not a pending
        // saved command, so it is not in the bounded pending read.
        let saved = scope
            .session
            .intent(&scope.context, command_id.to_owned())
            .await
            .map_err(storage)?;
        self.check_scope(&scope, false).await?;
        let Some(saved) = saved else {
            return Ok(PublicationObservation::Unconfirmed);
        };
        if saved.operation != IntentOperation::PublishGrantSuccessor {
            return Err(failure(409, "owner_grant_publication_history_unavailable"));
        }
        let input: SuccessorPublication = wire::decode(saved.input)?;
        if input.command_id != command_id
            || input.expected_contribution_revision != expected_contribution_revision
            || input.expected_predecessor_revision != expected_predecessor_revision
            || digest(&input.receipt)? != digest(&grant)?
        {
            return Err(failure(409, "owner_grant_publication_history_unavailable"));
        }
        if matches!(saved.reply, Some(IntentReply::Failure { .. })) {
            return Ok(PublicationObservation::Rejected);
        }
        let recovery: PublicationRecovery = self
            .request(&scope, Operation::RecoverSuccession(command_id.into()), false)
            .await?;
        positive(recovery.observed_at_ms)?;
        if recovery.recorded != recovery.grant.is_some() {
            return Err(invalid());
        }
        let Some(view) = recovery.grant else {
            if saved.reply.is_some() {
                return Err(failure(409, "grant_publication_receipt_missing"));
            }
            return Ok(PublicationObservation::Unconfirmed);
        };
        positive(view.revision)?;
        positive(view.observed_at_ms)?;
        if view.id != grant.grant_id
            || view.receipt_digest != digest(&grant)?
            || digest(&view.receipt)? != digest(&grant)?
        {
            return Err(invalid());
        }
        self.check_scope(&scope, false).await?;
        Ok(PublicationObservation::ServerObserved(Box::new(view)))
    }

    /// A rejection is final only if the native journal recorded the audited
    /// first-attempt failure of this exact successor command.
    pub(in crate::console::native) async fn successor_publication_rejected(
        &self,
        command_id: &str,
    ) -> Result<bool, NativeError> {
        let scope = self.scope(Permission::PublishGrants).await?;
        let saved = scope
            .session
            .intent(&scope.context, command_id.to_owned())
            .await
            .map_err(storage)?;
        self.check_scope(&scope, false).await?;
        Ok(saved.is_some_and(|intent| {
            intent.operation == IntentOperation::PublishGrantSuccessor
                && matches!(intent.reply, Some(IntentReply::Failure { .. }))
        }))
    }

    /// Publish an existing owner-ledger successor receipt. No hold is created.
    pub(in crate::console::native) async fn publish_successor(
        &self,
        registry: &OwnerResourceRegistry,
        command_id: String,
        grant_id: String,
        expected_contribution_revision: u64,
        expected_predecessor_revision: u64,
    ) -> Result<ResourceGrantView, NativeError> {
        let scope = self.scope(Permission::PublishGrants).await?;
        let grant = self.original_grant(&scope, registry, &grant_id).await?;
        predecessor(&grant)?;
        drop(scope);
        self.publish_successor_original(
            registry,
            SuccessorPublication {
                command_id,
                expected_contribution_revision,
                expected_predecessor_revision,
                receipt: grant,
            },
        )
        .await
    }

    async fn publish_successor_original(
        &self,
        registry: &OwnerResourceRegistry,
        input: SuccessorPublication,
    ) -> Result<ResourceGrantView, NativeError> {
        let _lane = self.manager.owner.0.management.mutations.lock().await;
        let scope = self.scope(Permission::PublishGrants).await?;
        key(&input.command_id)?;
        positive(input.expected_contribution_revision)?;
        positive(input.expected_predecessor_revision)?;
        let grant = self
            .original_grant(&scope, registry, &input.receipt.grant_id)
            .await?;
        if digest(&grant)? != digest(&input.receipt)? {
            return Err(failure(409, "native_grant_receipt_conflict"));
        }
        predecessor(&grant)?;
        let intent = Intent::new(
            IntentOperation::PublishGrantSuccessor,
            input.command_id.clone(),
            &input,
        )
        .map_err(storage)?;
        let intent = if self.require_unused_publication {
            intent.require_unused_grant_publication()
        } else {
            intent
        };
        let saved = scope
            .session
            .prepare_intent(&scope.context, intent.clone())
            .await
            .map_err(storage)?;
        if let Some(IntentReply::Failure { status, code }) = &saved.reply {
            self.check_scope(&scope, false).await?;
            return Err(failure(*status, code));
        }
        let recovered = if saved.is_first_attempt() {
            None
        } else {
            let recovery: PublicationRecovery = self
                .request(
                    &scope,
                    Operation::RecoverSuccession(input.command_id.clone()),
                    false,
                )
                .await?;
            positive(recovery.observed_at_ms)?;
            if recovery.recorded != recovery.grant.is_some() {
                return Err(invalid());
            }
            if saved.reply.is_some() && !recovery.recorded {
                return Err(failure(409, "grant_publication_receipt_missing"));
            }
            recovery.grant
        };
        let result = if let Some(grant) = recovered {
            grant
        } else {
            let preflight = async {
                let enrollment = self.enrollment(&scope, &input.receipt, true).await?;
                self.require_publishable(&scope, registry, enrollment, input.receipt.clone())
                    .await
            }
            .await;
            if let Err(error) = preflight {
                if saved.is_first_attempt()
                    && matches!(
                        error.code.as_str(),
                        "allocation_enrollment_changed" | "native_grant_not_publishable"
                    )
                {
                    scope
                        .session
                        .record_intent_reply(
                            &scope.context,
                            intent.clone(),
                            IntentReply::Failure {
                                status: error.status,
                                code: error.code.clone(),
                            },
                        )
                        .await
                        .map_err(storage)?;
                    self.check_scope(&scope, true).await?;
                }
                return Err(error);
            }
            let result: Published = match self
                .request(&scope, Operation::PublishSuccessor(input), true)
                .await
            {
                Ok(value) => value,
                Err(error) => {
                    if saved.is_first_attempt() && successor_not_published(&error.code) {
                        scope
                            .session
                            .record_intent_reply(
                                &scope.context,
                                intent.clone(),
                                IntentReply::Failure {
                                    status: error.status,
                                    code: error.code.clone(),
                                },
                            )
                            .await
                            .map_err(storage)?;
                        self.check_scope(&scope, true).await?;
                    }
                    return Err(error);
                }
            };
            positive(result.observed_at_ms)?;
            result.grant
        };
        positive(result.revision)?;
        positive(result.observed_at_ms)?;
        if result.id != grant.grant_id
            || result.receipt_digest != digest(&grant)?
            || digest(&result.receipt)? != digest(&grant)?
        {
            return Err(invalid());
        }
        self.check_scope(&scope, true).await?;
        if saved.reply.is_none() {
            scope
                .session
                .record_intent_reply(
                    &scope.context,
                    intent,
                    IntentReply::Success(serde_json::to_value(&result).map_err(|_| invalid())?),
                )
                .await
                .map_err(storage)?;
        }
        self.check_scope(&scope, true).await?;
        Ok(result)
    }
}

/// Audited against the server `grant.succeed` transaction: each is decided
/// before any row is written. Auth, transport and post-commit failures are
/// omitted, so the original command stays pending and is only ever retried.
pub(super) fn successor_not_published(code: &str) -> bool {
    matches!(
        code,
        "contribution_not_admitted"
            | "management_revision_changed"
            | "resource_grant_binding_mismatch"
            | "contribution_executor_changed"
            | "invalid_grant_period"
            | "invalid_grant_succession"
            | "grant_succession_window_closed"
            | "grant_predecessor_not_effective"
            | "grant_successor_exists"
            | "grant_successor_below_reserved"
            | "resource_grant_conflict"
            | "resource_grant_already_exists"
            | "pool_grant_succession_unavailable"
    )
}
