//! The remote owner's irreversible verdict publication, not execution authority.
use super::*;
pub(crate) const SCHEMA: &str = include_str!("review.sql");
pub(crate) const OBJECTS: &[&str] = &[
    "remote_verdict_publications",
    "retain_remote_verdict_publication",
    "immutable_remote_verdict_publication",
    "remote_verdict_receipts",
    "retain_remote_verdict_receipt",
    "immutable_remote_verdict_receipt",
];
/// Construct only after actual SDK private-state, signature and device checks.
/// JSON deserialization cannot establish these facts.
pub struct ObservedRequest {
    pub observed_at: i64,
    pub event_id: String,
    pub sender_ed25519: String,
    pub reviewer_matrix_device: String,
    pub request: Request,
}
pub struct RequestPage {
    pub requests: Vec<ObservedRequest>,
    pub next_batch: Option<String>,
}
impl ObservedRequest {
    pub fn validate(&self, owner: &str, profile: &str, now: i64) -> Result<()> {
        self.request.validate()?;
        atom(&self.reviewer_matrix_device, 255)?;
        if self.request.owner != owner
            || self.request.profile != profile
            || !event(&self.event_id)
            || self.sender_ed25519.len() != 43
            || self.reviewer_matrix_device == self.request.publisher_matrix_device
            || self.observed_at < 0
            || self.observed_at > now
            || now - self.observed_at > 5
            || self.request.proposal.expires <= now
        {
            return Err(Error::Unauthorized);
        }
        Ok(())
    }
}
pub struct VerdictPublication {
    record: VerdictRecord,
    instance: std::sync::Arc<()>,
    can_send: bool,
}
pub struct VerdictSendPermit {
    record: VerdictRecord,
}
impl VerdictPublication {
    pub fn begin_publication(&mut self) -> Result<VerdictSendPermit> {
        if !std::mem::replace(&mut self.can_send, false) {
            return Err(Error::Unknown);
        }
        Ok(VerdictSendPermit {
            record: self.record.clone(),
        })
    }
}
impl VerdictSendPermit {
    pub fn record(&self) -> &VerdictRecord {
        &self.record
    }
    pub fn transaction_id(&self) -> String {
        format!("hagency.remote-verdict.{}", self.record.request_digest)
    }
    pub fn content(&self) -> Result<Value> {
        Verdict {
            approval_id: self.record.request.approval_id.clone(),
            request_digest: self.record.request_digest.clone(),
            approved: self.record.approved,
        }
        .message_content(&self.record.request_event_id)
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerdictRecord {
    pub request: Request,
    pub request_digest: String,
    pub request_event_id: String,
    pub sender_ed25519: String,
    pub reviewer_matrix_device: String,
    pub approved: bool,
    pub created_at: i64,
    pub event_id: Option<String>,
}
impl Ledger {
    fn remote_review_identity(&self, owner: &str, request: &Request) -> Result<()> {
        self.owner(owner)?;
        request.validate()?;
        let profile: String =
            self.db
                .query_row("SELECT digest FROM profile_identity", [], |r| r.get(0))?;
        if request.owner != owner || request.profile != profile {
            return Err(Error::Unauthorized);
        }
        Ok(())
    }
    /// Commit possible IO before returning the only send custody. Another event,
    /// device, decision, handle or restored snapshot cannot rearm this approval.
    pub fn reserve_remote_verdict(
        &mut self,
        owner: &str,
        observed: &ObservedRequest,
        reviewed_digest: &str,
        approved: bool,
        clock: impl Fn() -> i64,
    ) -> Result<VerdictPublication> {
        self.remote_review_identity(owner, &observed.request)?;
        let request_digest = observed.request.digest()?;
        if request_digest != reviewed_digest {
            return Err(Error::Unauthorized);
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = clock();
        observed.validate(owner, &observed.request.profile, now)?;
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM remote_verdict_publications WHERE approval_id=?)",
            [&observed.request.approval_id],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(Error::Unknown);
        }
        let (count,bytes):(usize,usize)=tx.query_row("SELECT COUNT(*),COALESCE(SUM(length(CAST(record AS BLOB))),0) FROM remote_verdict_publications", [], |r|Ok((r.get(0)?,r.get(1)?)))?;
        let record = VerdictRecord {
            request: observed.request.clone(),
            request_digest,
            request_event_id: observed.event_id.clone(),
            sender_ed25519: observed.sender_ed25519.clone(),
            reviewer_matrix_device: observed.reviewer_matrix_device.clone(),
            approved,
            created_at: now,
            event_id: None,
        };
        let encoded = json(&record)?;
        if count >= MAX_PUBLICATIONS || bytes + encoded.len() > MAX_PUBLICATION_BYTES {
            return Err(Error::Invalid("remote verdict capacity"));
        }
        tx.execute(
            "INSERT INTO remote_verdict_publications VALUES (?,?,?)",
            (&record.request.approval_id, &record.request_digest, encoded),
        )?;
        tx.commit()?;
        Ok(VerdictPublication {
            record,
            instance: self.remote_instance.clone(),
            can_send: true,
        })
    }
    pub fn record_remote_verdict_receipt(
        &mut self,
        owner: &str,
        publication: &VerdictPublication,
        event_id: &str,
        now: i64,
    ) -> Result<()> {
        self.remote_review_identity(owner, &publication.record.request)?;
        if !std::sync::Arc::ptr_eq(&self.remote_instance, &publication.instance)
            || !event(event_id)
            || event_id == publication.record.request_event_id
            || now < publication.record.created_at
        {
            return Err(Error::Unauthorized);
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let original: Option<String> = tx
            .query_row(
                "SELECT event_id FROM remote_verdict_receipts WHERE approval_id=?",
                [&publication.record.request.approval_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(original) = original {
            return if original == event_id {
                Ok(())
            } else {
                Err(Error::Conflict)
            };
        }
        tx.execute(
            "INSERT INTO remote_verdict_receipts VALUES (?,?,?)",
            (&publication.record.request.approval_id, event_id, now),
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn remote_verdict_record(
        &self,
        owner: &str,
        approval_id: &str,
    ) -> Result<Option<VerdictRecord>> {
        self.owner(owner)?;
        if !digest(approval_id) {
            return Err(Error::Invalid("remote approval id"));
        }
        let row:Option<(String,Option<String>)>=self.db.query_row("SELECT p.record,r.event_id FROM remote_verdict_publications p LEFT JOIN remote_verdict_receipts r USING(approval_id) WHERE p.approval_id=?", [approval_id], |r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let Some((encoded, event)) = row else {
            return Ok(None);
        };
        let mut record: VerdictRecord = serde_json::from_str(&encoded)?;
        self.remote_review_identity(owner, &record.request)?;
        if record.request.digest()? != record.request_digest {
            return Err(Error::ForeignDatabase);
        }
        record.event_id = event;
        Ok(Some(record))
    }
}
#[cfg(test)]
mod tests;
