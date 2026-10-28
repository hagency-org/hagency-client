use super::*;

fn record() -> EmptyDevicePrepare {
    EmptyDevicePrepare {
        source_binding: "a".repeat(64),
        plan_digest: "b".repeat(64),
        inventory_digest: "c".repeat(64),
        personal_database_present: false,
        completed_private_records: 0,
    }
}
fn descriptor() -> Descriptor {
    Descriptor {
        version: 3,
        registry_id: "d".repeat(64),
        migration_id: "original_setup".into(),
        origin: "https://matrix.test/".into(),
        owner: "@owner:matrix.test".into(),
        profile: "e".repeat(64),
        original_device: "original_device".into(),
        task_prepare: None,
        empty_prepare: Some(record()),
    }
}
#[test]
fn empty_descriptor_roundtrips_without_provider_or_quota_and_cannot_replace_original() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("private");
    hagency_store::private::directory(&state).unwrap();
    let value = descriptor();
    let bytes = serde_json::to_vec(&value).unwrap();
    task_prepare::persist_new(&state, &bytes).unwrap();
    assert!(read_descriptor(&state).unwrap().unwrap() == value);
    let object = serde_json::to_value(value).unwrap();
    assert!(object.get("taskPrepare").is_none());
    assert!(object["emptyPrepare"].get("monthlyAccountTokens").is_none());
    let mut replacement = descriptor();
    replacement.migration_id = "replacement".into();
    assert!(task_prepare::persist_new(&state, &serde_json::to_vec(&replacement).unwrap()).is_err());
    assert_eq!(std::fs::read(state.join(INITIALIZER_FILE)).unwrap(), bytes);
}
#[test]
fn empty_descriptor_rejects_ambiguous_kinds_unknown_fields_and_invalid_digests() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("private");
    hagency_store::private::directory(&state).unwrap();
    for case in ["wrong_version", "mixed", "missing", "quota", "uppercase"] {
        let mut v = serde_json::to_value(descriptor()).unwrap();
        match case {
            "wrong_version" => v["version"] = serde_json::json!(2),
            "mixed" => {
                v["taskPrepare"] = serde_json::json!({"sourceBinding":"a".repeat(64),"planDigest":"b".repeat(64),"inventoryDigest":"c".repeat(64),"monthlyAccountTokens":700})
            }
            "missing" => {
                v.as_object_mut().unwrap().remove("emptyPrepare");
            }
            "quota" => v["emptyPrepare"]["monthlyAccountTokens"] = serde_json::json!(0),
            "uppercase" => v["emptyPrepare"]["planDigest"] = serde_json::json!("B".repeat(64)),
            _ => unreachable!(),
        }
        hagency_store::private::replace(
            &state.join(INITIALIZER_FILE),
            &serde_json::to_vec(&v).unwrap(),
        )
        .unwrap();
        assert!(read_descriptor(&state).is_err(), "case {case}");
    }
}
#[test]
fn older_initializer_serialization_does_not_gain_empty_setup_fields() {
    for version in [1, 2] {
        let mut v = descriptor();
        v.version = version;
        v.empty_prepare = None;
        if version == 2 {
            v.task_prepare = Some(TaskAdoptionPrepare {
                source_binding: "a".repeat(64),
                plan_digest: "b".repeat(64),
                inventory_digest: "c".repeat(64),
                monthly_account_tokens: 700,
                review: None,
            });
        }
        let encoded = serde_json::to_value(v).unwrap();
        assert!(encoded.get("emptyPrepare").is_none());
        let old_bytes = serde_json::to_vec(&encoded).unwrap();
        let decoded: Descriptor = serde_json::from_slice(&old_bytes).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
    }
}
