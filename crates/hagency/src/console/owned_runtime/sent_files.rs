//! Original execution-device file delivery. All external IO uses the pinned
//! owner Console and fixed Server protocol; no model-supplied destination.
use super::super::device_execution::FileExpectation;
use super::*;
use hagency_agent_local::{
    file_delivery::{Delivery, Manifest, Target},
    sent_files::Record,
};
pub(super) async fn attach(
    session: &mut runtime::Session,
    host: &PinnedConsole,
    lease: &LeaseRef,
    prepared: &inbox::Prepared,
    workspace: &std::path::Path,
    cancel: &watch::Receiver<bool>,
) -> Result<(), RuntimeError> {
    let (ledger, owner) = super::encrypted::ledger(host).await?;
    if local(ledger.encrypted_input(&owner, &prepared.dispatch.id))?.is_some() {
        let bridge = super::encrypted_files::Host::new(LiveToolGate {
            console: host.clone(),
            lease: lease.clone(),
            expected: prepared.dispatch.clone(),
            execution: prepared.execution_id.clone(),
            cancel: cancel.clone(),
        });
        return session
            .enable_sent_files(
                hagency_agent_local::sent_files::Sender::open(workspace)
                    .map_err(|_| RuntimeError::Profile)?,
                Arc::new(bridge),
                prepared.dispatch.id.clone(),
                prepared.execution_id.clone(),
            )
            .map_err(|_| RuntimeError::Profile);
    }
    let metadata = crate::server_admission::discover(&host.pin.origin)
        .await
        .map_err(|_| RuntimeError::Transport)?;
    if !metadata
        .capabilities
        .iter()
        .any(|c| c == hagency_agent_local::file_delivery::CAPABILITY)
    {
        return Ok(());
    }
    session
        .enable_sent_files(
            hagency_agent_local::sent_files::Sender::open(workspace)
                .map_err(|_| RuntimeError::Profile)?,
            Arc::new(Host {
                gate: LiveToolGate {
                    console: host.clone(),
                    lease: lease.clone(),
                    expected: prepared.dispatch.clone(),
                    execution: prepared.execution_id.clone(),
                    cancel: cancel.clone(),
                },
            }),
            prepared.dispatch.id.clone(),
            prepared.execution_id.clone(),
        )
        .map_err(|_| RuntimeError::Profile)
}
struct Host {
    gate: LiveToolGate,
}
impl Host {
    fn expected(&self, record: &Record) -> Result<FileExpectation, ()> {
        let remote = record.remote.as_ref().ok_or(())?;
        Ok(FileExpectation {
            target: Target {
                dispatch_id: self.gate.expected.id.clone(),
                execution_id: self.gate.execution.clone(),
                file_id: remote.id.clone(),
            },
            request_id: record.request_id.clone(),
            file: record.manifest.clone(),
        })
    }
    async fn call(&self, op: Op) -> Result<Delivery, ()> {
        let mut cancel = self.gate.cancel.clone();
        let Response::FileDelivery(receipt) = operation(&self.gate.console, &mut cancel, op)
            .await
            .map_err(|_| ())?
        else {
            return Err(());
        };
        Ok(receipt)
    }
}
impl codex::ExecutionGate for Host {
    fn authorize<'a>(
        &'a self,
        dispatch: &'a str,
        execution: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        codex::ExecutionGate::authorize(&self.gate, dispatch, execution)
    }
}
impl codex::FileDeliveryHost for Host {
    fn prepare<'a>(
        &'a self,
        request: &'a str,
        file: &'a Manifest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Delivery, ()>> + Send + 'a>>
    {
        Box::pin(async move {
            self.call(Op::PrepareFile {
                lease: self.gate.lease.clone(),
                dispatch_id: self.gate.expected.id.clone(),
                execution_id: self.gate.execution.clone(),
                request_id: request.into(),
                file: file.clone(),
            })
            .await
        })
    }
    fn chunk<'a>(
        &'a self,
        record: &'a Record,
        offset: u64,
        bytes: &'a [u8],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Delivery, ()>> + Send + 'a>>
    {
        Box::pin(async move {
            self.call(Op::StageFileChunk {
                lease: self.gate.lease.clone(),
                expected: self.expected(record)?,
                offset,
                bytes: bytes.to_vec(),
            })
            .await
        })
    }
    fn queue<'a>(
        &'a self,
        record: &'a Record,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Delivery, ()>> + Send + 'a>>
    {
        Box::pin(async move {
            self.call(Op::QueueFile {
                lease: self.gate.lease.clone(),
                expected: self.expected(record)?,
            })
            .await
        })
    }
    fn status<'a>(
        &'a self,
        record: &'a Record,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Delivery, ()>> + Send + 'a>>
    {
        Box::pin(async move {
            self.call(Op::FileDelivery {
                expected: self.expected(record)?,
            })
            .await
        })
    }
}
