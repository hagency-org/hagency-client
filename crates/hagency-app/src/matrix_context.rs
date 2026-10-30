//! Fence asynchronous UI work to the Matrix session that initiated it.
use matrix_sdk::Client;

pub(crate) fn is_current(client: &Client) -> bool {
    crate::sliding_sync::get_client().is_some_and(|current| {
        // SessionMeta is stored inside the SDK's shared BaseClient. Its address
        // remains equal for Client clones, but differs after a same-user/device
        // re-login. Checking only the IDs would accept work from that old session.
        client
            .session_meta()
            .zip(current.session_meta())
            .is_some_and(|(a, b)| std::ptr::eq(a, b))
            && client.user_id().is_some()
            && current.user_id() == client.user_id()
            && current.device_id() == client.device_id()
            && current.homeserver() == client.homeserver()
    })
}

pub(crate) fn ensure_current(client: &Client) -> anyhow::Result<()> {
    anyhow::ensure!(
        is_current(client),
        "The Matrix session changed. Reopen this panel."
    );
    Ok(())
}

/// Opaque, in-process identity of the SDK session that initiated UI work.
/// Same-user/device re-login must not accept a queued result from its old SDK.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SessionPin {
    epoch: u64,
    instance: usize,
    owner: matrix_sdk::ruma::OwnedUserId,
    device: matrix_sdk::ruma::OwnedDeviceId,
    origin: String,
}
impl std::fmt::Debug for SessionPin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MatrixSession")
            .field("epoch", &self.epoch)
            .field("owner", &self.owner)
            .field("device", &self.device)
            .finish_non_exhaustive()
    }
}
impl SessionPin {
    pub(crate) fn capture(client: &Client) -> Option<Self> {
        Some(Self {
            epoch: crate::account_session::epoch(),
            instance: std::ptr::from_ref(client.session_meta()?) as usize,
            owner: client.user_id()?.to_owned(),
            device: client.device_id()?.to_owned(),
            origin: client.homeserver().to_string(),
        })
    }
    pub(crate) fn is_current(&self) -> bool {
        self.epoch == crate::account_session::epoch()
            && !crate::sliding_sync::account_changing()
            && crate::sliding_sync::get_client()
                .is_some_and(|client| Self::capture(&client).as_ref() == Some(self))
    }
}
