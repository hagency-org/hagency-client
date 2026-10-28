use super::*;

impl AllocationTransport {
    pub async fn poll_allocation(
        &self,
        registry: &OwnerResourceRegistry,
        after: Option<String>,
    ) -> Result<AllocationProgress, NativeError> {
        self.allocation(
            registry,
            Operation::Poll {
                parent: false,
                after,
            },
            None,
        )
        .await
    }
    pub async fn recover_allocation(
        &self,
        registry: &OwnerResourceRegistry,
        command_id: String,
        command_digest: String,
    ) -> Result<AllocationProgress, NativeError> {
        key(&command_id)?;
        hash(&command_digest)?;
        self.allocation(
            registry,
            Operation::Recover {
                parent: false,
                id: command_id.clone(),
                digest: command_digest.clone(),
            },
            Some((command_id, command_digest)),
        )
        .await
    }
    async fn allocation(
        &self,
        registry: &OwnerResourceRegistry,
        operation: Operation,
        original: Option<(String, String)>,
    ) -> Result<AllocationProgress, NativeError> {
        let scope = self.scope(Permission::ExecuteAllocations).await?;
        let started = Instant::now();
        let reply: Envelope<Delivery<AllocationCommand, NativeAllocationReceipt>> =
            self.request(&scope, operation, false).await?;
        let Some(delivery) = reply.delivery else {
            if original.is_some() {
                return Err(invalid());
            }
            return Ok(AllocationProgress::Idle);
        };
        let until = lease(
            &delivery,
            reply.observed_at_ms,
            started,
            &scope,
            original.is_some(),
        )?;
        let c = &delivery.command;
        if digest(c)? != delivery.command_digest
            || original
                .as_ref()
                .is_some_and(|(id, d)| id != &c.command_id || d != &delivery.command_digest)
            || !matches!(c.operation.as_str(), "project.reserve" | "project.release")
        {
            return Err(invalid());
        }
        let grant = self
            .original_grant(&scope, registry, &c.grant_receipt.grant_id)
            .await?;
        if digest(&grant)? != digest(&c.grant_receipt)? {
            return Err(failure(409, "native_grant_receipt_conflict"));
        }
        let old = registry
            .recover_project_allocation(
                scope.profile.clone(),
                scope.device.device_id().into(),
                c.command_id.clone(),
                delivery.command_digest.clone(),
            )
            .await
            .map_err(storage)?;
        let receipt = if let Some(receipt) = old {
            receipt
        } else {
            if delivery.recorded.is_some() {
                return Err(failure(409, "native_allocation_receipt_missing"));
            }
            if !delivery.execution_allowed {
                return Ok(AllocationProgress::HistoricalReceiptMissing {
                    command_id: c.command_id.clone(),
                });
            }
            // Only the poll path can reach this constructor. Recovery is a
            // historical upload ticket even if a faulty server claims otherwise.
            let enrollment = self
                .enrollment(&scope, &grant, c.operation == "project.reserve")
                .await?;
            self.check_scope(&scope, false).await?;
            let proof = VerifiedAllocationDelivery::new(
                enrollment,
                c.clone(),
                delivery.command_digest.clone(),
                true,
                until,
            )
            .map_err(storage)?;
            registry
                .apply_project_allocation(proof)
                .await
                .map_err(storage)?
        };
        self.check_scope(&scope, true).await?;
        if receipt.command_id != c.command_id || receipt.allocation_id != c.allocation_id {
            return Err(invalid());
        }
        let receipt_digest = digest(&receipt)?;
        if let Some(recorded) = delivery.recorded {
            if digest(&recorded)? != receipt_digest {
                return Err(failure(409, "allocation_receipt_conflict"));
            }
        } else {
            if Instant::now() >= until {
                return Err(failure(409, "allocation_ticket_expired"));
            }
            let ack: AllocationAck = self
                .upload(
                    &scope,
                    Operation::AllocationResult {
                        id: c.command_id.clone(),
                        digest: delivery.command_digest.clone(),
                        token: delivery.token,
                        receipt: receipt.clone(),
                    },
                    until,
                )
                .await?;
            positive(ack.observed_at_ms)?;
            if ack.command_id != c.command_id
                || ack.command_digest != delivery.command_digest
                || ack.receipt_digest != receipt_digest
                || ack.allocation.id != c.allocation_id
                || ack.allocation.project_id != c.project_id
                || ack.allocation.grant_id != grant.grant_id
            {
                return Err(failure(409, "allocation_acknowledgement_conflict"));
            }
        }
        self.check_scope(&scope, true).await?;
        Ok(AllocationProgress::Recorded {
            command_id: c.command_id.clone(),
            ledger_revision: receipt.ledger_revision,
        })
    }

    pub async fn poll_parent_settlement(
        &self,
        registry: &OwnerResourceRegistry,
        after: Option<String>,
    ) -> Result<AllocationProgress, NativeError> {
        self.parent(
            registry,
            Operation::Poll {
                parent: true,
                after,
            },
            None,
        )
        .await
    }
    pub async fn recover_parent_settlement(
        &self,
        registry: &OwnerResourceRegistry,
        command_id: String,
        command_digest: String,
    ) -> Result<AllocationProgress, NativeError> {
        key(&command_id)?;
        hash(&command_digest)?;
        self.parent(
            registry,
            Operation::Recover {
                parent: true,
                id: command_id.clone(),
                digest: command_digest.clone(),
            },
            Some((command_id, command_digest)),
        )
        .await
    }
    async fn parent(
        &self,
        registry: &OwnerResourceRegistry,
        operation: Operation,
        original: Option<(String, String)>,
    ) -> Result<AllocationProgress, NativeError> {
        let scope = self.scope(Permission::SettleGrants).await?;
        let started = Instant::now();
        let reply: Envelope<Delivery<GrantClosureCommand, NativeGrantSettlementReceipt>> =
            self.request(&scope, operation, false).await?;
        let Some(delivery) = reply.delivery else {
            if original.is_some() {
                return Err(invalid());
            }
            return Ok(AllocationProgress::Idle);
        };
        let until = lease(
            &delivery,
            reply.observed_at_ms,
            started,
            &scope,
            original.is_some(),
        )?;
        let c = &delivery.command;
        if digest(c)? != delivery.command_digest
            || original
                .as_ref()
                .is_some_and(|(id, d)| id != &c.command_id || d != &delivery.command_digest)
            || !matches!(c.operation.as_str(), "grant.close" | "grant.settle")
        {
            return Err(invalid());
        }
        let grant = self.original_grant(&scope, registry, &c.grant_id).await?;
        if digest(&grant)? != c.grant_receipt_digest {
            return Err(failure(409, "native_grant_receipt_conflict"));
        }
        let old = registry
            .recover_parent_settlement(
                scope.profile.clone(),
                scope.device.device_id().into(),
                c.command_id.clone(),
                delivery.command_digest.clone(),
            )
            .await
            .map_err(storage)?;
        let receipt = if let Some(receipt) = old {
            receipt
        } else {
            if delivery.recorded.is_some() {
                return Err(failure(409, "native_allocation_receipt_missing"));
            }
            if !delivery.execution_allowed {
                return Ok(AllocationProgress::HistoricalReceiptMissing {
                    command_id: c.command_id.clone(),
                });
            }
            let enrollment = self.enrollment(&scope, &grant, false).await?;
            self.check_scope(&scope, false).await?;
            let proof = VerifiedParentDelivery::new(
                enrollment,
                c.clone(),
                delivery.command_digest.clone(),
                true,
                until,
            )
            .map_err(storage)?;
            registry
                .apply_parent_settlement(proof)
                .await
                .map_err(storage)?
        };
        self.check_scope(&scope, true).await?;
        if receipt.command_id != c.command_id
            || receipt.command_digest != delivery.command_digest
            || receipt.grant_id != grant.grant_id
        {
            return Err(invalid());
        }
        let receipt_digest = digest(&receipt)?;
        if let Some(recorded) = delivery.recorded {
            if digest(&recorded)? != receipt_digest {
                return Err(failure(409, "allocation_receipt_conflict"));
            }
        } else {
            if Instant::now() >= until {
                return Err(failure(409, "allocation_ticket_expired"));
            }
            let ack: ParentAck = self
                .upload(
                    &scope,
                    Operation::ParentResult {
                        id: c.command_id.clone(),
                        digest: delivery.command_digest.clone(),
                        token: delivery.token,
                        receipt: receipt.clone(),
                    },
                    until,
                )
                .await?;
            positive(ack.observed_at_ms)?;
            if ack.command_id != c.command_id
                || ack.command_digest != delivery.command_digest
                || ack.receipt_digest != receipt_digest
                || ack.settlement.grant_id != grant.grant_id
            {
                return Err(failure(409, "allocation_acknowledgement_conflict"));
            }
        }
        self.check_scope(&scope, true).await?;
        Ok(AllocationProgress::Recorded {
            command_id: c.command_id.clone(),
            ledger_revision: receipt.ledger_revision,
        })
    }
}
