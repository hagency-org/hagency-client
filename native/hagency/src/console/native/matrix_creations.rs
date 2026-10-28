//! Typed, token-free Matrix creation entrypoints. These call the journal service
//! directly; no Salvo requests, route paths, or TestClient are constructed.
use super::{Command, NativeError, NativeOwner, failure};
use crate::console::server_login::matrix_creations::service::{self, Action, Reply};
pub use crate::console::server_login::matrix_creations::{
    Input as MatrixCreationInput, Kind as MatrixCreationKind, Resume as MatrixCreationResume,
};
pub use service::{
    Creation as MatrixCreation, CreationReply as MatrixCreationReply, Entry as MatrixCreationEntry,
    ListReply as MatrixCreationList, Phase as MatrixCreationPhase,
};

pub(super) fn action(command: &Command) -> Result<Option<Action>, NativeError> {
    Ok(Some(match command {
        Command::CreateMatrix { input } => {
            // Compatibility adapter for callers predating the typed API. Keep
            // the former HTTP body limit and closed fields on this boundary.
            if serde_json::to_vec(input)
                .map_err(|_| failure(400, "invalid_arguments"))?
                .len()
                > 16384
            {
                return Err(failure(400, "invalid_arguments"));
            }
            Action::Start(
                serde_json::from_value(input.clone())
                    .map_err(|_| failure(400, "invalid_arguments"))?,
            )
        }
        Command::Creations => Action::List,
        Command::Creation { id } => Action::Status(id.clone()),
        Command::ResumeCreation { id, room_id } => Action::Resume {
            id: id.clone(),
            input: MatrixCreationResume {
                room_id: room_id.clone(),
            },
        },
        _ => return Ok(None),
    }))
}
impl NativeOwner {
    /// Legacy owner-managed Space/Room creation. Managed setup uses a separate
    /// approval protocol and marker and is deliberately not enabled here.
    pub async fn matrix_creation_start(
        &self,
        input: MatrixCreationInput,
    ) -> Result<MatrixCreationReply, NativeError> {
        one(self.run_creation(Action::Start(input)).await?)
    }
    pub async fn matrix_creation_status(
        &self,
        command_id: String,
    ) -> Result<MatrixCreationReply, NativeError> {
        one(self.run_creation(Action::Status(command_id)).await?)
    }
    pub async fn matrix_creation_resume(
        &self,
        command_id: String,
        input: MatrixCreationResume,
    ) -> Result<MatrixCreationReply, NativeError> {
        one(self
            .run_creation(Action::Resume {
                id: command_id,
                input,
            })
            .await?)
    }
    pub async fn matrix_creations(&self) -> Result<MatrixCreationList, NativeError> {
        match self.run_creation(Action::List).await? {
            Reply::List(reply) => Ok(reply),
            Reply::One(_) => Err(failure(503, "invalid_native_response")),
        }
    }
    async fn run_creation(&self, action: Action) -> Result<Reply, NativeError> {
        let _guard = self.0.calls.lock().await;
        let matrix_managed = self
            .0
            .matrix_source
            .lock()
            .map_err(|_| failure(503, "native_state_unavailable"))?
            .is_some();
        if matrix_managed {
            self.sync_matrix().await?;
        } else {
            self.identity().await?;
        }
        let cookie = self.0.cookie.lock().await.clone();
        self.creation_locked(&cookie, action).await
    }
    pub(super) async fn creation_locked(
        &self,
        cookie: &str,
        action: Action,
    ) -> Result<Reply, NativeError> {
        let raw = cookie
            .strip_prefix("hagency_console=")
            .ok_or(failure(401, "owner_authorization_required"))?;
        service::invoke(
            &self.0.host.console,
            raw,
            action,
            Some((&self.0.expected_origin, &self.0.expected_owner)),
        )
        .await
        .map_err(|error| failure(error.status, &error.code))
    }
}
fn one(reply: Reply) -> Result<MatrixCreationReply, NativeError> {
    match reply {
        Reply::One(reply) => Ok(reply),
        Reply::List(_) => Err(failure(503, "invalid_native_response")),
    }
}

impl super::NativeManagement {
    pub async fn pool_agent_room(&self,command_id:String,name:String)->Result<MatrixCreationReply,NativeError> {
        use hagency_store::management::Permission;
        let (context,session)=self.authorized().await?;
        session.authorize(&context,Permission::RequestAgents).map_err(|_|failure(403,"management_permission_required"))?;
        let cookie=self.owner.0.cookie.lock().await.clone();
        let raw=cookie.strip_prefix("hagency_console=").ok_or_else(||failure(401,"owner_authorization_required"))?;
        let device=self.owner.0.host.console.authorized_device().await.map_err(|_|failure(401,"owner_authorization_required"))?;
        let gate=Box::new(move || !self.is_retired() && session.authorize(&context,Permission::RequestAgents).is_ok());
        service::invoke_pool(&self.owner.0.host.console,raw,&device,gate,MatrixCreationInput {
            command_id,kind:MatrixCreationKind::AgentRoom,name,project_id:None,space_id:None,
        }).await.map_err(|e|failure(e.status,&e.code))
    }
}
