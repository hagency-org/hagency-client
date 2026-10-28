//! A source fence is observed from exact source rows, never asserted by a JSON receipt.
use super::*;
pub(super) const DOMAIN_ARCHIVE_SCHEMA: &str = "CREATE TABLE management_archive_identity(singleton INTEGER PRIMARY KEY CHECK(singleton=1),manifest TEXT NOT NULL); CREATE TRIGGER management_archive_identity_retained BEFORE DELETE ON management_archive_identity BEGIN SELECT RAISE(ABORT,'retain original archive identity'); END; CREATE TRIGGER management_archive_identity_immutable BEFORE UPDATE ON management_archive_identity BEGIN SELECT RAISE(ABORT,'immutable archive identity'); END;";
pub(super) const PERSONAL_ARCHIVE_SCHEMA: &str = "CREATE TABLE personal_archive_identity(singleton INTEGER PRIMARY KEY CHECK(singleton=1),manifest TEXT NOT NULL); CREATE TRIGGER personal_archive_identity_retained BEFORE DELETE ON personal_archive_identity BEGIN SELECT RAISE(ABORT,'retain original archive identity'); END; CREATE TRIGGER personal_archive_identity_immutable BEFORE UPDATE ON personal_archive_identity BEGIN SELECT RAISE(ABORT,'immutable archive identity'); END;";

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
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
fn manifest(plan: &Plan, kind: &str) -> Manifest {
    Manifest {
        version: 1,
        kind: kind.into(),
        migration_id: plan.migration_id.clone(),
        registry_id: plan.registry_id.clone(),
        profile_digest: plan.inventory.profile.clone(),
        owner_mxid: plan.inventory.owner.clone(),
        inventory_digest: plan.inventory_digest.clone(),
        source_schema: plan.inventory.source_schema,
    }
}
/// Original canonical plan and finite native source custody. Not deserializable.
/// Merely obtaining this handle does not fence, copy or activate a source.
pub struct AdoptionTransfer {
    pub(super) authority: AdoptionAuthority,
    pub(super) plan: Plan,
}
/// Verified archive rows plus the independently retained native source gate.
/// No public constructor accepts serialized evidence or caller-asserted flags.
pub struct VerifiedAdoptionArchives {
    pub(super) authority: AdoptionAuthority,
    pub(super) plan: Plan,
    pub(super) evidence: String,
}
impl VerifiedAdoptionArchives {
    pub(super) fn check(&self, current: &AdoptionAuthority) -> Result<(), Error> {
        self.authority.check()?;
        current.check()?;
        if !Arc::ptr_eq(&self.authority.registry.0, &current.registry.0)
            || self.authority.profile.digest != current.profile.digest
            || self.authority.profile.account != current.profile.account
        {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
}
impl AdoptionTransfer {
    pub fn plan_digest(&self) -> Result<String, Error> {
        self.plan.digest()
    }
    pub fn includes_operator_tasks(&self) -> bool {
        matches!(self.plan.version, 2 | 3)
    }
    pub fn migration_id(&self) -> &str {
        &self.plan.migration_id
    }
    pub fn inventory_digest(&self) -> &str {
        &self.plan.inventory_digest
    }
    pub fn source_binding(&self) -> &str {
        &self.plan.source_binding
    }
    /// Reopen an activated archive without reinterpreting its now mutable
    /// private command history as the original pre-import snapshot. This
    /// validates only exact schema and permanent source provenance; the active
    /// canonical plan and actual journal worker are checked separately.
    #[cfg(feature = "resource-adoption")]
    pub fn inspect_active_archives(
        &self,
        domain: &mut Connection,
        personal: &mut Connection,
        binding: &str,
    ) -> Result<(), Error> {
        self.authority.check()?;
        if binding != self.plan.source_binding
            || !domain.is_readonly("main")?
            || !personal.is_readonly("main")?
            || !domain.is_autocommit()
            || !personal.is_autocommit()
        {
            return Err(Error::LocalAuthority);
        }
        let deadline = self
            .authority
            .expires
            .min(Instant::now() + Duration::from_secs(2));
        domain.progress_handler(1000, Some(move || Instant::now() >= deadline))?;
        personal.progress_handler(1000, Some(move || Instant::now() >= deadline))?;
        let result = (|| {
            let d = domain.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let p = personal.transaction_with_behavior(TransactionBehavior::Deferred)?;
            schema::validate(
                &d,
                &p,
                self.plan.inventory.source_schema,
                deadline,
                (true, true),
            )?;
            for (db, table, kind) in [
                (&d, "management_archive_identity", "management"),
                (&p, "personal_archive_identity", "personal"),
            ] {
                if db.pragma_query_value(None, "application_id", |r| r.get::<_, i32>(0))?
                    != 0x48415231
                {
                    return Err(Error::Schema);
                }
                let raw:String=db.query_row(&format!("SELECT CASE WHEN length(CAST(manifest AS BLOB))<=4096 THEN manifest END FROM {table} WHERE singleton=1"),[],|r|r.get(0))?;
                if serde_json::from_str::<Manifest>(&raw)? != manifest(&self.plan, kind) {
                    return Err(Error::Quarantined);
                }
            }
            self.authority.check()?;
            if Instant::now() >= deadline {
                return Err(Error::Busy);
            }
            d.commit()?;
            p.commit()?;
            Ok(())
        })();
        domain.progress_handler(0, None::<fn() -> bool>)?;
        personal.progress_handler(0, None::<fn() -> bool>)?;
        result
    }
    /// Revalidates either original sources or an interrupted partial fence.
    /// Physical source locks, original backup and installation exclusion remain
    /// requirements of the native authority; this does not infer their presence.
    pub fn inspect_source_state(
        &self,
        domain: &mut Connection,
        personal: &mut Connection,
        source_binding: &str,
    ) -> Result<(), Error> {
        self.authority.check()?;
        if source_binding != self.plan.source_binding
            || !domain.is_autocommit()
            || !personal.is_autocommit()
        {
            return Err(Error::LocalAuthority);
        }
        let deadline = self
            .authority
            .expires
            .min(Instant::now() + Duration::from_secs(2));
        domain.progress_handler(1000, Some(move || Instant::now() >= deadline))?;
        personal.progress_handler(1000, Some(move || Instant::now() >= deadline))?;
        let result = (|| {
            let archived = (
                domain.pragma_query_value(None, "application_id", |r| r.get::<_, i32>(0))?
                    == 0x48415231,
                personal.pragma_query_value(None, "application_id", |r| r.get::<_, i32>(0))?
                    == 0x48415231,
            );
            let current =
                inventory::collect_for_plan(domain, personal, &self.plan, deadline, archived)?;
            if canonical::digest(&serde_json::to_value(current)?)? != self.plan.inventory_digest {
                return Err(Error::Quarantined);
            }
            for (db, is_archive, table, kind) in [
                (
                    &*domain,
                    archived.0,
                    "management_archive_identity",
                    "management",
                ),
                (
                    &*personal,
                    archived.1,
                    "personal_archive_identity",
                    "personal",
                ),
            ] {
                if is_archive {
                    let raw: String = db.query_row(&format!("SELECT CASE WHEN length(CAST(manifest AS BLOB))<=4096 THEN manifest END FROM {table} WHERE singleton=1"), [], |r| r.get(0))?;
                    if serde_json::from_str::<Manifest>(&raw)? != manifest(&self.plan, kind) {
                        return Err(Error::Quarantined);
                    }
                }
            }
            self.authority.check()?;
            if Instant::now() >= deadline {
                return Err(Error::Busy);
            }
            Ok(())
        })();
        domain.progress_handler(0, None::<fn() -> bool>)?;
        personal.progress_handler(0, None::<fn() -> bool>)?;
        result
    }
    /// Fence the personal source while its original native BEGIN IMMEDIATE
    /// writer custody is still held. The caller previously verified both exact
    /// source snapshots and persisted the backup/refusal. Never starts a new
    /// transaction or opens a path; errors leave rollback to that same custodian.
    #[cfg(feature = "resource-adoption")]
    pub fn fence_locked_personal(&self, personal: &Connection) -> Result<(), Error> {
        self.authority.check()?;
        if personal.is_readonly("main")? || personal.is_autocommit() {
            return Err(Error::LocalAuthority);
        }
        let deadline = self
            .authority
            .expires
            .min(Instant::now() + Duration::from_secs(2));
        personal.progress_handler(1000, Some(move || Instant::now() >= deadline))?;
        let result = (|| {
            let app: i32 = personal.pragma_query_value(None, "application_id", |r| r.get(0))?;
            if app == 0x48414c32 {
                personal.execute_batch(PERSONAL_ARCHIVE_SCHEMA)?;
                personal.execute(
                    "INSERT INTO personal_archive_identity VALUES(1,?1)",
                    [serde_json::to_string(&manifest(&self.plan, "personal"))?],
                )?;
                personal.pragma_update(None, "application_id", 0x48415231)?;
            } else if app == 0x48415231 {
                let raw:String=personal.query_row("SELECT CASE WHEN length(CAST(manifest AS BLOB))<=4096 THEN manifest END FROM personal_archive_identity WHERE singleton=1",[],|r|r.get(0))?;
                if serde_json::from_str::<Manifest>(&raw)? != manifest(&self.plan, "personal") {
                    return Err(Error::Quarantined);
                }
            } else {
                return Err(Error::Schema);
            }
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
    /// Closed, default-disabled source fence. The native caller must already
    /// hold source locks, a synced original backup and the permanent installation
    /// refusal. A partial success is retained and revalidated on exact retry.
    #[cfg(feature = "resource-adoption")]
    pub fn fence_sources(
        &self,
        domain: &mut Connection,
        personal: &mut Connection,
        source_binding: &str,
    ) -> Result<(), Error> {
        if domain.is_readonly("main")? || personal.is_readonly("main")? {
            return Err(Error::LocalAuthority);
        }
        self.inspect_source_state(domain, personal, source_binding)?;
        for (db, schema, table, kind) in [
            (
                &mut *domain,
                DOMAIN_ARCHIVE_SCHEMA,
                "management_archive_identity",
                "management",
            ),
            (
                &mut *personal,
                PERSONAL_ARCHIVE_SCHEMA,
                "personal_archive_identity",
                "personal",
            ),
        ] {
            self.authority.check()?;
            let deadline = self
                .authority
                .expires
                .min(Instant::now() + Duration::from_secs(2));
            db.progress_handler(1000, Some(move || Instant::now() >= deadline))?;
            let result = (|| {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                self.authority.check()?;
                let app: i32 = tx.pragma_query_value(None, "application_id", |r| r.get(0))?;
                if app != 0x48415231 {
                    tx.execute_batch(schema)?;
                    tx.execute(
                        &format!("INSERT INTO {table} VALUES(1,?1)"),
                        [serde_json::to_string(&manifest(&self.plan, kind))?],
                    )?;
                    tx.pragma_update(None, "application_id", 0x48415231)?;
                } else {
                    let raw: String = tx.query_row(
                        &format!("SELECT manifest FROM {table} WHERE singleton=1"),
                        [],
                        |r| r.get(0),
                    )?;
                    if serde_json::from_str::<Manifest>(&raw)? != manifest(&self.plan, kind) {
                        return Err(Error::Quarantined);
                    }
                }
                self.authority.check()?;
                if Instant::now() >= deadline {
                    return Err(Error::Busy);
                }
                tx.commit()?;
                self.authority.check().map_err(|_| Error::OutcomeUnknown)
            })();
            db.progress_handler(0, None::<fn() -> bool>)?;
            result?;
        }
        self.inspect_source_state(domain, personal, source_binding)
    }
    /// The host must retain physical source locks and previously synced backup
    /// custody through its authority gate. These connections must be to the
    /// same sources; only their read-only archive state is examined here.
    pub fn inspect_archives(
        &self,
        domain: &mut Connection,
        personal: &mut Connection,
    ) -> Result<VerifiedAdoptionArchives, Error> {
        self.authority.check()?;
        if !domain.is_readonly("main")?
            || !personal.is_readonly("main")?
            || !domain.is_autocommit()
            || !personal.is_autocommit()
        {
            return Err(Error::LocalAuthority);
        }
        let deadline = self
            .authority
            .expires
            .min(Instant::now() + Duration::from_secs(2));
        domain.progress_handler(1000, Some(move || Instant::now() >= deadline))?;
        personal.progress_handler(1000, Some(move || Instant::now() >= deadline))?;
        let result = (|| {
            let current =
                inventory::collect_for_plan(domain, personal, &self.plan, deadline, (true, true))?;
            if canonical::digest(&serde_json::to_value(current)?)? != self.plan.inventory_digest {
                return Err(Error::Quarantined);
            }
            for (db, sql, kind) in [
                (
                    &*domain,
                    "SELECT CASE WHEN length(CAST(manifest AS BLOB))<=4096 THEN manifest END FROM management_archive_identity WHERE singleton=1",
                    "management",
                ),
                (
                    &*personal,
                    "SELECT CASE WHEN length(CAST(manifest AS BLOB))<=4096 THEN manifest END FROM personal_archive_identity WHERE singleton=1",
                    "personal",
                ),
            ] {
                let raw: String = db.query_row(sql, [], |r| r.get(0))?;
                if serde_json::from_str::<Manifest>(&raw)? != manifest(&self.plan, kind) {
                    return Err(Error::Quarantined);
                }
            }
            self.authority.check()?;
            if Instant::now() >= deadline {
                return Err(Error::Busy);
            }
            Ok(VerifiedAdoptionArchives {
                authority: self.authority.clone(),
                plan: self.plan.clone(),
                evidence: canonical::digest(
                    &json!({"plan":self.plan.digest()?,"management":manifest(&self.plan,"management"),"personal":manifest(&self.plan,"personal")}),
                )?,
            })
        })();
        domain.progress_handler(0, None::<fn() -> bool>)?;
        personal.progress_handler(0, None::<fn() -> bool>)?;
        result
    }
    #[cfg(all(test, feature = "resource-adoption"))]
    pub(super) fn fixture_fence(&self, domain: &Connection, personal: &Connection) {
        for (db, sql, table, kind) in [
            (
                domain,
                DOMAIN_ARCHIVE_SCHEMA,
                "management_archive_identity",
                "management",
            ),
            (
                personal,
                PERSONAL_ARCHIVE_SCHEMA,
                "personal_archive_identity",
                "personal",
            ),
        ] {
            db.execute_batch(sql).unwrap();
            db.execute(
                &format!("INSERT INTO {table} VALUES(1,?1)"),
                [serde_json::to_string(&manifest(&self.plan, kind)).unwrap()],
            )
            .unwrap();
            db.pragma_update(None, "application_id", 0x48415231)
                .unwrap();
        }
    }
}
impl OwnerResourceRegistry {
    pub async fn resource_adoption_transfer(
        &self,
        authority: AdoptionAuthority,
        id: String,
    ) -> Result<AdoptionTransfer, Error> {
        authority.check()?;
        identifier(&id, 128)?;
        if !Arc::ptr_eq(&self.0, &authority.registry.0) {
            return Err(Error::LocalAuthority);
        }
        let after = authority.clone();
        let plan = self
            .0
            .store
            .call(4096, move |db| {
                authority.check()?;
                let (plan, _) = writer::read(&db.db, &authority, &id)?;
                if matches!(plan.version, 2 | 3) {
                    task_transfer::verify(&db.db, &plan)?;
                }
                authority.check()?;
                Ok(plan)
            })
            .await?;
        after.check()?;
        Ok(AdoptionTransfer {
            authority: after,
            plan,
        })
    }
    /// Exact canonical lookup for original native prepare recovery. Only a real
    /// missing row returns None; authorization, corruption and worker failures
    /// remain errors and cannot authorize recapturing an original source.
    pub async fn find_resource_adoption_transfer(
        &self,
        authority: AdoptionAuthority,
        id: String,
    ) -> Result<Option<AdoptionTransfer>, Error> {
        authority.check()?;
        identifier(&id, 128)?;
        if !Arc::ptr_eq(&self.0, &authority.registry.0) {
            return Err(Error::LocalAuthority);
        }
        let after = authority.clone();
        let plan = self
            .0
            .store
            .call(4096, move |db| {
                authority.check()?;
                let tx = db.db.unchecked_transaction()?;
                let exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM owner_resource_adoptions WHERE id=?1)",
                    [&id],
                    |r| r.get(0),
                )?;
                let plan = if exists {
                    let (plan, _) = writer::read(&tx, &authority, &id)?;
                    if matches!(plan.version, 2 | 3) {
                        task_transfer::verify(&tx, &plan)?;
                    }
                    Some(plan)
                } else {
                    None
                };
                authority.check()?;
                tx.commit()?;
                Ok(plan)
            })
            .await?;
        after.check()?;
        Ok(plan.map(|plan| AdoptionTransfer {
            authority: after,
            plan,
        }))
    }
    /// Records a currently verified source archive; does not import definitions.
    pub async fn record_adoption_sources(
        &self,
        authority: AdoptionAuthority,
        proof: VerifiedAdoptionArchives,
    ) -> Result<AdoptionSummary, Error> {
        proof.check(&authority)?;
        if !Arc::ptr_eq(&self.0, &authority.registry.0) {
            return Err(Error::LocalAuthority);
        }
        let after = authority.clone();
        let result = self
            .0
            .store
            .call(4096, move |db| {
                db.record_adoption_sources(&authority, &proof)
            })
            .await?;
        after.check().map_err(|_| Error::OutcomeUnknown)?;
        Ok(result)
    }
    /// Import inactive definitions after rechecking actual original archives.
    /// This creates no active profile access and no capacity allocation.
    pub async fn import_resource_adoption(
        &self,
        authority: AdoptionAuthority,
        proof: VerifiedAdoptionArchives,
    ) -> Result<AdoptionSummary, Error> {
        proof.check(&authority)?;
        if !Arc::ptr_eq(&self.0, &authority.registry.0) {
            return Err(Error::LocalAuthority);
        }
        let after = authority.clone();
        let result = self
            .0
            .store
            .call(4096, move |db| {
                db.import_resource_adoption(&authority, &proof)
            })
            .await?;
        after.check().map_err(|_| Error::OutcomeUnknown)?;
        Ok(result)
    }
}
