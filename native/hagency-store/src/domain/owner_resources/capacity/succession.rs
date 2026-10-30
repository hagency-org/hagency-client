//! A pool's capacity chain. Every successor (renewal: next UTC month;
//! amendment: same month) is its own immutable grant row and receipt; this
//! module only resolves which grant governs an instant and which grant a
//! child's hold is accounted to. No receipt, command or binding is rewritten.
use super::*;

/// Chains are bounded by the 2048-row grant cap of the profile.
const MAX_CHAIN: usize = 2049;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    Pending,
    Effective,
    Rejected,
    Cancelled,
}
impl State {
    fn parse(value: &str) -> Result<Self, Error> {
        Ok(match value {
            "pending" => Self::Pending,
            "effective" => Self::Effective,
            "rejected" => Self::Rejected,
            "cancelled" => Self::Cancelled,
            _ => return Err(Error::Schema),
        })
    }
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Effective => "effective",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
        }
    }
    /// A successor that never held capacity and never will.
    pub(crate) fn void(self) -> bool {
        matches!(self, Self::Rejected | Self::Cancelled)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Row {
    pub grant_id: String,
    pub predecessor_grant_id: String,
    pub kind: String,
    pub state: State,
    pub effective_from_ms: u64,
}
impl Row {
    pub(crate) fn amendment(&self) -> bool {
        self.kind == SUCCESSION_AMENDMENT
    }
    pub(crate) fn renewal(&self) -> bool {
        self.kind == SUCCESSION_RENEWAL
    }
}
fn read(r: &rusqlite::Row<'_>) -> rusqlite::Result<(String, String, String, String, u64)> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
}
fn parse(
    (grant_id, predecessor_grant_id, kind, state, effective_from_ms): (String, String, String, String, u64),
) -> Result<Row, Error> {
    Ok(Row {
        grant_id,
        predecessor_grant_id,
        kind,
        state: State::parse(&state)?,
        effective_from_ms,
    })
}
const COLUMNS: &str = "grant_id,predecessor_grant_id,kind,state,effective_from_ms";

/// This grant's own link to its predecessor; `None` for a chain root.
pub(crate) fn row(db: &Connection, grant: &str) -> Result<Option<Row>, Error> {
    db.query_row(
        &format!("SELECT {COLUMNS} FROM owner_capacity_grant_successions WHERE grant_id=?1"),
        [grant],
        read,
    )
    .optional()?
    .map(parse)
    .transpose()
}

/// The successor that currently occupies this grant's single slot: a pending
/// or effective successor. Rejected successors and cancelled renewals never
/// held capacity and free the slot.
pub(crate) fn live_successor(db: &Connection, predecessor: &str) -> Result<Option<Row>, Error> {
    let mut q = db.prepare(&format!(
        "SELECT {COLUMNS} FROM owner_capacity_grant_successions WHERE predecessor_grant_id=?1 AND state IN('pending','effective') ORDER BY grant_id LIMIT 2"
    ))?;
    let rows = q
        .query_map([predecessor], read)?
        .collect::<Result<Vec<_>, _>>()?;
    match rows.len() {
        0 => Ok(None),
        1 => Ok(Some(parse(rows.into_iter().next().ok_or(Error::Schema)?)?)),
        // The insert trigger allows one; two can only be a damaged ledger.
        _ => Err(Error::Conflict),
    }
}

/// The grant governing instant `t` on the path that starts at `grant`: follow
/// effective successors whose effective time has come. Pending, rejected and
/// cancelled successors are never followed.
pub(crate) fn effective_at(db: &Connection, grant: &str, t: u64) -> Result<String, Error> {
    let mut current = grant.to_owned();
    for _ in 0..MAX_CHAIN {
        let next = live_successor(db, &current)?
            .filter(|s| s.state == State::Effective && s.effective_from_ms <= t);
        match next {
            Some(s) => current = s.grant_id,
            None => return Ok(current),
        }
    }
    Err(Error::Capacity)
}

/// `grant` and its predecessors, nearest first, ending at the chain root.
pub(crate) fn lineage(db: &Connection, grant: &str) -> Result<Vec<String>, Error> {
    let mut out = vec![grant.to_owned()];
    while let Some(link) = row(db, out.last().ok_or(Error::Schema)?)? {
        if out.len() >= MAX_CHAIN || out.contains(&link.predecessor_grant_id) {
            return Err(Error::Capacity);
        }
        out.push(link.predecessor_grant_id);
    }
    Ok(out)
}

/// When this child was permanently fenced, if it is. A fenced child whose time
/// predates the fence-time table counts as fenced before any succession.
pub(crate) fn fenced_at(db: &Connection, child: &str) -> Result<Option<u64>, Error> {
    let (fenced, at): (bool, Option<u64>) = db.query_row(
        "SELECT a.permanent_fence,t.fenced_at_ms FROM owner_managed_agents a LEFT JOIN owner_managed_fence_times t ON t.child_id=a.id WHERE a.id=?1",
        [child],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(fenced.then(|| at.unwrap_or(0)))
}

/// The grant a child's hold counts in at `at`: the effective grant of its
/// original grant's chain at `min(at, fenced_at)`. A child fenced before an
/// amendment flips or a renewal starts stays with the grant it ran under.
pub(crate) fn accounting_grant(
    db: &Connection,
    original_grant: &str,
    fenced: Option<u64>,
    at: u64,
) -> Result<String, Error> {
    effective_at(db, original_grant, fenced.map_or(at, |f| f.min(at)))
}

/// Grant ids of the chain that contains `grant`: its lineage and every
/// successor reachable from the root, including void ones.
pub(crate) fn chain(db: &Connection, grant: &str) -> Result<std::collections::BTreeSet<String>, Error> {
    let root = lineage(db, grant)?.pop().ok_or(Error::Schema)?;
    let mut out = std::collections::BTreeSet::from([root.clone()]);
    let mut frontier = vec![root];
    while let Some(next) = frontier.pop() {
        let mut q = db.prepare(
            "SELECT grant_id FROM owner_capacity_grant_successions WHERE predecessor_grant_id=?1 ORDER BY grant_id LIMIT 2049",
        )?;
        for id in q.query_map([&next], |r| r.get::<_, String>(0))? {
            let id = id?;
            if out.len() >= MAX_CHAIN {
                return Err(Error::Capacity);
            }
            if out.insert(id.clone()) {
                frontier.push(id);
            }
        }
    }
    Ok(out)
}

/// A void successor (rejected, or a cancelled renewal) is history that never
/// held capacity; nothing ever ran under it.
pub(crate) fn void(db: &Connection, grant: &str) -> Result<bool, Error> {
    Ok(row(db, grant)?.is_some_and(|r| r.state.void()))
}

/// This grant was replaced within its month by an effective amendment, and the
/// grant now governing its chain is canonically closed. Such a grant never gets
/// its own closure: its remaining children's liability stays counted.
pub(crate) fn superseded_into_closed_head(db: &Connection, grant: &str) -> Result<bool, Error> {
    let Some(next) = live_successor(db, grant)? else {
        return Ok(false);
    };
    if next.state != State::Effective || !next.amendment() {
        return Ok(false);
    }
    let head = effective_at(db, &next.grant_id, u64::MAX)?;
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM owner_parent_settlements WHERE grant_id=?1)",
        [&head],
        |r| r.get(0),
    )?)
}

fn receipt_of(db: &Connection, grant: &str) -> Result<(NativeGrantReceipt, bool), Error> {
    let (raw, open): (String, bool) = db.query_row(
        "SELECT g.receipt,g.active AND NOT EXISTS(SELECT 1 FROM owner_parent_settlements WHERE grant_id=g.id) FROM owner_capacity_grants g WHERE g.id=?1",
        [grant],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok((serde_json::from_str(&raw)?, open))
}

/// When the contiguous capacity chain governing `original` at `now` ends
/// (phase 1b, E8): the governing grant's own end, extended by each open,
/// effective renewal that starts exactly when its predecessor's month ends
/// while that predecessor's delegation still reaches the month end.
pub(crate) fn chain_until(db: &Connection, original: &str, now: u64) -> Result<u64, Error> {
    let head = effective_at(db, original, now)?;
    let (mut receipt, _) = receipt_of(db, &head)?;
    let mut until = receipt.period_end_ms.min(receipt.delegation_expires_at_ms);
    for _ in 0..MAX_CHAIN {
        let Some(next) = live_successor(db, &receipt.grant_id)? else {
            break;
        };
        if !next.renewal()
            || next.state != State::Effective
            || until != receipt.period_end_ms
            || next.effective_from_ms != receipt.period_end_ms
        {
            break;
        }
        let (renewal, open) = receipt_of(db, &next.grant_id)?;
        if !open || renewal.period_start_ms != receipt.period_end_ms {
            break;
        }
        until = renewal.period_end_ms.min(renewal.delegation_expires_at_ms);
        receipt = renewal;
    }
    Ok(until)
}
