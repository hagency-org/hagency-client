//! Observation only: no source opener, recovery, migration or allocation grant.
//! Use the already-owned worker so committed WAL rows and private profile scope
//! are retained. Counts cannot establish that another ledger has no liability.
use super::*;

/// A lower bound when capped, never a falsely complete count.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditCount {
    pub observed: u32,
    pub capped: bool,
}
impl AuditCount {
    pub(crate) fn add(&mut self, count: &Self) {
        self.observed += count.observed;
        self.capped |= count.capped;
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditResource {
    pub id: String,
    pub revision: String,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileLedgerHistory {
    pub resource_definitions: AuditCount,
    pub seat_definitions: AuditCount,
    pub command_receipts: AuditCount,
    pub pending_intents: AuditCount,
    pub replied_intents: AuditCount,
    pub acknowledged_intents: AuditCount,
    pub capacity_and_usage_rows: AuditCount,
    pub execution_rows: AuditCount,
    /// Includes unknown tables. Names and contents never leave this audit.
    pub other_history_rows: AuditCount,
    pub other_populated_tables: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditBlocker {
    ProviderMappingNotEstablished,
    OlderSourceSchema,
    PersonalLedgerNotInspected,
    SourceCensusNotEstablished,
    PendingPrivateCommands,
    CapacityOrUsageHistory,
    ExecutionHistory,
    OtherHistory,
    ObservationLimit,
}

/// Safe inspection output, not a migration proof or authorization to adopt.
/// Deliberately not Deserialize; it cannot be presented as trusted evidence.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileLedgerAudit {
    pub version: u16,
    pub schema_version: i32,
    pub optional_history_tables_missing: u32,
    pub observed_at_ms: u64,
    pub scope: &'static str,
    pub adoption: &'static str,
    pub tables_observed: u32,
    pub counts_complete: bool,
    pub resources_complete: bool,
    pub resources: Vec<AuditResource>,
    pub history: ProfileLedgerHistory,
    pub blockers: Vec<AuditBlocker>,
}

impl Session {
    /// `profile_digest` is derived by the native host from its authenticated
    /// server/issuer/subject/owner, never accepted from mini-app arguments.
    /// The profile must match the selected private source database exactly.
    pub async fn reconciliation_audit(
        &self,
        context: &HostContext,
        profile_digest: String,
    ) -> Result<ProfileLedgerAudit, Error> {
        let _pending = self.scope.pending.try_acquire().map_err(|_| Error::Busy)?;
        self.check(context, Permission::ReadResources)?;
        if profile_digest.len() != 64
            || !profile_digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::LocalAuthority);
        }
        let session = self.clone();
        let caller = context.clone();
        let result = self
            .store
            .legacy()?
            .call(32 * 1024, move |db| {
                session.check(&caller, Permission::ReadResources)?;
                let result = db.profile_ledger_audit(&profile_digest)?;
                session.check(&caller, Permission::ReadResources)?;
                Ok(result)
            })
            .await?;
        self.check(context, Permission::ReadResources)?;
        Ok(result)
    }
}
