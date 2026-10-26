//! Current owner-runtime attachment adapter. No retired Fleet media authority.
use super::*;
use crate::console::native::MatrixTokenSource;
use hagency_agent_local::attachments::Attachment;
async fn available(
    host: &PinnedConsole,
    config: &StartConfig,
) -> Result<Option<Arc<dyn MatrixTokenSource>>, RuntimeError> {
    if runtime::kind(&config.credential_ref) != runtime::Kind::Codex
        || !(config.host_files || config.profile.programming.enabled)
    {
        return Ok(None);
    }
    let Some(source) = host
        .console
        .0
        .server_login
        .agent_file_source(&host.pin)
        .await
    else {
        return Ok(None);
    };
    let metadata = crate::server_admission::discover(&host.pin.origin)
        .await
        .map_err(|_| RuntimeError::Transport)?;
    Ok(metadata
        .capabilities
        .iter()
        .any(|c| c == hagency_agent_local::attachments::CAPABILITY)
        .then_some(source))
}
pub(super) async fn enabled<T: Transport>(
    transport: &T,
    config: &StartConfig,
) -> Result<bool, RuntimeError> {
    let Some(host) = transport.approval_host() else {
        return Ok(false);
    };
    Ok(available(&host, config).await?.is_some())
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn attach(
    session: &mut runtime::Session,
    host: &PinnedConsole,
    config: &StartConfig,
    lease: &LeaseRef,
    prepared: &inbox::Prepared,
    workspace: &std::path::Path,
    cancel: &watch::Receiver<bool>,
) -> Result<(), RuntimeError> {
    let Some(source) = available(host, config).await? else {
        return Ok(());
    };
    let response = host
        .execute(Op::Attachments {
            lease: lease.clone(),
            dispatch_id: prepared.dispatch.id.clone(),
            execution_id: prepared.execution_id.clone(),
        })
        .await?;
    let Response::Attachments(descriptors) = response else {
        return Err(RuntimeError::Transport);
    };
    let attachment = descriptors.attachments.into_iter().next();
    let bridge = Arc::new(LiveAttachmentHost {
        gate: LiveToolGate {
            console: host.clone(),
            lease: lease.clone(),
            expected: prepared.dispatch.clone(),
            execution: prepared.execution_id.clone(),
            cancel: cancel.clone(),
        },
        source,
        attachment: attachment.clone(),
    });
    session
        .enable_received_files(
            hagency_agent_local::received_files::Receiver::open(workspace)
                .map_err(|_| RuntimeError::Profile)?,
            bridge,
            prepared.dispatch.id.clone(),
            prepared.execution_id.clone(),
            prepared.dispatch.event_id.clone(),
            attachment,
        )
        .map_err(|_| RuntimeError::Profile)
}
struct LiveAttachmentHost {
    gate: LiveToolGate,
    source: Arc<dyn MatrixTokenSource>,
    attachment: Option<Attachment>,
}
impl codex::AttachmentHost for LiveAttachmentHost {
    fn authorize<'a>(
        &'a self,
        dispatch: &'a str,
        execution: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        Box::pin(async move {
            if !codex::ExecutionGate::authorize(&self.gate, dispatch, execution).await {
                return false;
            }
            // The metadata endpoint also makes a negative requester observation
            // permanent for this file request and compares the original source.
            let mut cancel = self.gate.cancel.clone();
            let response = operation(
                &self.gate.console,
                &mut cancel,
                Op::Attachments {
                    lease: self.gate.lease.clone(),
                    dispatch_id: dispatch.into(),
                    execution_id: execution.into(),
                },
            )
            .await;
            matches!(response,Ok(Response::Attachments(a)) if a.attachments==self.attachment.iter().cloned().collect::<Vec<_>>())
                && !*self.gate.cancel.borrow()
        })
    }
    fn download<'a>(
        &'a self,
        attachment: &'a Attachment,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, ()>> + Send + 'a>> {
        Box::pin(async move {
            if self.attachment.as_ref() != Some(attachment)
                || !codex::AttachmentHost::authorize(
                    self,
                    &self.gate.expected.id,
                    &self.gate.execution,
                )
                .await
            {
                return Err(());
            }
            let bytes = self
                .source
                .fetch_agent_attachment(attachment.clone())
                .await
                .map_err(|_| ())?;
            if bytes.len() > hagency_agent_local::received_files::MAX_BYTES
                || !codex::AttachmentHost::authorize(
                    self,
                    &self.gate.expected.id,
                    &self.gate.execution,
                )
                .await
            {
                return Err(());
            }
            Ok(bytes)
        })
    }
}
