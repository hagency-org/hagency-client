use super::*;
use hagency_core::recurring_quota::Cadence;

struct Fixture {
    _root: tempfile::TempDir,
    db: DomainRepository,
    first: Resource,
    sibling: Resource,
}

impl Fixture {
    fn new(cap: Option<u64>, cadence: Cadence) -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut db = DomainRepository::open(&root.path().join("state")).unwrap();
        let resource = |name: &str| {
            serde_json::from_value::<Resource>(json!({
                "presetId":name,"seatId":"calendar_account","framework":"codex",
                "model":"gpt-5.6-sol","reasoning":"medium","published":true,
                "ceiling":{"tokens":1000,"period":cadence}
            })).unwrap()
        };
        let first = resource("calendar_first");
        let sibling = resource("calendar_sibling");
        db.put_resource(&first).unwrap();
        db.put_resource(&sibling).unwrap();
        let mut fixture = Self { _root: root, db, first, sibling };
        fixture.set_cap(cap, cadence);
        fixture
    }

    fn set_cap(&mut self, cap: Option<u64>, cadence: Cadence) {
        let seat: Seat = serde_json::from_value(json!({
            "id":"calendar_account","declaration":{"quotaTokens":cap,"period":cadence}
        })).unwrap();
        self.db.put_seat(&seat).unwrap();
    }

    // Seed an approved V4 parent in the real native ledger. The production
    // binding and admission checks must run before any grant row is retained.
    fn publish(&mut self, resource: &Resource, calendar: &Calendar, id: &str, at: u64, tokens: u64) -> Result<(), Error> {
        let tx = self.db.db.transaction()?;
        bind_calendar(&tx, &resource.id(), &"a".repeat(64), id, Some(calendar), at)?;
        check_grant(&tx, resource, id, tokens, at)?;
        let vector: Value = serde_json::from_str(include_str!("owner_resources/capacity/managed_agents/pool-agent-v1.json"))?;
        let mut receipt: owner_resources::NativeGrantReceipt = serde_json::from_value(vector["command"]["poolGrant"].clone())?;
        let period = calendar.period_at(at).unwrap();
        receipt.version = 4;
        receipt.grant_id = id.into();
        receipt.resource_id = resource.id();
        receipt.resource_revision = crate::resource_publication_revision(resource)?;
        receipt.allocated_tokens = tokens;
        receipt.admission_request_id = Some(format!("admission_{id}"));
        receipt.calendar = Some(calendar.clone());
        receipt.period_key = period.key;
        receipt.period_start_ms = period.start_ms;
        receipt.period_end_ms = period.end_ms;
        assert!(receipt.supported_shape());
        tx.execute(
            "INSERT INTO owner_capacity_grants(id,profile,resource_id,preset_id,seat_id,allocated_tokens,receipt,active,accounting_mode) VALUES(?1,?2,?3,?4,?5,?6,?7,1,'reservation_only_v1')",
            params![id,"a".repeat(64),resource.id(),resource.preset_id,resource.seat_id,tokens,serde_json::to_string(&receipt)?],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn assert_mismatch(&self, resource: &Resource, at: u64) {
        let budget = budget_at(&self.db.db, resource, None, false, at).unwrap();
        assert_eq!(budget.seat.status, allocation::SeatStatus::PeriodMismatch);
        assert!(budget.remaining_tokens.is_none());
        assert!(budget.seat.remaining.is_none());
        assert!(matches!(check_grant(&self.db.db, resource, "blocked", 1, at), Err(Error::NoCeiling)));
    }
}

fn calendar(cadence: Cadence, zone: &str) -> Calendar {
    Calendar { cadence, time_zone: zone.into() }
}

// 2026-11-01 00:30 UTC is already November for the account, still October
// for Los Angeles. Its next local boundary is seven hours after UTC's.
const OVERLAP: u64 = 1_793_493_000_000;

#[test]
fn shared_account_calendar_rejects_incompatible_initial_publication_without_changing_owner_cap() {
    let mut f = Fixture::new(Some(1000), Cadence::Monthly);
    let first = f.first.clone();
    for selected in [
        calendar(Cadence::Monthly, "America/Los_Angeles"),
        // London currently has offset zero but is not a fixed UTC calendar.
        calendar(Cadence::Monthly, "Europe/London"),
        calendar(Cadence::Monthly, "Etc/UTC"),
        calendar(Cadence::Weekly, "UTC"),
    ] {
        assert!(matches!(f.publish(&first, &selected, "denied", OVERLAP, 1000), Err(Error::NoCeiling)));
        let rows: u64 = f.db.db.query_row("SELECT COUNT(*) FROM owner_resource_calendars", [], |row| row.get(0)).unwrap();
        assert_eq!(rows, 0);
        let grants: u64 = f.db.db.query_row("SELECT COUNT(*) FROM owner_capacity_grants", [], |row| row.get(0)).unwrap();
        assert_eq!(grants, 0);
        assert_eq!(account_ceiling::declaration(&f.db.db, "calendar_account").unwrap().unwrap().quota_tokens.map(u64::from), Some(1000));
    }
}

#[test]
fn shared_account_calendar_prevents_overlapping_month_replenishment_after_cap_is_enabled() {
    let mut f = Fixture::new(None, Cadence::Monthly);
    let first = f.first.clone();
    let sibling = f.sibling.clone();
    let local = calendar(Cadence::Monthly, "America/Los_Angeles");
    let utc = calendar(Cadence::Monthly, "UTC");
    let local_period = local.period_at(OVERLAP).unwrap();
    let account_period = utc.period_at(OVERLAP).unwrap();
    assert_eq!(local_period.key, "2026-10");
    assert_eq!(account_period.key, "2026-11");
    assert!(account_period.start_ms < local_period.end_ms);
    assert!(local_period.end_ms < account_period.end_ms);
    f.publish(&first, &local, "local_october", OVERLAP, 1000).unwrap();
    let before = budget_at(&f.db.db, &first, None, false, OVERLAP).unwrap();
    assert_eq!(before.pool.remaining.map(u64::from), Some(0));
    // With no account cap, an arbitrary resource calendar resets normally.
    let after = budget_at(&f.db.db, &first, None, false, local_period.end_ms).unwrap();
    assert_eq!(after.remaining_tokens.map(u64::from), Some(1000));
    // Enabling the account cap must not interpret released October-local
    // liability as permission to spend again in that same UTC November.
    f.set_cap(Some(1000), Cadence::Monthly);
    for at in [OVERLAP, local_period.end_ms, account_period.end_ms] {
        f.assert_mismatch(&first, at);
        f.assert_mismatch(&sibling, at);
    }
    // Both existing binding replay and a new UTC sibling fail closed.
    assert!(matches!(bind_calendar(&f.db.db, &first.id(), &"a".repeat(64), "local_october", Some(&local), local_period.end_ms), Err(Error::NoCeiling)));
    assert!(matches!(f.publish(&sibling, &utc, "utc_november", local_period.end_ms, 1000), Err(Error::NoCeiling)));
    assert_eq!(account_ceiling::declaration(&f.db.db, "calendar_account").unwrap().unwrap().quota_tokens.map(u64::from), Some(1000));
}

#[test]
fn shared_account_calendar_checks_sibling_cadence_after_publication() {
    let mut f = Fixture::new(None, Cadence::Monthly);
    let first = f.first.clone();
    f.publish(&first, &calendar(Cadence::Daily, "UTC"), "daily", OVERLAP, 500).unwrap();
    f.set_cap(Some(1000), Cadence::Monthly);
    f.assert_mismatch(&first, OVERLAP);
    f.assert_mismatch(&f.sibling, OVERLAP);
}

#[test]
fn shared_account_calendar_matching_utc_windows_preserve_the_owner_limit() {
    for cadence in [Cadence::Daily, Cadence::Weekly, Cadence::Monthly] {
        let mut f = Fixture::new(Some(1000), cadence);
        let first = f.first.clone();
        let sibling = f.sibling.clone();
        let selected = calendar(cadence, "UTC");
        f.publish(&first, &selected, "first", OVERLAP, 700).unwrap();
        let budget = budget_at(&f.db.db, &sibling, None, false, OVERLAP).unwrap();
        assert_eq!(budget.seat.status, allocation::SeatStatus::Declared);
        assert_eq!(budget.remaining_tokens.map(u64::from), Some(300));
        assert!(f.publish(&sibling, &selected, "too_much", OVERLAP, 301).is_err());
        f.publish(&sibling, &selected, "sibling", OVERLAP, 300).unwrap();
        let exhausted = budget_at(&f.db.db, &first, None, false, OVERLAP).unwrap();
        assert_eq!(exhausted.remaining_tokens.map(u64::from), Some(0));
        let next = selected.period_at(OVERLAP).unwrap().end_ms;
        let renewed = budget_at(&f.db.db, &first, None, false, next).unwrap();
        assert_eq!(renewed.seat.status, allocation::SeatStatus::Declared);
        assert_eq!(renewed.remaining_tokens.map(u64::from), Some(1000));
    }
}
