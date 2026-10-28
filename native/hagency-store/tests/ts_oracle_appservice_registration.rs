//! TS test oracle — `tests/appservice-receiver.test.js` (case group
//! `the registration we hand to a project side`).
//!
//! Each test below asserts the SAME observable outcome the TS case asserts,
//! against the native issuer (`DomainRepository::issue_side_registration`,
//! `hagency-store/src/domain/side_registration.rs`), which ports
//! `generateRegistration` (`appservice-receiver.js:70-110`) and
//! `renderRegistrationYaml` (`appservice-receiver.js:111-136`).
//!
//! The TS case asserts on the returned registration object; the native issuer
//! never hands the object out (ADR-132: the route returns fingerprints, tokens
//! stay on disk). The oracle therefore reads the artefact that IS the TS
//! object's rendered form — the registration YAML at the issuer's own `path`.
use hagency_core::authority::Registration;
use hagency_store::{DomainRepository, IssueSideRegistrationRequest};
use std::path::Path;

fn registration() -> Registration {
    let fleet = format!("hf_{}", "a".repeat(32));
    Registration {
        fleet_id: fleet.clone(),
        generation: 1,
        server_name: "example.test".into(),
        reception_room_id: "!reception:example.test".into(),
        representative_mxid: format!("@{fleet}_representative:example.test"),
        approval_bot_mxid: "@approval:example.test".into(),
    }
}

fn request(url: &str) -> IssueSideRegistrationRequest {
    IssueSideRegistrationRequest {
        side: "example.test".into(),
        url: url.into(),
        registration_id: None,
        sender_localpart: None,
        user_namespace: None,
        exclusive: None,
    }
}

fn repo() -> (tempfile::TempDir, DomainRepository) {
    let dir = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&dir.path().join("state")).unwrap();
    db.register(&registration()).unwrap();
    (dir, db)
}

/// The YAML key's value, unquoted — the same field `yaml_field` in the console
/// oracle reads. The YAML is written by both callers of the issuer.
fn yaml_field(yaml: &str, key: &str) -> String {
    yaml.lines()
        .find_map(|l| l.trim_start().strip_prefix(&format!("{key}: ")))
        .expect("key present in the YAML")
        .trim()
        .trim_matches('"')
        .to_owned()
}

fn issue(url: &str) -> (tempfile::TempDir, String) {
    let (dir, mut db) = repo();
    let issued = db.issue_side_registration(&request(url), 1000).unwrap();
    let yaml = std::fs::read_to_string(&issued.path).unwrap();
    (dir, yaml)
}

/// TS: `tokens are RANDOM and distinct, never derived` —
/// `a.as_token !== a.hs_token`, two issues differ in both tokens, both are 64
/// hex characters (hex, not base64: the hs_token travels in a query string).
#[test]
fn ts_tokens_are_random_and_distinct_never_derived() {
    let (_a, first) = issue("https://us.example");
    let (_b, second) = issue("https://us.example");
    let as1 = yaml_field(&first, "as_token");
    let hs1 = yaml_field(&first, "hs_token");
    let as2 = yaml_field(&second, "as_token");
    assert_ne!(
        as1, hs1,
        "as_token and hs_token authorise opposite directions"
    );
    assert_ne!(as1, as2, "two issues must not derive the same as_token");
    assert_ne!(hs1, yaml_field(&second, "hs_token"));
    for token in [&as1, &hs1] {
        assert_eq!(token.len(), 64, "32 bytes, hex");
        assert!(
            token
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "hex, not base64: {token}"
        );
    }
}

/// TS: `it carries every field a homeserver needs` — the object's key set is
/// exactly these seven, and the trailing slash is trimmed from the url.
#[test]
fn ts_registration_carries_every_field_a_homeserver_needs() {
    let (_dir, yaml) = issue("https://us.example/");
    for key in [
        "id",
        "url",
        "as_token",
        "hs_token",
        "sender_localpart",
        "rate_limited",
        "namespaces",
    ] {
        assert!(
            yaml.contains(&format!("\n{key}:")),
            "missing {key} in:\n{yaml}"
        );
    }
    assert_eq!(yaml_field(&yaml, "url"), "https://us.example");
}

/// TS: `sender_localpart is lowercased, because the spec requires it of
/// localparts` — input `Hagency` renders `hagency`.
#[test]
fn ts_sender_localpart_is_lowercased() {
    let (_dir, mut db) = repo();
    let issued = db
        .issue_side_registration(
            &IssueSideRegistrationRequest {
                sender_localpart: Some("Hagency".into()),
                ..request("https://u.example")
            },
            1000,
        )
        .unwrap();
    let yaml = std::fs::read_to_string(&issued.path).unwrap();
    assert_eq!(yaml_field(&yaml, "sender_localpart"), "hagency");
}

/// TS: `the default namespace matches the existing agent prefix rather than
/// changing it` — `@ac_.*`.
#[test]
fn ts_default_user_namespace_matches_the_agent_prefix() {
    let (_dir, yaml) = issue("https://u.example");
    assert_eq!(yaml_field(&yaml, "regex"), "@ac_.*");
}

/// TS: `id and url are required` — a missing url is a refusal that names it.
#[test]
fn ts_url_is_required() {
    let (_dir, mut db) = repo();
    let error = db
        .issue_side_registration(&request("   "), 1000)
        .expect_err("an empty url is refused");
    assert!(
        error.to_string().contains("url is required"),
        "the refusal names the missing field: {error}"
    );
}

/// TS: `id and url are required` (the id half) — an explicitly empty
/// `registration_id` is refused rather than defaulted.
#[test]
fn ts_registration_id_is_refused_when_supplied_empty() {
    let (_dir, mut db) = repo();
    let error = db
        .issue_side_registration(
            &IssueSideRegistrationRequest {
                registration_id: Some(String::new()),
                ..request("https://u.example")
            },
            1000,
        )
        .expect_err("an empty registration id is refused");
    assert!(
        error.to_string().contains("registration id"),
        "the refusal names the field: {error}"
    );
}

/// TS: `the YAML round-trips through the fields a homeserver parses`.
#[test]
fn ts_yaml_round_trips_the_fields_a_homeserver_parses() {
    let (_dir, yaml) = issue("https://us.example");
    assert_eq!(yaml_field(&yaml, "id"), "hagency-example.test");
    assert_eq!(yaml_field(&yaml, "url"), "https://us.example");
    assert_eq!(yaml_field(&yaml, "sender_localpart"), "hagency");
    assert!(yaml.contains("regex: \"@ac_.*\""), "{yaml}");
    assert!(yaml.contains("aliases: []"), "{yaml}");
    assert!(yaml.contains("rooms: []"), "{yaml}");
    assert_eq!(yaml_field(&yaml, "rate_limited"), "false");
    // The tokens the YAML carries are the ones the homeserver will use.
    assert_eq!(yaml_field(&yaml, "as_token").len(), 64);
    assert_eq!(yaml_field(&yaml, "hs_token").len(), 64);
}

/// TS: `the YAML says the homeserver must be restarted` — Palpo loads
/// registrations into a OnceCell, so the file names the restart.
#[test]
fn ts_yaml_says_the_homeserver_must_be_restarted() {
    let (_dir, yaml) = issue("https://u.example");
    assert!(
        yaml.to_lowercase().contains("restart"),
        "the YAML must name the restart: {yaml}"
    );
}

/// TS: `user namespace is exclusive by default and overridable` — a
/// non-exclusive namespace lets a bystander register an `@ac_*` user and speak
/// as an agent, so `true` is the default and `false` is an explicit opt-in.
#[test]
fn ts_user_namespace_is_exclusive_by_default_and_overridable() {
    let (_default_dir, default_yaml) = issue("https://u.example");
    assert!(
        default_yaml.contains("- exclusive: true"),
        "exclusive is the default: {default_yaml}"
    );
    let (_dir, mut db) = repo();
    let issued = db
        .issue_side_registration(
            &IssueSideRegistrationRequest {
                exclusive: Some(false),
                ..request("https://u.example")
            },
            1000,
        )
        .unwrap();
    let yaml = std::fs::read_to_string(&issued.path).unwrap();
    assert!(
        yaml.contains("- exclusive: false"),
        "an explicit false is honoured: {yaml}"
    );
}

/// TS: `it carries every field a homeserver needs` (the url rule) and the
/// renderer's `url.replace(/\/+$/, '')` — any number of trailing slashes is
/// trimmed, not doubled.
#[test]
fn ts_trailing_slash_is_trimmed_from_the_url() {
    let (_dir, yaml) = issue("http://127.0.0.1:13443///");
    assert_eq!(yaml_field(&yaml, "url"), "http://127.0.0.1:13443");
}

/// The issuer writes the YAML at the TS-declared side-file name
/// (`backend-v2.js:10091`: `side.id.replace(/[^\w.-]/g, '_')`).
#[test]
fn ts_registration_file_is_named_for_the_side() {
    let (dir, mut db) = repo();
    let issued = db
        .issue_side_registration(&request("https://u.example"), 1000)
        .unwrap();
    let expected = dir.path().join("state/registrations/example.test.yaml");
    assert!(
        expected.exists(),
        "the YAML is on disk at the reported path"
    );
    // The store reports the canonical host path; macOS spells the temporary
    // directory through the /var -> /private/var alias, so both sides are
    // compared as the same canonical file.
    assert_eq!(
        Path::new(&issued.path).canonicalize().unwrap(),
        expected.canonicalize().unwrap()
    );
}
