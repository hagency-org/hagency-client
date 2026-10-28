use super::*;
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Receipt {
    version: u16,
    kind: String,
    migration_id: String,
    registry_id: String,
    profile_digest: String,
    owner_mxid: String,
    plan_digest: String,
    inventory_digest: String,
    source_schema: i32,
    source_evidence: String,
    custody_evidence: String,
    history: Summary,
}
fn manifest(plan: &Plan, kind: &str) -> Value {
    json!({"version":1,"kind":kind,"migrationId":plan.migration_id,"registryId":plan.registry_id,
        "profileDigest":plan.inventory.profile,"ownerMxid":plan.inventory.owner,
        "inventoryDigest":plan.inventory_digest,"sourceSchema":plan.inventory.source_schema})
}
pub(super) fn expected(plan: &Plan) -> Result<Receipt, Error> {
    let history = plan
        .inventory
        .task_history
        .clone()
        .ok_or(Error::Quarantined)?;
    if !matches!(plan.version, 2 | 3) {
        return Err(Error::Quarantined);
    }
    history.validate()?;
    let digest = plan.digest()?;
    Ok(Receipt {
        version: 2,
        kind: "operator_task_transfer_v2".into(),
        migration_id: plan.migration_id.clone(),
        registry_id: plan.registry_id.clone(),
        profile_digest: plan.inventory.profile.clone(),
        owner_mxid: plan.inventory.owner.clone(),
        plan_digest: digest.clone(),
        inventory_digest: plan.inventory_digest.clone(),
        source_schema: plan.inventory.source_schema,
        source_evidence: canonical::digest(
            &json!({"plan":digest,"management":manifest(plan,"management"),"personal":manifest(plan,"personal")}),
        )?,
        custody_evidence: canonical::digest(
            &json!({"plan":digest,"custody":plan.inventory.custody_digest,"count":plan.inventory.custody_rows}),
        )?,
        history,
    })
}
pub(in super::super) fn saved(db: &Connection, plan: &Plan) -> Result<(), Error> {
    let (migration,digest,raw):(String,String,String)=db.query_row("SELECT migration_id,receipt_digest,CASE WHEN length(CAST(receipt AS BLOB))<=8192 THEN receipt END FROM owner_operator_task_histories WHERE profile=?1",[&plan.inventory.profile],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let actual: Receipt = serde_json::from_str(&raw)?;
    if migration != plan.migration_id
        || actual != expected(plan)?
        || canonical::digest(&serde_json::to_value(actual)?)? != digest
    {
        return Err(Error::Quarantined);
    }
    Ok(())
}
/// Current active original adoption, not the import receipt alone, authorizes
/// canonical reads/writes. This never opens a source or recreates missing data.
#[cfg(feature = "resource-adoption")]
pub(in super::super) fn authorize_active(
    db: &Connection,
    authority: &AdoptionAuthority,
    original: &Plan,
) -> Result<(), Error> {
    authority.check()?;
    let (plan, phase) = writer::read(db, authority, &original.migration_id)?;
    if phase != 4
        || plan.digest()? != original.digest()?
        || canonical::digest(&serde_json::to_value(&plan.inventory)?)? != plan.inventory_digest
    {
        return Err(Error::LocalAuthority);
    }
    let receipt = expected(&plan)?;
    saved(db, &plan)?;
    let (source,custody,result):(String,String,String)=db.query_row("SELECT source_evidence,custody_evidence,CASE WHEN length(CAST(result AS BLOB))<=262144 THEN result END FROM owner_resource_adoptions WHERE id=?1",[&plan.migration_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    if source != receipt.source_evidence
        || custody != receipt.custody_evidence
        || serde_json::from_str::<Value>(&result)? != serde_json::to_value(plan.summary(4)?)?
    {
        return Err(Error::Quarantined);
    }
    authority.check()
}
