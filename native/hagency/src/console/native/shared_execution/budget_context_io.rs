//! Context input on exact v3 work routes; historical descriptors keep their original bytes.
use super::*;
use super::context_io::ContextAction;
use super::budget_work_io::WorkAction;
/// The highest context record version the endpoint advertises, if any.
/// Version 2 implies version 1 routes; a lone version 2 flag is not trusted.
async fn paired(b: &Bound) -> Result<Option<u16>, NativeError> {
    let d = b.demand_discovery().await?;
    if d.capabilities.len() > 64 || d.capabilities.iter().any(|c| c.len() > 128) {
        return Err(invalid());
    }
    let has = |name: &str| d.capabilities.iter().any(|c| c == name);
    Ok(match (has(SHARED_CONTEXT_CAPABILITY), has(SHARED_CONTEXT_CAPABILITY_V2)) {
        (true, true) => Some(2),
        (true, false) => Some(1),
        _ => None,
    })
}
/// A version 2 freeze names the model window this Agent last reported so the
/// server can budget history against it. Nothing here grants model custody.
async fn budget(
    r: &OwnerResourceRegistry,
    a: &VerifiedSharedActivity,
    version: u16,
) -> Result<Option<SharedContextBudgetInput>, NativeError> {
    if version < 2 {
        return Ok(None);
    }
    Ok(Some(SharedContextBudgetInput {
        version: 2,
        model_context_window: r.shared_context_window(a).await.map_err(store)?,
    }))
}
pub(super) async fn prepare(
    io: &Io,
    b: &Bound,
    r: &OwnerResourceRegistry,
    a: VerifiedSharedActivity,
    w: &SharedWorkItem,
    authority: SharedActivityRef,
    until: Instant,
) -> Result<SharedPreparedInput, NativeError> {
    if w.encrypted {
        let text=super::e2ee::input(io,b,w).await?;
        return r.prepare_shared_decrypted_input(a,w.clone(),text).await.map_err(store);
    }
    let Some(version) = paired(b).await? else {
        return r
            .prepare_shared_legacy_input(a, w.clone())
            .await
            .map_err(store);
    };
    let budget = budget(r, &a, version).await?;
    let mode: Timed<SharedWorkInputMode> = b
        .upload(
            WorkAction::Context(Box::new(ContextAction::Mode {
                work: w.id.clone(),
                input: SharedWorkEvidenceInput {
                    work_digest: digest(w)?,
                    runtime_command_digest: w.runtime_command_digest.clone(),
                },
            }))
            .into(),
            until,
        )
        .await?;
    mode.value.validate().map_err(store)?;
    let mode = mode.value;
    let context = if mode.mode == "thread_context_v1" {
        let action = if let Some(hash) = &mode.context_digest {
            ContextAction::Recover {
                work: w.id.clone(),
                input: SharedContextRecoverInput {
                    work_digest: digest(w)?,
                    runtime_command_digest: w.runtime_command_digest.clone(),
                    context_digest: hash.clone(),
                },
            }
        } else {
            ContextAction::Freeze {
                work: w.id.clone(),
                input: SharedContextInput {
                    authority,
                    work_digest: digest(w)?,
                    budget,
                },
            }
        };
        let c: Timed<SharedContextRecord> = b.upload(WorkAction::Context(Box::new(action)).into(), until).await?;
        c.value.render_for(w).map_err(store)?;
        Some(c.value)
    } else {
        None
    };
    r.prepare_shared_input(a, w.clone(), mode, context)
        .await
        .map_err(store)
}
pub(super) async fn start(
    b: &Bound,
    r: &OwnerResourceRegistry,
    attempt: SharedStartAttempt,
    w: &SharedWorkItem,
    input: Option<&SharedPreparedInput>,
    authority: SharedActivityRef,
    until: Instant,
) -> Result<SharedCallCustody, NativeError> {
    if let Some(c) = input.and_then(SharedPreparedInput::context) {
        let start: Timed<SharedContextStart> = b
            .upload(
                WorkAction::Context(Box::new(ContextAction::Start {
                    work: w.id.clone(),
                    input: SharedContextStartInput {
                        authority,
                        work_digest: digest(w)?,
                        context_digest: c.context_digest.clone(),
                        input_digest: c.input_digest.clone(),
                    },
                }))
                .into(),
                until,
            )
            .await?;
        r.issue_shared_context_call(attempt, start.value)
            .await
            .map_err(store)
    } else {
        let start: Timed<SharedWorkStart> = b
            .upload(
                WorkAction::Start(SharedWorkInput {
                    authority,
                    work_id: w.id.clone(),
                    work_digest: digest(w)?,
                }).into(),
                until,
            )
            .await?;
        r.issue_shared_call(attempt, start.value)
            .await
            .map_err(store)
    }
}
