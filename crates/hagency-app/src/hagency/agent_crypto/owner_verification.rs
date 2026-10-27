//! Fresh SDK cross-signing observations; no serialized flag can establish trust.
use super::*;
use std::sync::Arc;

fn identity_device(
    expected: &AgentCryptoIdentity,
) -> Result<&matrix_sdk::ruma::DeviceId, NativeError> {
    if expected.matrix_device_id.is_empty() {
        return Err(refused());
    }
    Ok(expected.matrix_device_id.as_str().into())
}
fn refused() -> NativeError {
    error("agent_crypto_owner_verification_unavailable")
}
async fn observe(
    client: &Client,
    expected: &AgentCryptoIdentity,
    owner: &matrix_sdk::ruma::UserId,
    fence: &Arc<matrix_sdk::HttpRequestFence>,
) -> Result<matrix_sdk::encryption::identities::UserIdentity, NativeError> {
    let own = client
        .encryption()
        .request_user_identity_with_fence(
            owner,
            client.device_id().ok_or_else(refused)?,
            fence.clone(),
        )
        .await
        .map_err(|_| refused())?
        .ok_or_else(|| error("agent_crypto_owner_keys_unverified"))?;
    let device = client
        .encryption()
        .get_own_device()
        .await
        .map_err(|_| refused())?
        .ok_or_else(refused)?;
    if !own.is_verified() || device.is_deleted() || !device.is_verified_with_cross_signing() {
        return Err(error("agent_crypto_owner_keys_unverified"));
    }
    let user = UserId::parse(&expected.matrix_user_id).map_err(|_| refused())?;
    if user == owner {
        return Err(error("agent_crypto_identity_mismatch"));
    }
    let identity = client
        .encryption()
        .request_user_identity_with_fence(&user, identity_device(expected)?, fence.clone())
        .await
        .map_err(|_| refused())?
        .ok_or_else(refused)?;
    let devices = client
        .encryption()
        .get_user_devices(&user)
        .await
        .map_err(|_| refused())?;
    let device = devices
        .devices()
        .find(|d| d.device_id().as_str() == expected.matrix_device_id)
        .ok_or_else(refused)?;
    if identity
        .master_key()
        .get_first_key()
        .map(|k| k.to_base64())
        .as_deref()
        != Some(&expected.master_key)
        || device.ed25519_key().map(|k| k.to_base64()).as_deref() != Some(&expected.ed25519)
        || device.curve25519_key().map(|k| k.to_base64()).as_deref() != Some(&expected.curve25519)
        || device.is_deleted()
        || !device.is_cross_signed_by_owner()
    {
        return Err(error("agent_crypto_identity_changed"));
    }
    Ok(identity)
}
/// Explicitly sign, or only observe, the exact independent Agent master/device
/// fingerprints. The caller retains original operation custody through shutdown.
/// The returned boolean is UI evidence, not a tool, message or lease grant.
pub async fn owner_trust(
    client: &Client,
    identity: AgentCryptoIdentity,
    authority: AgentCryptoAuthority,
    verify: bool,
    live: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<bool, NativeError> {
    let device = authority.device().await?;
    let owner = UserId::parse(device.owner_mxid()).map_err(|_| refused())?;
    let publisher = client.device_id().ok_or_else(refused)?.to_owned();
    if client.user_id() != Some(&owner)
        || client.homeserver().as_str() != device.origin()
        || !live()
    {
        return Err(refused());
    }
    let sdk = client.clone();
    let expected_owner = owner.clone();
    let origin = device.origin().to_owned();
    let auth = authority.clone();
    let fence = Arc::new(matrix_sdk::HttpRequestFence::new(move || {
        !auth.is_retired()
            && device.bearer().is_ok()
            && live()
            && sdk.user_id() == Some(&expected_owner)
            && sdk.device_id() == Some(publisher.as_ref())
            && sdk.homeserver().as_str() == origin
    }));
    let _retirement = Retirement(fence.clone());
    let other = observe(client, &identity, &owner, &fence).await?;
    let observed = if verify && !other.is_verified() {
        if !client
            .encryption()
            .cross_signing_status()
            .await
            .is_some_and(|s| s.has_user_signing)
        {
            return Err(error("agent_crypto_owner_signing_keys_unavailable"));
        }
        authority.device().await?;
        other
            .verify_with_request_fence(fence.clone())
            .await
            .map_err(|_| error("agent_crypto_owner_verification_unconfirmed"))?;
        observe(client, &identity, &owner, &fence).await?
    } else {
        other
    };
    // A signature upload receipt or a cached local verification bit cannot
    // establish the result: requery the actual keys/signatures after possible IO.
    authority.device().await?;
    let verified = observed.is_verified();
    if verified {
        let user = UserId::parse(&identity.matrix_user_id).map_err(|_| refused())?;
        let devices = client
            .encryption()
            .get_user_devices(&user)
            .await
            .map_err(|_| refused())?;
        if !devices.devices().any(|d| {
            d.device_id().as_str() == identity.matrix_device_id
                && !d.is_deleted()
                && d.is_verified_with_cross_signing()
        }) {
            return Err(error("agent_crypto_identity_changed"));
        }
    }
    fence.retire_and_drain().await.map_err(|_| refused())?;
    if verify && !verified {
        return Err(error("agent_crypto_owner_verification_unconfirmed"));
    }
    Ok(verified)
}
