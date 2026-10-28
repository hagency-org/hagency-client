//! Profile-scoped descriptive selection. These reads never create a start intent
//! or reconstruct process custody; every effect still needs fresh typed proof.
use super::*;

pub struct SharedSuccessorSelection {
    original: SharedRuntimeCommand,
    predecessor: Option<SharedAttemptCommand>,
}
impl SharedSuccessorSelection {
    pub fn original(&self) -> &SharedRuntimeCommand {
        &self.original
    }
    pub fn predecessor(&self) -> Option<&SharedAttemptCommand> {
        self.predecessor.as_ref()
    }
    pub fn into_parts(self) -> (SharedRuntimeCommand, Option<SharedAttemptCommand>) {
        (self.original, self.predecessor)
    }
}

pub(super) fn saved_start(
    db: &Connection,
    profile: &ProfileAuthority,
    id: &str,
) -> Result<SharedAttemptStartIntent, Error> {
    let (child, saved_profile, original_digest, original, predecessor_digest, predecessor, input, input_digest): (String,String,String,String,String,Option<String>,String,String) = db.query_row(
        "SELECT child_id,profile,original_runtime_digest,original_runtime_payload,predecessor_digest,predecessor_payload,input_payload,input_digest FROM owner_shared_start_intents WHERE id=?1", [id],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?)),
    )?;
    let original: SharedRuntimeCommand = serde_json::from_str(&original)?;
    original.validate(&profile.account)?;
    let input: SharedAttemptStartInput = serde_json::from_str(&input)?;
    let (predecessor_id, number, actual_predecessor) = if let Some(payload) = predecessor {
        let head: SharedAttemptCommand = serde_json::from_str(&payload)?;
        head.validate(&profile.account)?;
        if head.original_runtime_digest != original_digest {
            return Err(Error::Conflict);
        }
        let digest = canonical::digest(&serde_json::to_value(&head)?)?;
        (head.command_id, head.process_attempt, digest)
    } else {
        (original.command_id.clone(), 1, original_digest.clone())
    };
    if saved_profile != profile.digest
        || child != original.agent_reservation_id
        || canonical::digest(&serde_json::to_value(&original)?)? != original_digest
        || canonical::digest(&serde_json::to_value(&input)?)? != input_digest
        || input.start_intent_id != id
        || input.assignment_id != original.assignment_id
        || input.predecessor.command_digest != predecessor_digest
        || predecessor_digest != actual_predecessor
        || input.predecessor.command_id != predecessor_id
        || input.predecessor.process_attempt != number
    {
        return Err(Error::Conflict);
    }
    Ok(SharedAttemptStartIntent { input, original })
}
impl OwnerResourceRegistry {
    /// Original lifetime only, independent of the successor protocol version.
    /// Useful for restriction/recovery paths; it grants no launch authority.
    pub async fn shared_original_selection(&self, profile: ProfileAuthority, assignment: String) -> Result<Option<SharedRuntimeCommand>, Error> {
        profile.check()?; identifier(&assignment,128)?;
        let p=profile.clone();let expires=p.expires;
        let value=self.0.store.call(32*1024,move|db|{
            profile.check()?;
            let row:Option<(String,String,String)>=db.db.query_row("SELECT child_id,command_digest,command_payload FROM owner_shared_runtime_lifetimes WHERE assignment_id=?1 AND profile=?2",params![assignment,profile.digest],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            let value=row.map(|(child,digest,payload)|->Result<SharedRuntimeCommand,Error>{
                let original:SharedRuntimeCommand=serde_json::from_str(&payload)?;original.validate(&profile.account)?;
                if original.assignment_id!=assignment||original.agent_reservation_id!=child||canonical::digest(&serde_json::to_value(&original)?)?!=digest{return Err(Error::Conflict)}
                Ok(original)
            }).transpose()?;
            profile.check()?;Ok(value)
        }).await?;
        after(&p,&self.0,expires)?;Ok(value)
    }
    pub async fn shared_successor_selection(
        &self,
        profile: ProfileAuthority,
        assignment: String,
    ) -> Result<Option<SharedSuccessorSelection>, Error> {
        profile.check()?;
        identifier(&assignment, 128)?;
        let p = profile.clone();
        let expires = p.expires;
        let result=self.0.store.call(64*1024,move|db|{
            profile.check()?;
            let row:Option<(String,String,String)>=db.db.query_row("SELECT child_id,command_digest,command_payload FROM owner_shared_runtime_lifetimes WHERE assignment_id=?1 AND profile=?2",params![assignment,profile.digest],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            let Some((child,digest,payload))=row else{return Ok(None)};
            let original:SharedRuntimeCommand=serde_json::from_str(&payload)?;
            original.validate(&profile.account)?;
            if original.assignment_id!=assignment || original.agent_reservation_id!=child || canonical::digest(&serde_json::to_value(&original)?)?!=digest {return Err(Error::Conflict)}
            let latest:Option<(String,String,String)>=db.db.query_row("SELECT profile,command_digest,command_payload FROM owner_shared_successor_attempts WHERE child_id=?1 ORDER BY process_attempt DESC LIMIT 1",[&child],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            let predecessor=latest.map(|(saved,digest,payload)|->Result<SharedAttemptCommand,Error>{
                let c:SharedAttemptCommand=serde_json::from_str(&payload)?;c.validate(&profile.account)?;
                if saved!=profile.digest || c.agent_reservation_id!=child || c.original_runtime_digest!=canonical::digest(&serde_json::to_value(&original)?)? || canonical::digest(&serde_json::to_value(&c)?)?!=digest {return Err(Error::Conflict)}
                Ok(c)
            }).transpose()?;
            profile.check()?;Ok(Some(SharedSuccessorSelection{original,predecessor}))
        }).await?;
        after(&p, &self.0, expires)?;
        Ok(result)
    }
    /// One bounded original journal entry. Empty/end does not create a request;
    /// the caller may rotate its opaque cursor on a later poll.
    pub async fn next_shared_attempt_start(
        &self,
        profile: ProfileAuthority,
        after_id: Option<String>,
    ) -> Result<Option<SharedAttemptStartIntent>, Error> {
        profile.check()?;
        if let Some(id) = &after_id {
            identifier(id, 128)?;
        }
        let p = profile.clone();
        let expires = p.expires;
        let result=self.0.store.call(64*1024,move|db|{
            profile.check()?;
            let id:Option<String>=db.db.query_row("SELECT id FROM owner_shared_start_intents WHERE profile=?1 AND (?2 IS NULL OR id>?2) AND NOT EXISTS(SELECT 1 FROM owner_shared_demand_decisions d WHERE d.start_intent_id=owner_shared_start_intents.id) ORDER BY id LIMIT 1",params![profile.digest,after_id],|r|r.get(0)).optional()?;
            let result=id.map(|id|saved_start(&db.db,&profile,&id)).transpose()?;
            profile.check()?;Ok(result)
        }).await?;
        after(&p, &self.0, expires)?;
        Ok(result)
    }
    /// Explicit Resume selects only the latest saved request for this assignment.
    /// It cannot substitute a fresh predecessor or a new request identifier.
    pub async fn latest_shared_attempt_start(
        &self,
        profile: ProfileAuthority,
        assignment: String,
    ) -> Result<Option<SharedAttemptStartIntent>, Error> {
        profile.check()?;
        identifier(&assignment, 128)?;
        let p = profile.clone();
        let expires = p.expires;
        let result=self.0.store.call(64*1024,move|db|{
            profile.check()?;
            let id:Option<String>=db.db.query_row("SELECT s.id FROM owner_shared_start_intents s JOIN owner_shared_runtime_lifetimes l ON l.child_id=s.child_id WHERE s.profile=?1 AND l.profile=?1 AND l.assignment_id=?2 AND NOT EXISTS(SELECT 1 FROM owner_shared_demand_decisions d WHERE d.start_intent_id=s.id) ORDER BY s.ledger_revision DESC LIMIT 1",params![profile.digest,assignment],|r|r.get(0)).optional()?;
            let result=id.map(|id|saved_start(&db.db,&profile,&id)).transpose()?;
            profile.check()?;Ok(result)
        }).await?;
        after(&p, &self.0, expires)?;
        Ok(result)
    }
}
