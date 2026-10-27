//! Frozen file bytes are encrypted on the execution host; only ciphertext is uploaded.
use super::*;
use hagency_agent_local::{
    codex::FileDeliveryHost,
    file_delivery::{Delivery, Manifest},
    sent_files::Record,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Mutex as LocalMutex;
struct Buffer {
    receipt: Delivery,
    bytes: Option<Vec<u8>>,
}
pub(super) struct Host {
    pub gate: LiveToolGate,
    buffers: LocalMutex<BTreeMap<String, Buffer>>,
}
impl Host {
    pub(super) fn new(gate: LiveToolGate) -> Self {
        Self {
            gate,
            buffers: LocalMutex::new(BTreeMap::new()),
        }
    }
    async fn authorize(&self) -> Result<(), ()> {
        if !codex::ExecutionGate::authorize(
            &self.gate,
            &self.gate.expected.id,
            &self.gate.execution,
        )
        .await
        {
            return Err(());
        }
        Ok(())
    }
    fn original<'a>(&self, record: &'a Record) -> Result<&'a Delivery, ()> {
        let r = record.remote.as_ref().ok_or(())?;
        r.validate(
            &self.gate.expected.id,
            &self.gate.execution,
            &record.request_id,
            &record.manifest,
        )
        .map_err(|_| ())?;
        Ok(r)
    }
    fn receipt(&self, record: &Record, p: &Value) -> Result<Delivery, ()> {
        let mut d = self.original(record)?.clone();
        if p["dispatchId"] != d.dispatch_id
            || p["executionId"] != d.execution_id
            || p["requestId"] != d.request_id
            || p["kind"] != "file"
        {
            return Err(());
        }
        d.received_bytes = d.manifest.bytes;
        d.upload_accepted = true;
        d.state = if p["state"] == "sent" {
            "sent"
        } else {
            "unknown"
        }
        .into();
        d.matrix_event_id = p["matrixEventId"].as_str().map(str::to_owned);
        d.validate(&d.dispatch_id, &d.execution_id, &d.request_id, &d.manifest)
            .map_err(|_| ())?;
        Ok(d)
    }
}
impl codex::ExecutionGate for Host {
    fn authorize<'a>(
        &'a self,
        d: &'a str,
        e: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        codex::ExecutionGate::authorize(&self.gate, d, e)
    }
}
impl FileDeliveryHost for Host {
    fn prepare<'a>(
        &'a self,
        request: &'a str,
        file: &'a Manifest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Delivery, ()>> + Send + 'a>>
    {
        Box::pin(async move {
            self.authorize().await?;
            file.validate().map_err(|_| ())?;
            let mut buffers = self.buffers.lock().map_err(|_| ())?;
            if buffers.contains_key(request) {
                return Err(());
            }
            let id = format!("efile_{}", new_consent_id().map_err(|_| ())?);
            let d = Delivery {
                id,
                dispatch_id: self.gate.expected.id.clone(),
                execution_id: self.gate.execution.clone(),
                request_id: request.into(),
                created_at_ms: wall_ms(),
                manifest: file.clone(),
                manifest_digest: file.digest().map_err(|_| ())?,
                received_bytes: 0,
                state: "staging".into(),
                upload_accepted: false,
                matrix_event_id: None,
            };
            buffers.insert(
                request.into(),
                Buffer {
                    receipt: d.clone(),
                    bytes: Some(Vec::new()),
                },
            );
            Ok(d)
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
            self.authorize().await?;
            let r = self.original(record)?;
            let mut buffers = self.buffers.lock().map_err(|_| ())?;
            let b = buffers.get_mut(&r.request_id).ok_or(())?;
            if b.receipt.id != r.id {
                return Err(());
            }
            let data = b.bytes.as_mut().ok_or(())?;
            if offset != data.len() as u64
                || bytes.len() > hagency_agent_local::file_delivery::CHUNK_BYTES
                || data.len() + bytes.len() > r.manifest.bytes as usize
            {
                return Err(());
            }
            data.extend_from_slice(bytes);
            b.receipt.received_bytes = data.len() as u64;
            Ok(b.receipt.clone())
        })
    }
    fn queue<'a>(
        &'a self,
        record: &'a Record,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Delivery, ()>> + Send + 'a>>
    {
        Box::pin(async move {
            self.authorize().await?;
            let r = self.original(record)?;
            // Taking the original buffer is irreversible. A timeout/drop cannot upload it again.
            let data = {
                let mut buffers = self.buffers.lock().map_err(|_| ())?;
                let b = buffers.get_mut(&r.request_id).ok_or(())?;
                if b.receipt.id != r.id {
                    return Err(());
                }
                b.bytes.take().ok_or(())?
            };
            if data.len() as u64 != r.manifest.bytes
                || format!("{:x}", Sha256::digest(&data)) != r.manifest.sha256
            {
                return Err(());
            }
            let crypto = super::encrypted::crypto(&self.gate.console, &self.gate.expected.agent_id)
                .await
                .map_err(|_| ())?;
            self.authorize().await?;
            let cancel = self.gate.cancel.clone();
            let file = crypto
                .upload_encrypted_file(data, Arc::new(move || !*cancel.borrow()))
                .await
                .map_err(|_| ())?;
            self.authorize().await?;
            let mut content = json!({"msgtype":"m.file","body":r.manifest.caption.as_deref().unwrap_or(&r.manifest.filename),"filename":r.manifest.filename,"file":file,"info":{"size":r.manifest.bytes,"mimetype":"application/octet-stream"}});
            super::encrypted::relation(&mut content, &self.gate.expected);
            let p = super::encrypted::publish(
                &self.gate.console,
                &self.gate.lease,
                &self.gate.expected,
                &self.gate.execution,
                "file",
                &r.request_id,
                Some(content),
            )
            .await
            .map_err(|_| ())?;
            self.receipt(record, &p)
        })
    }
    fn status<'a>(
        &'a self,
        record: &'a Record,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Delivery, ()>> + Send + 'a>>
    {
        Box::pin(async move {
            let r = self.original(record)?;
            let p = super::encrypted::call(
                &self.gate.console,
                &self.gate.lease,
                &self.gate.expected.id,
                &self.gate.execution,
                "status",
                json!({"kind":"file","requestId":r.request_id}),
            )
            .await
            .map_err(|_| ())?;
            self.receipt(record, &p["publication"])
        })
    }
}
