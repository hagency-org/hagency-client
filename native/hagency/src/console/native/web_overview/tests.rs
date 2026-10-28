use super::*;
use hagency_core::project::{Resource, Seat};

fn resource(id: &str, quota: Option<u64>) -> service::ResourceSummary {
    let temp = tempfile::tempdir().unwrap();
    let mut db =
        hagency_store::DomainRepository::open_managed(&temp.path().join("state"), &"a".repeat(64))
            .unwrap();
    let config: Resource = serde_json::from_value(json!({"presetId":id,"seatId":"shared_seat","framework":"codex","model":"gpt-6.1-sol","reasoning":"low","published":true,"ceiling":{"tokens":100,"period":"monthly"}})).unwrap();
    let seat: Seat = serde_json::from_value(json!({"id":"shared_seat","declaration":quota.map(|tokens|json!({"quotaTokens":tokens,"period":"monthly"}))})).unwrap();
    db.put_seat(&seat).unwrap();
    db.put_resource(&config).unwrap();
    let (budget, usage) = db
        .resource_headroom(&config.id(), 1_780_000_000_000)
        .unwrap();
    service::ResourceSummary {
        id: config.id(),
        revision: "a".repeat(64),
        framework: config.framework,
        model: config.model,
        reasoning: config.reasoning,
        capabilities: vec![],
        published: true,
        budget,
        usage,
        local_name: None,
    }
}
fn page(resources: Vec<service::ResourceSummary>) -> service::ResourcePage {
    service::ResourcePage {
        protocol: "fixture",
        observed_at_ms: 1_780_000_000_000,
        resources,
        next_after: None,
    }
}
fn account(count: u64) -> service::AccountSummary {
    service::AccountSummary {
        id: "binding_original".into(),
        kind: "resource_binding",
        runtime: "codex".into(),
        state: "linked".into(),
        ordinal: None,
        revision: None,
        profile: None,
        readiness: "unknown",
        identity_key: None,
        resource_count: count,
    }
}
#[test]
fn web_overview_arguments_are_closed_and_bounded() {
    assert!(serde_json::from_value::<WebOverviewQuery>(json!({"profile":"other"})).is_err());
    for limit in [0, 17, u16::MAX] {
        assert!(
            WebOverviewQuery {
                after: None,
                limit: Some(limit)
            }
            .validate()
            .is_err()
        );
    }
    assert!(
        WebAccountCapacityQuery {
            id: None,
            after: None,
            resource_after: Some("resource_a".into())
        }
        .validate()
        .is_err()
    );
    assert!(
        WebAccountCapacityQuery {
            id: Some("account_a".into()),
            after: Some("account_b".into()),
            resource_after: None
        }
        .validate()
        .is_err()
    );
    assert!(
        WebTerminalQuery {
            id: "../../private".into()
        }
        .validate()
        .is_err()
    );
    assert!(
        serde_json::from_value::<WebTerminalQuery>(json!({"id":"agent","pane":"private"})).is_err()
    );
}
#[test]
fn web_usage_unknown_is_not_zero_and_cache_reads_are_not_fresh_draw() {
    let mut measured = resource("measured", None);
    measured.usage.spent = Some(17);
    measured.usage.consumed = Some(117);
    measured.usage.drawn = 17;
    measured.usage.spend_period_key = Some("2026-05".into());
    let result = usage::project(page(vec![resource("unmeasured", None), measured])).unwrap();
    assert!(result["items"][0]["knownFreshTokens"].is_null());
    assert!(result["items"][0]["cacheReadTokens"].is_null());
    assert_eq!(result["items"][1]["cacheReadTokens"], 100);
    assert_eq!(result["totals"]["knownFreshTokens"], 17);
    assert_eq!(result["totals"]["measuredResourceCount"], 1);
    assert_eq!(result["totals"]["usageComplete"], false);
    assert!(result["items"][1]["busyMs"].is_null());
    assert!(result["items"][1]["tasks"].is_null());
    let empty = usage::project(page(vec![resource("empty", None)])).unwrap();
    assert!(empty["totals"]["knownFreshTokens"].is_null());
}
#[test]
fn web_shared_account_capacity_is_not_multiplied_by_resource_count() {
    let result = accounts::account_projection(
        account(2),
        vec![resource("first", Some(150)), resource("second", Some(150))],
        None,
    )
    .unwrap();
    assert_eq!(result["capacity"]["quotaTokens"], 150);
    assert_eq!(result["declaredByPeriod"]["monthly"], 200);
    assert_eq!(result["oversubscribedTokens"], 50);
    assert_eq!(result["aggregateComplete"], true);
    let partial = accounts::account_projection(
        account(3),
        vec![resource("first", Some(150))],
        Some("resource_a".into()),
    )
    .unwrap();
    assert!(partial["oversubscribedTokens"].is_null());
    assert_eq!(partial["aggregateComplete"], false);
    let unknown =
        accounts::account_projection(account(1), vec![resource("unknown", None)], None).unwrap();
    assert!(unknown["capacity"]["quotaTokens"].is_null());
    assert!(unknown["oversubscribedTokens"].is_null());
    assert!(
        !result.to_string().contains("shared_seat"),
        "private seat identity never leaves native"
    );
}
#[test]
fn web_usage_rejects_inconsistent_or_unrepresentable_measurement() {
    let mut bad = resource("bad", None);
    bad.usage.spent = Some(101);
    bad.usage.consumed = Some(100);
    assert!(usage::project(page(vec![bad])).is_err());
    let mut large = resource("large", None);
    large.usage.reserved = hagency_core::JSON_SAFE_MAX;
    let mut one = resource("one", None);
    one.usage.reserved = 1;
    assert!(usage::project(page(vec![large, one])).is_err());
}

#[test]
fn web_capability_card_uses_selected_model_not_aggregate_offer_qualification() {
    let mut offers=service::WebOfferPage {
        resource_id:"resource_low".into(),revision:"a".repeat(64),offers_editable:true,offer_editing_reason:None,
        resource_qualifications:vec!["documentation".into()],
        items:vec![
            service::WebRoleOffer {role:"coding".into(),qualified:true,catalog_published:true,..Default::default()},
            service::WebRoleOffer {role:"documentation".into(),qualified:false,catalog_published:false,..Default::default()},
        ],
    };
    qualify_resource_roles(&mut offers);
    assert!(!offers.items[0].qualified,"another owned strong model cannot qualify this lightweight model");
    assert!(offers.items[0].catalog_published,"aggregate offer publication remains separate");
    assert!(offers.items[1].qualified,"unpublished documentation capability is still model-qualified");
    assert!(!offers.items[1].catalog_published,"qualification does not advertise capacity");
}
