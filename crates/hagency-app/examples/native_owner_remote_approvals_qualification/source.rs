//! Qualification pinning around the SAME desktop SDK transport functions.
use hagency::native_owner::{
    remote_approval::{
        Publication, SendPermit, ObservedVerdict,
        review::{ObservedRequest, RequestPage, VerdictSendPermit},
    },
    MatrixAccessToken, MatrixTokenSource, NativeError,
};
use hagency_app::hagency::backend;
use matrix_sdk::Client;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
pub struct Source {
    pub client: Client,
    pub origin: String,
    pub owner: String,
    pub device: String,
    pub active: Arc<AtomicBool>,
}
impl Source {
    pub fn new(client: Client, origin: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            owner: client.user_id().ok_or("SDK user")?.to_string(),
            device: client.device_id().ok_or("SDK device")?.to_string(),
            client,
            origin: origin.into(),
            active: Arc::new(AtomicBool::new(true)),
        })
    }
    pub fn check(&self) -> Result<(), NativeError> {
        if !self.active.load(Ordering::Acquire)
            || self.client.homeserver().as_str() != self.origin
            || self
                .client
                .user_id()
                .is_none_or(|u| u.as_str() != self.owner)
            || self
                .client
                .device_id()
                .is_none_or(|d| d.as_str() != self.device)
        {
            return Err(NativeError {
                status: 401,
                code: "qualification_sdk_scope_changed".into(),
            });
        }
        Ok(())
    }
    fn live(&self, live: &(dyn Fn() -> bool + Send + Sync)) -> Result<(), NativeError> {
        self.check()?;
        if !live() {
            return Err(NativeError {
                status: 401,
                code: "qualification_original_retired".into(),
            });
        }
        Ok(())
    }
}
impl MatrixTokenSource for Source {
    fn access_token(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<MatrixAccessToken, NativeError>> + Send + '_>,
    > {
        Box::pin(async {
            self.check()?;
            let who = self.client.whoami().await.map_err(|_| NativeError {
                status: 401,
                code: "qualification_sdk_whoami".into(),
            })?;
            self.check()?;
            if who.user_id.as_str() != self.owner
                || who.device_id.as_deref() != self.client.device_id()
            {
                return Err(NativeError {
                    status: 401,
                    code: "qualification_sdk_whoami_scope".into(),
                });
            }
            Ok(MatrixAccessToken {
                access_token: self.client.access_token().ok_or_else(|| NativeError {
                    status: 401,
                    code: "qualification_sdk_token".into(),
                })?,
                client_id: "sdk-client".into(),
            })
        })
    }
    fn supports_remote_approvals(&self) -> bool {
        true
    }
    fn prepare_remote_approval<'a>(
        &'a self,
        room: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, NativeError>> + Send + 'a>>
    {
        Box::pin(async move {
            self.access_token().await?;
            backend::prepare_remote_approval(&self.client, &self.origin, &self.owner, room, || {
                self.check()
            })
            .await
        })
    }
    fn publish_remote_approval(
        &self,
        permit: SendPermit,
        live: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, NativeError>> + Send + '_>>
    {
        Box::pin(async move {
            self.access_token().await?;
            backend::publish_remote_approval(&self.client, &self.origin, permit, || {
                self.live(&*live)
            })
            .await
        })
    }
    fn observe_remote_approval<'a>(
        &'a self,
        publication: &'a Publication,
        request_event: &'a str,
        verdict_event: &'a str,
        live: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ObservedVerdict, NativeError>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.access_token().await?;
            backend::observe_remote_approval_verdict(
                &self.client,
                &self.origin,
                publication,
                request_event,
                verdict_event,
                || self.live(&*live),
            )
            .await
        })
    }
    fn find_remote_approval<'a>(
        &'a self,
        publication: &'a Publication,
        request_event: &'a str,
        live: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ObservedVerdict, NativeError>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.access_token().await?;
            backend::find_remote_approval(
                &self.client,
                &self.origin,
                publication,
                request_event,
                || self.live(&*live),
            )
            .await
        })
    }
    fn list_remote_requests<'a>(
        &'a self,
        room: &'a str,
        cursor: Option<&'a str>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<RequestPage, NativeError>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.access_token().await?;
            backend::list_remote_approval_requests(
                &self.client,
                &self.origin,
                &self.owner,
                room,
                cursor,
                || self.check(),
            )
            .await
        })
    }
    fn inspect_remote_request<'a>(
        &'a self,
        room: &'a str,
        event: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ObservedRequest, NativeError>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.access_token().await?;
            backend::inspect_remote_approval_request(
                &self.client,
                &self.origin,
                &self.owner,
                room,
                event,
                || self.check(),
            )
            .await?
            .ok_or_else(|| NativeError {
                status: 409,
                code: "qualification_not_a_request".into(),
            })
        })
    }
    fn publish_remote_verdict(
        &self,
        permit: VerdictSendPermit,
        live: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, NativeError>> + Send + '_>>
    {
        Box::pin(async move {
            self.access_token().await?;
            backend::publish_remote_approval_verdict(&self.client, &self.origin, permit, || {
                self.live(&*live)
            })
            .await
        })
    }
}
