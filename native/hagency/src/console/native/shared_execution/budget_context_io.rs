//! Context input on exact v3 work routes; historical descriptors keep their original bytes.
use super::*;
use super::context_io::ContextAction;
use super::budget_work_io::WorkAction;
async fn paired(b: &Bound) -> Result<bool, NativeError> {
    let d = b.demand_discovery().await?;
    if d.capabilities.len() > 64 || d.capabilities.iter().any(|c| c.len() > 128) {
        return Err(invalid());
    }
    Ok(d.capabilities
        .iter()
        .any(|c| c == SHARED_CONTEXT_CAPABILITY))
}
pub(super) async fn prepare(
    b: &Bound,
    r: &OwnerResourceRegistry,
    a: VerifiedSharedActivity,
    w: &SharedWorkItem,
    authority: SharedActivityRef,
    until: Instant,
) -> Result<SharedPreparedInput, NativeError> {
    if !paired(b).await? {
        return r
            .prepare_shared_legacy_input(a, w.clone())
            .await
            .map_err(store);
    }
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
