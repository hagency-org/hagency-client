use super::*;
use hagency_store::management::{Intent, IntentOperation, IntentReply};

pub(in crate::console::native) enum PublicationObservation {
    Unconfirmed,
    Rejected,
    ServerObserved(Box<ResourceGrantView>),
}

impl AllocationTransport {
    /// Observe an existing command without preparing, retrying, acknowledging or
    /// recording it. A negative recovery response never authorizes publication.
    pub(in crate::console::native) async fn observe_publication(
        &self,
        registry: &OwnerResourceRegistry,
        command_id: &str,
        grant_id: &str,
        expected_contribution_revision: u64,
    ) -> Result<PublicationObservation, NativeError> {
        key(command_id)?;
        positive(expected_contribution_revision)?;
        let scope = self.scope(Permission::ReadIntents).await?;
        // Intent reads are filtered by the operation's permission. Require it
        // explicitly rather than mistaking a hidden receipt for an unknown one.
        scope
            .session
            .authorize(&scope.context, Permission::PublishGrants)
            .map_err(storage)?;
        let grant = self.original_grant(&scope, registry, grant_id).await?;
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
        if saved.operation != IntentOperation::PublishGrant {
            return Err(failure(409, "owner_grant_publication_history_unavailable"));
        }
        let input: Publication = wire::decode(saved.input)?;
        if input.command_id != command_id
            || input.expected_contribution_revision != expected_contribution_revision
            || digest(&input.receipt)? != digest(&grant)?
        {
            return Err(failure(409, "owner_grant_publication_history_unavailable"));
        }
        if matches!(saved.reply, Some(IntentReply::Failure { .. })) {
            return Ok(PublicationObservation::Rejected);
        }
        // This endpoint only reads the original server command and current grant.
        // In particular, do not call publish_original when recorded is false.
        let recovery: PublicationRecovery = self
            .request(
                &scope,
                Operation::RecoverPublication(command_id.into()),
                false,
            )
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

    pub(in crate::console::native) async fn review_publication(
        &self,
        registry: &OwnerResourceRegistry,
        grant_id: &str,
    ) -> Result<(u64, u64), NativeError> {
        let scope = self.scope(Permission::PublishGrants).await?;
        let grant = self.original_grant(&scope, registry, grant_id).await?;
        let (enrollment, revision, observed) =
            self.enrollment_observation(&scope, &grant, true).await?;
        self.require_publishable(&scope, registry, enrollment, grant)
            .await?;
        self.check_scope(&scope, false).await?;
        Ok((revision, observed))
    }
    pub(in crate::console::native) async fn ensure_unpublished_intent(
        &self,
        grant_id: &str,
    ) -> Result<(), NativeError> {
        let scope = self.scope(Permission::PublishGrants).await?;
        scope
            .session
            .require_unused_grant_publication(&scope.context, grant_id.into())
            .await
            .map_err(|error| {
                if matches!(error, hagency_store::Error::Conflict) {
                    failure(409, "owner_grant_publication_history_unavailable")
                } else {
                    storage(error)
                }
            })?;
        self.check_scope(&scope, false).await?;
        Ok(())
    }
    pub(super) async fn require_publishable(
        &self,
        scope: &Scope,
        registry: &OwnerResourceRegistry,
        enrollment: VerifiedManagementEnrollment,
        grant: NativeGrantReceipt,
    ) -> Result<(), NativeError> {
        self.check_scope(scope, false).await?;
        let result = registry
            .require_publishable_capacity_grant(enrollment, grant)
            .await;
        self.check_scope(scope, false).await?;
        result.map_err(|error| {
            if matches!(error, hagency_store::Error::Conflict) {
                failure(409, "native_grant_not_publishable")
            } else {
                storage(error)
            }
        })
    }
    /// A rejection is final only if the existing native journal recorded the
    /// audited first-attempt failure. A later transport error is never failure.
    pub(in crate::console::native) async fn publication_rejected(
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
            intent.operation == IntentOperation::PublishGrant
                && matches!(intent.reply, Some(IntentReply::Failure { .. }))
        }))
    }

    /// Publish only an existing owner-ledger receipt. Caller data cannot mint a
    /// resource, provider identity, grant or enrollment. No hold is recreated.
    pub async fn publish_grant(
        &self,
        registry: &OwnerResourceRegistry,
        command_id: String,
        grant_id: String,
        expected_contribution_revision: u64,
    ) -> Result<ResourceGrantView, NativeError> {
        let scope = self.scope(Permission::PublishGrants).await?;
        let grant = self.original_grant(&scope, registry, &grant_id).await?;
        self.publish_original(
            registry,
            Publication {
                command_id,
                expected_contribution_revision,
                receipt: grant,
            },
        )
        .await
    }
    /// Native custody recovery after restart; never changes the original CAS,
    /// receipt or command identity and never invents another parent hold.
    pub async fn retry_publication(
        &self,
        registry: &OwnerResourceRegistry,
        command_id: String,
    ) -> Result<ResourceGrantView, NativeError> {
        key(&command_id)?;
        let scope = self.scope(Permission::PublishGrants).await?;
        let saved = scope
            .session
            .intent(&scope.context, command_id.clone())
            .await
            .map_err(storage)?
            .filter(|p| p.operation == IntentOperation::PublishGrant)
            .ok_or_else(|| failure(404, "management_saved_command_not_found"))?;
        let input: Publication = wire::decode(saved.input)?;
        if input.command_id != command_id {
            return Err(invalid());
        }
        self.publish_original(registry, input).await
    }
    async fn publish_original(
        &self,
        registry: &OwnerResourceRegistry,
        input: Publication,
    ) -> Result<ResourceGrantView, NativeError> {
        let _lane = self.manager.owner.0.management.mutations.lock().await;
        let scope = self.scope(Permission::PublishGrants).await?;
        key(&input.command_id)?;
        positive(input.expected_contribution_revision)?;
        let grant = self
            .original_grant(&scope, registry, &input.receipt.grant_id)
            .await?;
        if digest(&grant)? != digest(&input.receipt)? {
            return Err(failure(409, "native_grant_receipt_conflict"));
        }
        let intent = Intent::new(
            IntentOperation::PublishGrant,
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
        // Always query original receipt on retry, even with a saved success:
        // current owner visibility and latest revoked state remain authoritative.
        let recovered = if saved.is_first_attempt() {
            None
        } else {
            let recovery: PublicationRecovery = self
                .request(
                    &scope,
                    Operation::RecoverPublication(input.command_id.clone()),
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
            // Validate current enrollment before the first publication/retry;
            // semantic conflict never causes a replacement local grant.
            let preflight = async {
                let enrollment = self.enrollment(&scope, &input.receipt, true).await?;
                self.require_publishable(&scope, registry, enrollment, input.receipt.clone())
                    .await
            }
            .await;
            if let Err(error) = preflight {
                // This exact mismatch is decided locally before the first POST.
                // Auth, unavailable and malformed-response errors remain pending.
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
            let result: Published =
                match self.request(&scope, Operation::Publish(input), true).await {
                    Ok(value) => value,
                    Err(error) => {
                        // Only an audited pre-effect reply on our first attempt can
                        // end custody. Earlier uncertain outcomes always keep the
                        // original command pending, even if a retry later rejects.
                        if saved.is_first_attempt() && definitely_not_published(&error.code) {
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
        // The journal keeps the first durable reply. Recovery returns the
        // latest server projection without overwriting that immutable receipt.
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

// Audited against the server grant.publish transaction. These checks precede
// insertion; post-commit reauthorization cannot emit them. Rejection never
// releases the native parent hold. Auth/conflict/transport failures are omitted.
fn definitely_not_published(code: &str) -> bool {
    matches!(
        code,
        "contribution_not_admitted"
            | "management_revision_changed"
            | "resource_grant_binding_mismatch"
            | "contribution_executor_changed"
            | "invalid_grant_period"
            | "resource_grant_conflict"
            | "resource_grant_already_exists"
    )
}
