//! Observe and fence the exact original optional sources. Files, backup custody
//! and the durable installation refusal are retained by the native authority;
//! this layer cannot create a missing source or select a path from JSON.
use super::*;
use crate::management::ArchiveJournalBinding;

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    version: u16,
    kind: String,
    migration_id: String,
    registry_id: String,
    profile_digest: String,
    owner_mxid: String,
    inventory_digest: String,
    source_schema: i32,
}
fn manifest(plan: &EmptyPlan, kind: &str) -> Manifest {
    Manifest {
        version: 1,
        kind: kind.into(),
        migration_id: plan.initialization_id.clone(),
        registry_id: plan.registry_id.clone(),
        profile_digest: plan.inventory.profile.clone(),
        owner_mxid: plan.inventory.owner.clone(),
        inventory_digest: plan.inventory_digest.clone(),
        source_schema: plan.inventory.source_schema,
    }
}
pub(in super::super) fn source_evidence(plan: &EmptyPlan) -> Result<String, Error> {
    Ok(canonical::digest(&json!({"plan":plan.digest()?,
        "management":manifest(plan,"management"),
        "personal":plan.inventory.personal_present.then(||manifest(plan,"personal"))}))?)
}
fn marker(db: &Connection, plan: &EmptyPlan, kind: &str) -> Result<(), Error> {
    let table = if kind == "management" {
        "management_archive_identity"
    } else {
        "personal_archive_identity"
    };
    let raw:String=db.query_row(&format!("SELECT CASE WHEN length(CAST(manifest AS BLOB))<=4096 THEN manifest END FROM {table} WHERE singleton=1"),[],|r|r.get(0))?;
    if serde_json::from_str::<Manifest>(&raw)? != manifest(plan, kind) {
        return Err(Error::Quarantined);
    }
    Ok(())
}
pub struct EmptyDeviceTransfer {
    pub(super) authority: AdoptionAuthority,
    pub(super) plan: EmptyPlan,
}
pub struct VerifiedEmptyDeviceArchives {
    pub(super) authority: AdoptionAuthority,
    pub(super) plan: EmptyPlan,
    pub(super) evidence: String,
}
impl EmptyDeviceTransfer {
    pub fn initialization_id(&self) -> &str {
        &self.plan.initialization_id
    }
    pub fn source_binding(&self) -> &str {
        &self.plan.source_binding
    }
    pub fn inventory_digest(&self) -> &str {
        &self.plan.inventory_digest
    }
    pub fn plan_digest(&self) -> Result<String, Error> {
        self.plan.digest()
    }
    pub fn personal_present(&self) -> bool {
        self.plan.inventory.personal_present
    }
    pub fn completed_private_records(&self) -> u32 {
        self.plan.inventory.completed_private_records
    }
    pub async fn summary(&self) -> Result<EmptyDeviceInitializationSummary, Error> {
        self.authority.check()?;
        let authority = self.authority.clone();
        let original = self.plan.clone();
        let result = self
            .authority
            .registry
            .0
            .store
            .call(16384, move |db| {
                let (plan, phase) = read_state(&db.db, &authority, &original.initialization_id)?;
                if plan.digest()? != original.digest()? {
                    return Err(Error::Quarantined);
                }
                let summary = plan.summary_at(phase)?;
                authority.check()?;
                Ok(summary)
            })
            .await?;
        self.authority.check()?;
        Ok(result)
    }
    pub fn archive_binding(&self) -> Result<ArchiveJournalBinding, Error> {
        self.authority.check()?;
        ArchiveJournalBinding::new(
            self.plan.initialization_id.clone(),
            self.plan.registry_id.clone(),
            self.plan.inventory.profile.clone(),
            self.plan.inventory.owner.clone(),
            self.plan.inventory_digest.clone(),
            self.plan.inventory.source_schema,
        )
    }
    /// Accept original state or an interrupted partial fence only if the full
    /// retained inventory and any already-written markers still match the plan.
    pub fn inspect_source_state(
        &self,
        domain: &mut Connection,
        mut personal: Option<&mut Connection>,
        binding: &str,
    ) -> Result<(), Error> {
        self.authority.check()?;
        if binding != self.plan.source_binding
            || personal.is_some() != self.plan.inventory.personal_present
            || !domain.is_autocommit()
            || personal.as_ref().is_some_and(|p| !p.is_autocommit())
        {
            return Err(Error::LocalAuthority);
        }
        let deadline = self
            .authority
            .expires
            .min(Instant::now() + Duration::from_secs(2));
        domain.progress_handler(1000, Some(move || Instant::now() >= deadline))?;
        if let Some(p) = personal.as_mut() {
            p.progress_handler(1000, Some(move || Instant::now() >= deadline))?;
        }
        let result = (|| {
            let d_archive = domain
                .pragma_query_value(None, "application_id", |r| r.get::<_, i32>(0))?
                == 0x48415231;
            let p_archive = personal
                .as_ref()
                .map(|p| p.pragma_query_value(None, "application_id", |r| r.get::<_, i32>(0)))
                .transpose()?
                .is_some_and(|v| v == 0x48415231);
            let inventory = empty_device::collect(
                domain,
                personal.as_deref_mut(),
                &self.plan.inventory.profile,
                &self.plan.inventory.owner,
                deadline,
                (d_archive, p_archive),
            )?;
            if canonical::digest(&serde_json::to_value(inventory)?)? != self.plan.inventory_digest {
                return Err(Error::Quarantined);
            }
            if d_archive {
                marker(domain, &self.plan, "management")?;
            }
            if let Some(p) = personal.as_ref().filter(|_| p_archive) {
                marker(p, &self.plan, "personal")?;
            }
            self.authority.check()?;
            if Instant::now() >= deadline {
                return Err(Error::Busy);
            }
            Ok(())
        })();
        domain.progress_handler(0, None::<fn() -> bool>)?;
        if let Some(p) = personal {
            p.progress_handler(0, None::<fn() -> bool>)?;
        }
        result
    }
    /// Native callers must first sync backups and durable legacy refusal while
    /// retaining source writer locks. This method never opens or creates files.
    pub fn fence_sources(
        &self,
        domain: &mut Connection,
        mut personal: Option<&mut Connection>,
        binding: &str,
    ) -> Result<(), Error> {
        if domain.is_readonly("main")?
            || personal
                .as_ref()
                .is_some_and(|p| p.is_readonly("main").unwrap_or(true))
        {
            return Err(Error::LocalAuthority);
        }
        self.inspect_source_state(domain, personal.as_deref_mut(), binding)?;
        self.fence_one(domain, "management")?;
        if let Some(p) = personal.as_mut() {
            self.fence_one(p, "personal")?;
        }
        self.inspect_source_state(domain, personal, binding)
    }
    fn write_marker(&self, db: &Connection, kind: &str) -> Result<(), Error> {
        let app = db.pragma_query_value(None, "application_id", |r| r.get::<_, i32>(0))?;
        if app == 0x48415231 {
            return marker(db, &self.plan, kind);
        }
        let (expected, sql, table) = if kind == "management" {
            (
                0x48414732,
                archives::DOMAIN_ARCHIVE_SCHEMA,
                "management_archive_identity",
            )
        } else {
            (
                0x48414c32,
                archives::PERSONAL_ARCHIVE_SCHEMA,
                "personal_archive_identity",
            )
        };
        if app != expected {
            return Err(Error::Schema);
        }
        db.execute_batch(sql)?;
        db.execute(
            &format!("INSERT INTO {table} VALUES(1,?1)"),
            [serde_json::to_string(&manifest(&self.plan, kind))?],
        )?;
        db.pragma_update(None, "application_id", 0x48415231)?;
        Ok(())
    }
    fn fence_one(&self, db: &mut Connection, kind: &str) -> Result<(), Error> {
        self.authority.check()?;
        let deadline = self
            .authority
            .expires
            .min(Instant::now() + Duration::from_secs(2));
        db.progress_handler(1000, Some(move || Instant::now() >= deadline))?;
        let result = (|| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            self.authority.check()?;
            self.write_marker(&tx, kind)?;
            self.authority.check()?;
            if Instant::now() >= deadline {
                return Err(Error::Busy);
            }
            tx.commit()?;
            self.authority.check().map_err(|_| Error::OutcomeUnknown)
        })();
        db.progress_handler(0, None::<fn() -> bool>)?;
        result
    }
    /// Finish the original personal BEGIN IMMEDIATE without releasing and
    /// reacquiring its writer lock. The native caller validated both snapshots
    /// and backup/refusal custody first; rollback on failure stays with it.
    pub fn fence_locked_personal(&self, personal: &Connection) -> Result<(), Error> {
        self.authority.check()?;
        if !self.plan.inventory.personal_present
            || personal.is_autocommit()
            || personal.is_readonly("main")?
        {
            return Err(Error::LocalAuthority);
        }
        let deadline = self
            .authority
            .expires
            .min(Instant::now() + Duration::from_secs(2));
        personal.progress_handler(1000, Some(move || Instant::now() >= deadline))?;
        let result = (|| {
            self.write_marker(personal, "personal")?;
            self.authority.check()?;
            if Instant::now() >= deadline {
                return Err(Error::Busy);
            }
            personal.execute_batch("COMMIT")?;
            self.authority.check().map_err(|_| Error::OutcomeUnknown)
        })();
        personal.progress_handler(0, None::<fn() -> bool>)?;
        result
    }
    /// Reopen the same activated sources without rechecking the original
    /// mutable private-command snapshot. Canonical active proof and the actual
    /// retained journal are checked separately by adopted_management.
    #[cfg(feature = "resource-adoption")]
    pub fn inspect_active_archives(
        &self,
        domain: &mut Connection,
        mut personal: Option<&mut Connection>,
        binding: &str,
    ) -> Result<(), Error> {
        self.authority.check()?;
        if binding != self.plan.source_binding
            || personal.is_some() != self.plan.inventory.personal_present
            || !domain.is_readonly("main")?
            || !domain.is_autocommit()
            || personal
                .as_ref()
                .is_some_and(|p| !p.is_readonly("main").unwrap_or(false) || !p.is_autocommit())
        {
            return Err(Error::LocalAuthority);
        }
        let deadline = self
            .authority
            .expires
            .min(Instant::now() + Duration::from_secs(2));
        domain.progress_handler(1000, Some(move || Instant::now() >= deadline))?;
        if let Some(p) = personal.as_mut() {
            p.progress_handler(1000, Some(move || Instant::now() >= deadline))?;
        }
        let result = (|| {
            let d = domain.transaction_with_behavior(TransactionBehavior::Deferred)?;
            if d.pragma_query_value(None, "application_id", |r| r.get::<_, i32>(0))? != 0x48415231
                || d.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))?
                    != self.plan.inventory.source_schema
            {
                return Err(Error::Schema);
            }
            schema::validate_domain(&d, self.plan.inventory.source_schema, deadline, true)?;
            marker(&d, &self.plan, "management")?;
            if d.query_row(
                "SELECT profile_digest FROM management_identity WHERE singleton=1",
                [],
                |r| r.get::<_, String>(0),
            )? != self.plan.inventory.profile
            {
                return Err(Error::LocalAuthority);
            }
            if let Some(p) = personal.as_mut() {
                let p = p.transaction_with_behavior(TransactionBehavior::Deferred)?;
                if p.pragma_query_value(None, "application_id", |r| r.get::<_, i32>(0))?
                    != 0x48415231
                    || p.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))? != 1
                {
                    return Err(Error::Schema);
                }
                schema::validate_personal(&p, deadline, true)?;
                marker(&p, &self.plan, "personal")?;
                if p.query_row(
                    "SELECT digest FROM profile_identity WHERE singleton=1",
                    [],
                    |r| r.get::<_, String>(0),
                )? != self.plan.inventory.profile
                    || p.query_row("SELECT owner FROM identity", [], |r| r.get::<_, String>(0))?
                        != self.plan.inventory.owner
                {
                    return Err(Error::LocalAuthority);
                }
                p.commit()?;
            }
            self.authority.check()?;
            if Instant::now() >= deadline {
                return Err(Error::Busy);
            }
            d.commit()?;
            Ok(())
        })();
        domain.progress_handler(0, None::<fn() -> bool>)?;
        if let Some(p) = personal {
            p.progress_handler(0, None::<fn() -> bool>)?;
        }
        result
    }
    pub fn inspect_archives(
        &self,
        domain: &mut Connection,
        mut personal: Option<&mut Connection>,
    ) -> Result<VerifiedEmptyDeviceArchives, Error> {
        if !domain.is_readonly("main")?
            || personal
                .as_ref()
                .is_some_and(|p| !p.is_readonly("main").unwrap_or(false))
        {
            return Err(Error::LocalAuthority);
        }
        self.inspect_source_state(domain, personal.as_deref_mut(), &self.plan.source_binding)?;
        if domain.pragma_query_value(None, "application_id", |r| r.get::<_, i32>(0))? != 0x48415231
            || personal
                .as_ref()
                .map(|p| p.pragma_query_value(None, "application_id", |r| r.get::<_, i32>(0)))
                .transpose()?
                .is_some_and(|v| v != 0x48415231)
        {
            return Err(Error::Schema);
        }
        self.authority.check()?;
        Ok(VerifiedEmptyDeviceArchives {
            authority: self.authority.clone(),
            plan: self.plan.clone(),
            evidence: source_evidence(&self.plan)?,
        })
    }
}
impl OwnerResourceRegistry {
    pub async fn empty_device_transfer(
        &self,
        authority: AdoptionAuthority,
        id: String,
        expected_digest: String,
    ) -> Result<EmptyDeviceTransfer, Error> {
        self.find_empty_device_transfer(authority, id, expected_digest)
            .await?
            .ok_or(Error::Conflict)
    }
    /// Only an actually absent original row returns None. Changed scope,
    /// corruption or a conflicting plan cannot authorize fresh initialization.
    pub async fn find_empty_device_transfer(
        &self,
        authority: AdoptionAuthority,
        id: String,
        expected_digest: String,
    ) -> Result<Option<EmptyDeviceTransfer>, Error> {
        authority.check()?;
        identifier(&id, 128)?;
        hash(&expected_digest)?;
        if !Arc::ptr_eq(&self.0, &authority.registry.0) {
            return Err(Error::LocalAuthority);
        }
        let check = authority.clone();
        let plan = self
            .0
            .store
            .call(16384, move |db| {
                check.check()?;
                let tx = db.db.unchecked_transaction()?;
                if !tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM owner_resource_adoptions WHERE id=?1)",
                    [&id],
                    |r| r.get::<_, bool>(0),
                )? {
                    check.check()?;
                    tx.commit()?;
                    return Ok(None);
                }
                let (plan, _) = read_state(&tx, &check, &id)?;
                if plan.digest()? != expected_digest {
                    return Err(Error::Conflict);
                }
                check.check()?;
                tx.commit()?;
                Ok(Some(plan))
            })
            .await?;
        authority.check()?;
        Ok(plan.map(|plan| EmptyDeviceTransfer { authority, plan }))
    }
    pub async fn record_empty_device_sources(
        &self,
        proof: VerifiedEmptyDeviceArchives,
    ) -> Result<EmptyDeviceInitializationSummary, Error> {
        proof.authority.check()?;
        if !Arc::ptr_eq(&self.0, &proof.authority.registry.0) {
            return Err(Error::LocalAuthority);
        }
        let after = proof.authority.clone();
        let summary=self.0.store.call(16384,move|db| {
            let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            proof.authority.check()?;
            let (plan,phase)=read_state(&tx,&proof.authority,&proof.plan.initialization_id)?;
            if plan.digest()?!=proof.plan.digest()? || proof.evidence!=source_evidence(&plan)? {return Err(Error::Quarantined);}
            if phase==0 {
                tx.execute("UPDATE owner_resource_adoptions SET phase=1,source_evidence=?2 WHERE id=?1",params![plan.initialization_id,proof.evidence])?;
            }
            proof.authority.check()?;
            tx.commit()?;
            proof.authority.check().map_err(|_|Error::OutcomeUnknown)?;
            plan.summary_at(phase.max(1))
        }).await?;
        after.check().map_err(|_| Error::OutcomeUnknown)?;
        Ok(summary)
    }
}

#[cfg(test)]
pub(super) mod tests;
