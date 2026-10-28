//! Finite original-attempt supervision lives with the native owner, not the view.
use super::{NativeManagement, NativeError, failure, management::store_error};
use crate::console::{
    lifecycle::{TaskCustody, DrainFailure},
    owner_provider::managed::{NamespaceLogin, LoginState},
};
use hagency_store::management::{
    AccountLoginHandle, AccountLoginRequest, AccountLoginStatus, HostContext, Permission, Intent,
    IntentOperation,
};
use hagency_store::{LoginVerdict, LoginOutcome, AccountReadinessMode};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Instant, Duration},
};

/// Native-only page opener. No Serialize, Debug or conversion to a script reply.
pub struct AccountLoginAuthorization(String);
impl AccountLoginAuthorization {
    pub fn into_url(self) -> String {
        self.0
    }
}
struct Snapshot {
    status: AccountLoginStatus,
    url: Option<String>,
    running: bool,
}
struct Attempt {
    context: HostContext,
    snapshot: Mutex<Snapshot>,
    cancel: AtomicBool,
}
#[derive(Default)]
pub(super) struct Manager {
    #[cfg(test)]
    pub(super) executable: Mutex<Option<std::path::PathBuf>>,
    tasks: TaskCustody,
    entries: Mutex<BTreeMap<String, Arc<Attempt>>>,
}
fn key(input: &AccountLoginRequest) -> Result<String, NativeError> {
    hagency_core::canonical::digest(&serde_json::json!({"input":input}))
        .map_err(|_| failure(400, "invalid_management_arguments"))
}
impl Manager {
    pub fn close(&self) {
        self.tasks.close();
        if let Ok(entries) = self.entries.lock() {
            for entry in entries.values() {
                entry.cancel.store(true, Ordering::Release);
            }
        }
    }
    pub async fn drain(&self) -> Result<(), DrainFailure> {
        self.close();
        self.tasks.drain().await
    }
    fn get(
        &self,
        context: &HostContext,
        input: &AccountLoginRequest,
    ) -> Result<Option<Arc<Attempt>>, NativeError> {
        Ok(self
            .entries
            .lock()
            .map_err(|_| failure(503, "management_unavailable"))?
            .get(&key(input)?)
            .filter(|entry| entry.context == *context)
            .cloned())
    }
    fn begin(
        &self,
        manager: Arc<NativeManagement>,
        context: HostContext,
        input: AccountLoginRequest,
        handle: AccountLoginHandle,
    ) -> Result<(), NativeError> {
        let activity = self
            .tasks
            .enter()
            .map_err(|_| failure(409, "management_outcome_unknown"))?;
        let key = key(&input)?;
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| failure(503, "management_unavailable"))?;
        self.tasks
            .check_open()
            .map_err(|_| failure(409, "management_outcome_unknown"))?;
        if entries.contains_key(&key) {
            return Err(failure(409, "management_outcome_unknown"));
        }
        entries.retain(|_, entry| entry.snapshot.lock().map_or(true, |s| s.running));
        if entries.len() >= 16 {
            return Err(failure(503, "management_capacity"));
        }
        let entry = Arc::new(Attempt {
            context: context.clone(),
            snapshot: Mutex::new(Snapshot {
                status: AccountLoginStatus {
                    command_id: input.command_id.clone(),
                    account_id: input.account_id.clone(),
                    state: "starting".into(),
                },
                url: None,
                running: true,
            }),
            cancel: AtomicBool::new(false),
        });
        entries.insert(key, entry.clone());
        let tasks = self.tasks.clone();
        self.tasks.cleanup(async move {
            let _activity = activity;
            run(manager, context, input, handle, entry, tasks).await
        });
        Ok(())
    }
}
impl NativeManagement {
    pub async fn login_account(
        self: &Arc<Self>,
        input: AccountLoginRequest,
    ) -> Result<AccountLoginStatus, NativeError> {
        input.validate().map_err(store_error)?;
        let (context, session) = self.authorized().await?;
        session
            .authorize(&context, Permission::LoginAccounts)
            .map_err(store_error)?;
        let intent = Intent::new(
            IntentOperation::LoginAccount,
            input.command_id.clone(),
            &input,
        )
        .map_err(store_error)?;
        self.with_intent(intent, async {
            let (context, session) = self.authorized().await?;
            let reservation = session
                .begin_account_login(&context, input.clone())
                .await
                .map_err(store_error)?;
            if let Some(handle) = reservation.handle {
                self.owner
                    .0
                    .account_logins
                    .begin(self.clone(), context, input.clone(), handle)?;
            }
            self.account_login_status(input).await
        })
        .await
    }
    async fn login_guard(
        &self,
        original: &HostContext,
        input: &AccountLoginRequest,
    ) -> Result<AccountLoginStatus, NativeError> {
        let (context, session) = self.authorized().await?;
        if &context != original {
            return Err(failure(401, "management_scope_changed"));
        }
        let result = session.account_login_status(&context, input.clone()).await;
        self.finish(&context, result, false).await
    }
    pub async fn account_login_status(
        &self,
        input: AccountLoginRequest,
    ) -> Result<AccountLoginStatus, NativeError> {
        let (context, session) = self.authorized().await?;
        let result = session.account_login_status(&context, input.clone()).await;
        let mut status = self.finish(&context, result, false).await?;
        if let Some(entry) = self.owner.0.account_logins.get(&context, &input)? {
            let snapshot = entry
                .snapshot
                .lock()
                .map_err(|_| failure(503, "management_unavailable"))?;
            if snapshot.running && status.state == "uncertain" {
                status = snapshot.status.clone();
            }
        }
        Ok(status)
    }
    pub async fn cancel_account_login(
        &self,
        input: AccountLoginRequest,
    ) -> Result<AccountLoginStatus, NativeError> {
        let mut status = self.account_login_status(input.clone()).await?;
        let (context, _) = self.authorized().await?;
        if let Some(entry) = self.owner.0.account_logins.get(&context, &input)? {
            entry.cancel.store(true, Ordering::Release);
            let mut snapshot = entry
                .snapshot
                .lock()
                .map_err(|_| failure(503, "management_unavailable"))?;
            snapshot.url = None;
            if snapshot.running {
                snapshot.status.state = "stopping".into();
                status = snapshot.status.clone();
            }
        }
        Ok(status)
    }
    pub async fn account_login_authorization(
        &self,
        input: AccountLoginRequest,
    ) -> Result<AccountLoginAuthorization, NativeError> {
        self.account_login_status(input.clone()).await?;
        let (context, _) = self.authorized().await?;
        let entry = self
            .owner
            .0
            .account_logins
            .get(&context, &input)?
            .ok_or(failure(409, "provider_authorization_unavailable"))?;
        if entry.context != context || entry.cancel.load(Ordering::Acquire) {
            return Err(failure(409, "provider_authorization_unavailable"));
        }
        let snapshot = entry
            .snapshot
            .lock()
            .map_err(|_| failure(503, "management_unavailable"))?;
        if !snapshot.running {
            return Err(failure(409, "provider_authorization_unavailable"));
        }
        snapshot
            .url
            .clone()
            .map(AccountLoginAuthorization)
            .ok_or(failure(409, "provider_authorization_unavailable"))
    }
}
fn update(entry: &Attempt, state: &str, url: Option<String>, running: bool) {
    if let Ok(mut snapshot) = entry.snapshot.lock() {
        snapshot.status.state = state.into();
        snapshot.url = url;
        snapshot.running = running;
    }
}
fn observed(entry: &Attempt, state: LoginState) -> Option<&'static str> {
    if state.authenticated {
        Some("authenticated")
    } else if !state.waiting {
        Some("failed")
    } else {
        update(
            entry,
            "awaiting_authorization",
            state.authorization_url,
            true,
        );
        None
    }
}
async fn run(
    manager: Arc<NativeManagement>,
    context: HostContext,
    input: AccountLoginRequest,
    handle: AccountLoginHandle,
    entry: Arc<Attempt>,
    tasks: TaskCustody,
) -> Result<(), DrainFailure> {
    let deadline = Instant::now() + Duration::from_secs(600);
    let authorized = manager.login_guard(&context, &input).await.is_ok()
        && handle.launch().check().is_ok()
        && tasks.check_open().is_ok();
    let mut login = None;
    let mut terminal = if authorized {
        #[cfg(test)]
        let selected = manager
            .owner
            .0
            .account_logins
            .executable
            .lock()
            .unwrap()
            .clone();
        #[cfg(test)]
        let start = if let Some(executable) = selected {
            NamespaceLogin::start_with(handle.launch(), &tasks, &executable).await
        } else {
            NamespaceLogin::start(handle.launch(), &tasks).await
        };
        #[cfg(not(test))]
        let start = NamespaceLogin::start(handle.launch(), &tasks).await;
        match start {
            Ok((process, status)) => {
                login = Some(process);
                observed(&entry, status)
            }
            Err(_error) => {
                #[cfg(test)]
                eprintln!("namespace login start: {}", _error.code);
                Some("uncertain")
            }
        }
    } else {
        Some("uncertain")
    };
    while terminal.is_none() {
        if entry.cancel.load(Ordering::Acquire) {
            terminal = Some("cancelled");
            break;
        }
        if Instant::now() >= deadline {
            terminal = Some("expired");
            break;
        }
        if tasks.check_open().is_err()
            || manager.login_guard(&context, &input).await.is_err()
            || handle.launch().check().is_err()
        {
            terminal = Some("uncertain");
            break;
        }
        match login.as_mut().unwrap().observe().await {
            Ok(status) => terminal = observed(&entry, status),
            Err(_) => terminal = Some("uncertain"),
        }
        if terminal.is_none() {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    update(&entry, "stopping", None, true);
    let stopped = match login.as_mut() {
        Some(p) => p.stop().await.is_ok(),
        None => true,
    };
    // Original namespace and current SDK must still agree before a fresh login claim.
    let mut state = terminal.unwrap_or("uncertain");
    if !stopped
        || (state == "authenticated"
            && (manager.login_guard(&context, &input).await.is_err()
                || handle.launch().check().is_err()
                || entry.cancel.load(Ordering::Acquire)
                || tasks.check_open().is_err()))
    {
        state = "uncertain";
    }
    let verdict = LoginVerdict {
        mode: if state == "authenticated" {
            AccountReadinessMode::Subscription
        } else {
            AccountReadinessMode::Unknown
        },
        outcome: match state {
            "authenticated" => LoginOutcome::Observed,
            "failed" | "cancelled" | "expired" => LoginOutcome::Refused,
            _ => LoginOutcome::Uncertain,
        },
        provider_state: state.into(),
        expires_at_ms: None,
    };
    let persisted = if stopped { handle.settle(verdict).await.is_ok() } else { false };
    update(
        &entry,
        if persisted { state } else { "uncertain" },
        None,
        false,
    );
    if stopped && persisted {
        Ok(())
    } else {
        Err(DrainFailure::Unknown)
    }
}
