use super::*;
pub(super) fn read(
    db: &Connection,
    authority: &AdoptionAuthority,
    id: &str,
) -> Result<(Plan, i64), Error> {
    let (profile, digest, raw, phase): (String, String, String, i64) = db.query_row(
        "SELECT CASE WHEN length(profile)=64 THEN profile END,CASE WHEN length(plan_digest)=64 THEN plan_digest END,CASE WHEN length(CAST(plan AS BLOB))<=262144 THEN plan END,phase FROM owner_resource_adoptions WHERE id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    if profile != authority.profile.digest || raw.len() > 256 * 1024 {
        return Err(Error::LocalAuthority);
    }
    let plan: Plan = serde_json::from_str(&raw)?;
    if plan.migration_id != id
        || plan.registry_id != authority.registry.id()
        || plan.inventory.owner != authority.profile.account
        || plan.inventory.profile != profile
        || plan.digest()? != digest
    {
        return Err(Error::Quarantined);
    }
    if plan.version == 3 { alert_transfer::verify(&plan)?; }
    phase_name(phase)?;
    Ok((plan, phase))
}
fn conflicts(tx: &Connection, plan: &Plan) -> Result<(), Error> {
    alert_transfer::conflicts(tx, plan)?;
    if tx.query_row("SELECT EXISTS(SELECT 1 FROM owner_resource_accounts WHERE provider_digest=?1 OR seat_id=?2)",params![plan.provider_digest,plan.target_seat.id],|r|r.get::<_,bool>(0))? || tx.query_row("SELECT EXISTS(SELECT 1 FROM seats WHERE id=?1)",[&plan.target_seat.id],|r|r.get::<_,bool>(0))?{return Err(Error::Conflict);}
    for resource in &plan.inventory.resources {
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM resources WHERE id=?1)",
            [resource.id()],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(Error::Conflict);
        }
    }
    Ok(())
}
/// Read-only preflight before native initializer persistence. The actual writer
/// repeats these checks under IMMEDIATE custody; this never reserves a slot.
pub(super) fn prepare_preflight(
    db: &Connection,
    authority: &AdoptionAuthority,
    plan: &Plan,
) -> Result<(), Error> {
    authority.check()?;
    let existing: Option<String> = db
        .query_row(
            "SELECT id FROM owner_resource_adoptions WHERE profile=?1 OR id=?2",
            params![plan.inventory.profile, plan.migration_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        if id != plan.migration_id {
            return Err(Error::Conflict);
        }
        let (old, _) = read(db, authority, &id)?;
        if old.digest()? != plan.digest()? {
            return Err(Error::Conflict);
        }
        if matches!(old.version, 2 | 3) {
            task_transfer::verify(db, &old)?;
        }
        return authority.check();
    }
    conflicts(db, plan)?;
    authority.check()
}
impl DomainRepository {
    pub(super) fn finish_adoption(
        &mut self,
        authority: &AdoptionAuthority,
        proof: &VerifiedAdoptionJournal,
        provider: Option<&VerifiedProviderAccount>,
    ) -> Result<AdoptionSummary, Error> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        proof.check(authority)?;
        let (plan, phase) = read(&tx, authority, &proof.plan.migration_id)?;
        if phase < 2 || plan.digest()? != proof.plan.digest()? {
            return Err(Error::Conflict);
        }
        if let Some(provider) = provider {
            if Instant::now() >= provider.expires
                || provider.digest != plan.provider_digest
                || provider.runtime != plan.runtime
            {
                return Err(Error::LocalAuthority);
            }
            if phase < 3 {
                return Err(Error::Conflict);
            }
        }
        verify_import(&tx, &plan, phase)?;
        if phase >= 3 {
            let old: String = tx.query_row(
                "SELECT custody_evidence FROM owner_resource_adoptions WHERE id=?1",
                [&plan.migration_id],
                |r| r.get(0),
            )?;
            if old != proof.evidence {
                return Err(Error::Quarantined);
            }
        }
        if provider.is_none() && phase == 2 {
            tx.execute(
                "UPDATE owner_resource_adoptions SET phase=3,custody_evidence=?2 WHERE id=?1",
                params![plan.migration_id, proof.evidence],
            )?;
        } else if provider.is_some() && phase == 3 {
            for resource in &plan.inventory.resources {
                if tx.execute("UPDATE owner_resource_grants SET active=1,revision=revision+1 WHERE profile=?1 AND resource_id=?2 AND active=0",params![plan.inventory.profile,resource.id()])?!=1{return Err(Error::Quarantined);}
            }
            if tx.execute("UPDATE owner_resource_identity SET revision=revision+1 WHERE singleton=1 AND revision<?1",[JSON_SAFE_MAX])?!=1{return Err(Error::Capacity);}
            tx.execute(
                "UPDATE owner_resource_adoptions SET phase=4,result=?2 WHERE id=?1",
                params![plan.migration_id, serde_json::to_string(&plan.summary(4)?)?],
            )?;
        } else {
            proof.check(authority)?;
            return plan.summary(phase);
        }
        proof.check(authority)?;
        if provider.is_some_and(|p| Instant::now() >= p.expires) {
            return Err(Error::LocalAuthority);
        }
        tx.commit()?;
        proof.check(authority).map_err(|_| Error::OutcomeUnknown)?;
        plan.summary(if provider.is_some() { 4 } else { 3 })
    }
    pub(super) fn record_adoption_sources(
        &mut self,
        authority: &AdoptionAuthority,
        proof: &VerifiedAdoptionArchives,
    ) -> Result<AdoptionSummary, Error> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        proof.check(authority)?;
        let (plan, phase) = read(&tx, authority, &proof.plan.migration_id)?;
        if plan.digest()? != proof.plan.digest()? {
            return Err(Error::Conflict);
        }
        if phase >= 1 {
            let old: String = tx.query_row(
                "SELECT source_evidence FROM owner_resource_adoptions WHERE id=?1",
                [&plan.migration_id],
                |r| r.get(0),
            )?;
            if old != proof.evidence {
                return Err(Error::Quarantined);
            }
            proof.check(authority)?;
            return plan.summary(phase);
        }
        tx.execute(
            "UPDATE owner_resource_adoptions SET phase=1,source_evidence=?2 WHERE id=?1",
            params![plan.migration_id, proof.evidence],
        )?;
        proof.check(authority)?;
        tx.commit()?;
        proof.check(authority).map_err(|_| Error::OutcomeUnknown)?;
        plan.summary(1)
    }

    pub(super) fn import_resource_adoption(
        &mut self,
        authority: &AdoptionAuthority,
        proof: &VerifiedAdoptionArchives,
    ) -> Result<AdoptionSummary, Error> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        proof.check(authority)?;
        let (plan, phase) = read(&tx, authority, &proof.plan.migration_id)?;
        if phase == 0 || plan.digest()? != proof.plan.digest()? {
            return Err(Error::Conflict);
        }
        let old: String = tx.query_row(
            "SELECT source_evidence FROM owner_resource_adoptions WHERE id=?1",
            [&plan.migration_id],
            |r| r.get(0),
        )?;
        if old != proof.evidence {
            return Err(Error::Quarantined);
        }
        if phase >= 2 {
            verify_import(&tx, &plan, phase)?;
            proof.check(authority)?;
            return plan.summary(phase);
        }
        conflicts(&tx, &plan)?;
        task_transfer::import(&tx, &plan)?;
        alert_transfer::import(&tx, &plan)?;
        bounded_row(&tx, "seats", "id", &plan.target_seat.id, 2048)?;
        tx.execute(
            "INSERT INTO seats VALUES(?1,?2)",
            params![
                plan.target_seat.id,
                serde_json::to_string(&plan.target_seat)?
            ],
        )?;
        tx.execute(
            "INSERT INTO owner_resource_accounts VALUES(?1,?2,'verified_provider')",
            params![plan.provider_digest, plan.target_seat.id],
        )?;
        for resource in &plan.inventory.resources {
            // Original source IDs, definition bytes and ceilings are retained;
            // no reconstructed preset or summed provider capacity is created.
            bounded_row(&tx, "resources", "id", &resource.id(), 2048)?;
            tx.execute(
                "INSERT INTO resources(id,preset_id,config) VALUES(?1,?2,?3)",
                params![
                    resource.id(),
                    resource.preset_id,
                    serde_json::to_string(resource)?
                ],
            )?;
            tx.execute(
                "INSERT INTO owner_resource_grants VALUES(?1,?2,1,0)",
                params![plan.inventory.profile, resource.id()],
            )?;
        }
        if tx.execute("UPDATE owner_resource_identity SET revision=revision+1 WHERE singleton=1 AND revision<?1",[JSON_SAFE_MAX])? != 1 { return Err(Error::Capacity); }
        tx.execute(
            "UPDATE owner_resource_adoptions SET phase=2 WHERE id=?1",
            [&plan.migration_id],
        )?;
        verify_import(&tx, &plan, 2)?;
        proof.check(authority)?;
        tx.commit()?;
        proof.check(authority).map_err(|_| Error::OutcomeUnknown)?;
        plan.summary(2)
    }
    pub(super) fn prepare_adoption(
        &mut self,
        authority: &AdoptionAuthority,
        plan: &Plan,
        history: Option<&task_transfer::History>,
    ) -> Result<AdoptionSummary, Error> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        authority.check()?;
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM owner_resource_adoptions WHERE id=?1)",
            [&plan.migration_id],
            |r| r.get::<_, bool>(0),
        )? {
            let (old, phase) = read(&tx, authority, &plan.migration_id)?;
            if old.digest()? != plan.digest()? {
                return Err(Error::Conflict);
            }
            if matches!(old.version, 2 | 3) {
                task_transfer::verify(&tx, &old)?;
            }
            authority.check()?;
            return old.summary(phase);
        }
        conflicts(&tx, plan)?;
        bounded_row(
            &tx,
            "owner_resource_adoptions",
            "id",
            &plan.migration_id,
            128,
        )?;
        tx.execute("INSERT INTO owner_resource_adoptions(id,profile,plan_digest,plan,phase) VALUES(?1,?2,?3,?4,0)",params![plan.migration_id,plan.inventory.profile,plan.digest()?,serde_json::to_string(plan)?])?;
        task_transfer::save(&tx, plan, history)?;
        authority.check()?;
        tx.commit()?;
        authority.check().map_err(|_| Error::OutcomeUnknown)?;
        plan.summary(0)
    }
}

fn verify_import(db: &Connection, plan: &Plan, phase: i64) -> Result<(), Error> {
    task_transfer::verify_import(db, plan, phase)?;
    alert_transfer::verify_import(db, plan, phase)?;
    let seat:String=db.query_row("SELECT CASE WHEN length(CAST(config AS BLOB))<=16384 THEN config END FROM seats WHERE id=?1",[&plan.target_seat.id],|r|r.get(0))?;
    if serde_json::from_str::<Value>(&seat)? != serde_json::to_value(&plan.target_seat)? {
        return Err(Error::Quarantined);
    }
    let mapping:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM owner_resource_accounts WHERE provider_digest=?1 AND seat_id=?2 AND identity_kind='verified_provider')",params![plan.provider_digest,plan.target_seat.id],|r|r.get(0))?;
    if !mapping {
        return Err(Error::Quarantined);
    }
    for resource in &plan.inventory.resources {
        if serde_json::to_value(read_resource(db, &resource.id())?)?
            != serde_json::to_value(resource)?
        {
            return Err(Error::Quarantined);
        }
        let active: bool = db.query_row(
            "SELECT active FROM owner_resource_grants WHERE profile=?1 AND resource_id=?2",
            params![plan.inventory.profile, resource.id()],
            |r| r.get(0),
        )?;
        if active != (phase == 4) {
            return Err(Error::Quarantined);
        }
    }
    Ok(())
}
