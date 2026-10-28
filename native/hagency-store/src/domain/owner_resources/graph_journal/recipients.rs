//! Reviewed native recipient snapshots. Custody of a review is not delivery
//! permission; the original message remains held with its original identity.
use super::*;

const MAX_SNAPSHOT: usize = 32 * 1024;

/// Native-only exact review bytes. `revision` is the graph revision that saved
/// this review, not an independently incremented recipient revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphRecipientBinding {
    pub snapshot: Vec<u8>,
    pub snapshot_digest: String,
    pub revision: u64,
}

impl GraphJournalWriter<'_> {
    /// Save or explicitly replace a native-resolved review under this graph's
    /// compare-and-swap. The adapter must verify the node exists and has never
    /// been issued; the store additionally refuses any non-held outbox row.
    /// A node waiting on dependencies may not have an outbox row yet.
    ///
    /// `snapshot` must be a bounded native JSON object, never a script identity.
    /// This does not enqueue, release, rewrite or deliver a message.
    pub fn bind_recipient(&mut self, node_id: &str, snapshot: Vec<u8>) -> Result<(), Error> {
        let result = self.bind_recipient_inner(node_id, snapshot);
        self.failed |= result.is_err();
        result
    }

    fn bind_recipient_inner(&mut self, node: &str, snapshot: Vec<u8>) -> Result<(), Error> {
        let a = self.authority;
        a.check_db(self.db)?;
        exact_id(node)?;
        validate_snapshot(&snapshot)?;
        // Bindings are for an existing, reviewed graph; creation cannot invent
        // a recipient without first exposing that graph for native review.
        if self.revision <= 1 {
            return Err(Error::Conflict);
        }
        let state: Option<String> = self.db.query_row(
            "SELECT state FROM owner_operator_graph_outbox WHERE registry_id=?1 AND profile=?2 AND owner_mxid=?3 AND scope_id=?4 AND graph_id=?5 AND node_id=?6",
            params![a.registry.0.id,a.profile.digest,a.profile.account,self.scope,self.graph,node],
            |r| r.get(0),
        ).optional()?;
        if state
            .as_deref()
            .is_some_and(|state| state != "pending_binding")
        {
            return Err(Error::Conflict);
        }
        let previous = read_binding(self.db, a, self.scope, self.graph, node)?;
        if let Some(previous) = &previous {
            // A transition cannot silently replace its own first review. An
            // exact repeated callback write is harmless and retains its bytes.
            if previous.revision == self.revision {
                return if previous.snapshot == snapshot {
                    Ok(())
                } else {
                    Err(Error::Conflict)
                };
            }
        } else {
            let count: u64 = self.db.query_row(
                "SELECT COUNT(*) FROM owner_operator_graph_recipients WHERE registry_id=?1 AND profile=?2 AND owner_mxid=?3 AND scope_id=?4 AND graph_id=?5",
                params![a.registry.0.id,a.profile.digest,a.profile.account,self.scope,self.graph],
                |r| r.get(0),
            )?;
            if count >= MAX_MESSAGES {
                return Err(Error::Capacity);
            }
        }
        a.check_db(self.db)?;
        self.db.execute(
            "INSERT INTO owner_operator_graph_recipients VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(registry_id,profile,owner_mxid,scope_id,graph_id,node_id) DO UPDATE SET revision=excluded.revision,snapshot=excluded.snapshot,snapshot_digest=excluded.snapshot_digest",
            params![a.registry.0.id,a.profile.digest,a.profile.account,self.scope,self.graph,node,self.revision,snapshot,bytes_digest(&snapshot)],
        )?;
        a.check_db(self.db)
    }
}

impl OwnerResourceRegistry {
    /// Scoped native read with digest verification. Possession of the snapshot
    /// grants no runtime or Matrix authority; callers revalidate live identity.
    pub async fn operator_graph_recipient(
        &self,
        profile: &ProfileAuthority,
        scope: &str,
        graph: &str,
        node: &str,
    ) -> Result<Option<GraphRecipientBinding>, Error> {
        exact_id(scope)?;
        exact_id(graph)?;
        exact_id(node)?;
        let a = Authority {
            registry: self.clone(),
            profile: profile.clone(),
        };
        a.check()?;
        let after = a.clone();
        let (scope, graph, node) = (scope.to_owned(), graph.to_owned(), node.to_owned());
        let result = self.0.store.call(MAX_SNAPSHOT as u32 + 4096, move |db| {
            a.check_db(&db.db)?;
            let binding = read_binding(&db.db, &a, &scope, &graph, &node)?;
            if let Some(binding) = &binding {
                let graph_revision: Option<u64> = db.db.query_row(
                    "SELECT revision FROM owner_operator_graphs WHERE registry_id=?1 AND profile=?2 AND owner_mxid=?3 AND scope_id=?4 AND graph_id=?5",
                    params![a.registry.0.id,a.profile.digest,a.profile.account,scope,graph], |r| r.get(0),
                ).optional()?;
                if graph_revision.is_none_or(|revision| binding.revision > revision) {
                    return Err(Error::Quarantined);
                }
            }
            a.check_db(&db.db)?;
            Ok(binding)
        }).await?;
        after.check()?;
        Ok(result)
    }
}

fn validate_snapshot(snapshot: &[u8]) -> Result<(), Error> {
    if snapshot.len() > MAX_SNAPSHOT {
        return Err(Error::Capacity);
    }
    if !serde_json::from_slice::<serde_json::Value>(snapshot)?.is_object() {
        return Err(Error::Unqualified);
    }
    Ok(())
}

fn read_binding(
    db: &Connection,
    a: &Authority,
    scope: &str,
    graph: &str,
    node: &str,
) -> Result<Option<GraphRecipientBinding>, Error> {
    let row: Option<(u64, Vec<u8>, String)> = db.query_row(
        "SELECT revision,CASE WHEN length(snapshot)<=32768 THEN snapshot END,snapshot_digest FROM owner_operator_graph_recipients WHERE registry_id=?1 AND profile=?2 AND owner_mxid=?3 AND scope_id=?4 AND graph_id=?5 AND node_id=?6",
        params![a.registry.0.id,a.profile.digest,a.profile.account,scope,graph,node],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    ).optional()?;
    row.map(|(revision, snapshot, snapshot_digest)| {
        if revision < 2 || bytes_digest(&snapshot) != snapshot_digest {
            return Err(Error::Quarantined);
        }
        validate_snapshot(&snapshot).map_err(|_| Error::Quarantined)?;
        Ok(GraphRecipientBinding {
            snapshot,
            snapshot_digest,
            revision,
        })
    })
    .transpose()
}

#[cfg(test)]
mod tests;
