//! Exact v3 evidence recovery; never constructs a process or model-call proof.
use super::*;
impl OwnerResourceRegistry {
    /// Bounded original inbox discovery for restart evidence upload. Identifiers
    /// confer no call authority; missing or unreported observations stay held.
    pub async fn shared_budget_attempt_work_ids(
        &self,
        a: ManagedCleanupAuthority,
        c: SharedBudgetAttemptCommand,
    ) -> Result<Vec<String>, Error> {
        a.check(&self.0)?;
        c.validate(&a.enrollment.profile.account)?;
        if a.child != c.agent_reservation_id {
            return Err(Error::LocalAuthority);
        }
        let p = a.enrollment.profile.clone();
        let expires = a.deadline();
        let registry = self.0.clone();
        let result=self.0.store.call(8*1024,move|db|{
            a.check(&registry)?;authority::cleanup(&db.db,&a)?;
            let digest=canonical::digest(&serde_json::to_value(&c)?)?;
            let saved:Option<String>=db.db.query_row("SELECT command_digest FROM owner_shared_successor_attempts WHERE child_id=?1 AND profile=?2 AND id=?3",params![a.child,a.enrollment.profile.digest,c.command_id],|r|r.get(0)).optional()?;
            if saved.is_none(){a.check(&registry)?;return Ok(Vec::new())}
            if saved.as_deref()!=Some(&digest){return Err(Error::LocalAuthority)}
            let mut q=db.db.prepare("SELECT id FROM owner_shared_work_inbox WHERE child_id=?1 AND json_extract(payload,'$.runtimeCommandDigest')=?2 ORDER BY id LIMIT 33")?;
            let ids=q.query_map(params![a.child,digest],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            if ids.len()>32{return Err(Error::Capacity)}
            for id in &ids {identifier(id,128)?;}
            a.check(&registry)?;Ok(ids)
        }).await?;
        after(&p, &self.0, expires)?;
        Ok(result)
    }
    pub async fn recover_shared_budget_attempt_work(
        &self,
        a: ManagedCleanupAuthority,
        c: SharedBudgetAttemptCommand,
        work_id: String,
        observation_id: Option<String>,
    ) -> Result<Option<SharedSavedWork>, Error> {
        a.check(&self.0)?;
        c.validate(&a.enrollment.profile.account)?;
        identifier(&work_id, 128)?;
        if let Some(id) = &observation_id {
            identifier(id, 128)?;
        }
        if a.child != c.agent_reservation_id {
            return Err(Error::LocalAuthority);
        }
        let p = a.enrollment.profile.clone();
        let expires = a.deadline();
        let registry = self.0.clone();
        let result=self.0.store.call(48*1024,move|db|{
            a.check(&registry)?;authority::cleanup(&db.db,&a)?;
            let command_digest=canonical::digest(&serde_json::to_value(&c)?)?;
            let original:Option<(String,String)>=db.db.query_row("SELECT command_digest,incarnation FROM owner_shared_successor_attempts WHERE child_id=?1 AND profile=?2 AND id=?3",params![a.child,a.enrollment.profile.digest,c.command_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            if original.is_none(){a.check(&registry)?;return Ok(None)}
            if original.as_ref().is_some_and(|v|v.0!=command_digest){return Err(Error::LocalAuthority)}
            let payload:Option<String>=db.db.query_row("SELECT payload FROM owner_shared_work_inbox WHERE id=?1 AND child_id=?2",params![work_id,a.child],|r|r.get(0)).optional()?;
            let result=if let Some(payload)=payload {
                let work:SharedWorkItem=serde_json::from_str(&payload)?;work.validate()?;
                if work.runtime_command_digest!=command_digest||work.assignment_id!=c.assignment_id{return Err(Error::LocalAuthority)}
                let receipt:Option<String>=db.db.query_row("SELECT payload FROM owner_shared_work_observations WHERE work_id=?1 AND (?2 IS NULL OR id=?2) ORDER BY ledger_revision DESC LIMIT 1",params![work.id,observation_id],|r|r.get(0)).optional()?;
                let receipt=receipt.map(|v|serde_json::from_str::<SharedWorkReceipt>(&v)).transpose()?;
                if let Some(r)=&receipt {
                    let provider:String=db.db.query_row("SELECT provider_digest FROM owner_shared_successor_attempts WHERE id=?1",[&c.command_id],|r|r.get(0))?;
                    let process=canonical::digest(&json!({"commandDigest":command_digest,"incarnation":original.unwrap().1,"providerDigest":provider}))?;
                    r.validate_for(&work,&process)?;
                }
                Some(SharedSavedWork{work,receipt})
            }else {None};
            a.check(&registry)?;Ok(result)
        }).await?;
        after(&p, &self.0, expires)?;
        Ok(result)
    }
}
