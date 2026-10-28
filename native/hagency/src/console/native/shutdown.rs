//! Local process closure is distinct from explicit withdrawal of remote consent.
use super::*;

impl NativeOwner {
    /// Immediately fence this owner and durably invalidate its saved native
    /// session before an account transition's first await. A failed write is
    /// reported; local capabilities remain retired regardless of that result.
    pub fn retire_and_revoke_resumption(&self) -> Result<(), NativeError> {
        self.retire();
        self.0
            .host
            .console
            .0
            .server_login
            .revoke_sdk_resume()
            .map_err(|_| failure(503, "native_logout_persistence_failed"))
    }

    /// Stop this process while preserving the original private SDK-backed
    /// native session. Restart still needs fresh SDK, owner and device proof.
    /// No runtime custody or local authority is restored by this operation.
    pub async fn suspend(&self) -> Result<(), NativeError> {
        self.retire();
        self.finish_close(false).await
    }

    /// Explicit logout/account withdrawal. Persist refusal before waiting for
    /// process cleanup, so a concurrent ordinary suspension cannot restore it.
    pub async fn shutdown_checked(&self) -> Result<(), NativeError> {
        let durable = self.retire_and_revoke_resumption();
        let cleanup = self.finish_close(true).await;
        durable.and(cleanup)
    }

    /// Compatibility cleanup for older hosts. Transition-capable hosts should
    /// use the checked API to display a failed durable logout or unknown drain.
    pub async fn shutdown(&self) {
        if let Err(error) = self.shutdown_checked().await {
            tracing::warn!(code = %error.code, "Native owner shutdown requires recovery");
        }
    }

    async fn finish_close(&self, revoke: bool) -> Result<(), NativeError> {
        let mut error = None;
        self.0.configuration_custody.close();
        // Clone retained handles, rather than remove them before the await.
        // Cancelled or concurrent close callers observe the original results.
        let tasks = match self.0.configuration_tasks.lock() {
            Ok(tasks) => tasks.clone(),
            Err(_) => {
                error = Some(failure(409, "native_configuration_drain_unknown"));
                Vec::new()
            }
        };
        let current = tokio::task::try_id();
        let mut called_from_configuration = false;
        for task in tasks {
            if current == Some(task.id()) {
                // A configuration task may discover SDK authorization loss.
                // It cannot await its own completion or claim that completion.
                called_from_configuration = true;
            } else if task.join().await.is_err() {
                error.get_or_insert_with(|| failure(409, "native_configuration_drain_unknown"));
            }
        }
        if called_from_configuration || self.0.configuration_custody.drain().await.is_err() {
            error.get_or_insert_with(|| failure(409, "native_configuration_drain_unknown"));
        }
        if self.0.host.console.0.authority.revoke_all().is_err() {
            error.get_or_insert_with(|| failure(503, "native_local_authority_close_failed"));
        }
        let console = &self.0.host.console;
        let login = &console.0.server_login;
        if revoke {
            login.revoke_native_all().await;
        }
        let (runtime, provider, shared, managed, accounts) = tokio::join!(
            console.0.owned_runtime.drain_for_migration(),
            console.0.owner_provider.drain_for_migration(),
            self.0.shared_execution.drain(),
            self.0.managed_runtime.drain(),
            self.0.account_logins.drain(),
        );
        for (result, code) in [
            (runtime, "native_runtime_drain_unknown"),
            (provider, "native_provider_drain_unknown"),
            (shared, "native_shared_execution_drain_unknown"),
            (managed, "native_managed_runtime_drain_unknown"),
            (accounts, "native_account_login_drain_unknown"),
        ] {
            if result.is_err() {
                error.get_or_insert_with(|| failure(409, code));
            }
        }
        if let Err(failure) = self.0.management.drain().await {
            error.get_or_insert(failure);
        }
        // A previous explicit withdrawal remains pending across normal close;
        // suspension never discards it or recreates a resumable credential.
        login.flush_native_revocations().await;
        self.retire();
        if let Some(task) = self.0.callback_task.lock().unwrap().take() {
            task.abort();
        }
        error.map_or(Ok(()), Err)
    }
}
