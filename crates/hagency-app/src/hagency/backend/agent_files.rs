//! Agent attachment bytes use the current authenticated SDK account. No shared
//! preview cache, ambient HTTP client, arbitrary URL, or unbounded SDK buffering.
use super::{NativeError, source_error};
use ::hagency::native_owner::{AgentAttachment, MAX_AGENT_FILE_BYTES};
use matrix_sdk::{
    Client,
    ruma::{
        api::{Metadata, client::authenticated_media, path_builder::PathBuilder},
        events::room::MediaSource,
    },
};
use std::{io::Read, time::Duration};
static FETCHES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
fn unavailable() -> NativeError {
    NativeError {
        status: 503,
        code: "agent_file_download_unavailable".into(),
    }
}
fn too_large() -> NativeError {
    NativeError {
        status: 413,
        code: "agent_file_too_large".into(),
    }
}
pub(super) async fn fetch(
    client: &Client,
    origin: &str,
    owner: &str,
    attachment: &AgentAttachment,
    live: impl Fn() -> Result<(), NativeError>,
) -> Result<Vec<u8>, NativeError> {
    tokio::time::timeout(
        Duration::from_secs(30),
        fetch_inner(client, origin, owner, attachment, &live),
    )
    .await
    .map_err(|_| unavailable())?
}
async fn fetch_inner(
    client: &Client,
    origin: &str,
    owner: &str,
    attachment: &AgentAttachment,
    live: &impl Fn() -> Result<(), NativeError>,
) -> Result<Vec<u8>, NativeError> {
    let check = || {
        live()?;
        if client.homeserver().as_str() != origin
            || client.user_id().is_none_or(|u| u.as_str() != owner)
        {
            return Err(source_error("matrix_account_changed"));
        }
        Ok(())
    };
    check()?;
    attachment.validate().map_err(|_| unavailable())?;
    if attachment
        .declared_size
        .is_some_and(|s| s > MAX_AGENT_FILE_BYTES as u64)
    {
        return Err(too_large());
    }
    let _permit = FETCHES.acquire().await.map_err(|_| unavailable())?;
    check()?;
    let source: MediaSource = match attachment.source["kind"].as_str() {
        Some("plain") => MediaSource::Plain(
            attachment.source["url"]
                .as_str()
                .ok_or_else(unavailable)?
                .into(),
        ),
        Some("encrypted") => MediaSource::Encrypted(Box::new(
            serde_json::from_value(attachment.source["file"].clone()).map_err(|_| unavailable())?,
        )),
        _ => return Err(unavailable()),
    };
    let uri = match &source {
        MediaSource::Plain(uri) => uri,
        MediaSource::Encrypted(file) => &file.url,
    };
    let versions = client
        .supported_versions()
        .await
        .map_err(|_| unavailable())?;
    check()?;
    let history = authenticated_media::get_content::v1::Request::PATH_BUILDER;
    if !history.is_supported(&versions) {
        return Err(unavailable());
    }
    let (server, id) = uri.parts().map_err(|_| unavailable())?;
    let endpoint = history
        .make_endpoint_url(
            std::borrow::Cow::Borrowed(&versions),
            origin,
            &[&server, &id],
            "",
        )
        .map_err(|_| unavailable())?;
    let url = url::Url::parse(&endpoint).map_err(|_| unavailable())?;
    let mut response = client
        .http_client()
        .get(url.clone())
        .timeout(Duration::from_secs(25))
        .bearer_auth(client.access_token().ok_or_else(unavailable)?)
        .send()
        .await
        .map_err(|_| unavailable())?
        .error_for_status()
        .map_err(|_| unavailable())?;
    check()?;
    if response.url() != &url {
        return Err(unavailable());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_AGENT_FILE_BYTES as u64)
    {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        check()?;
        if bytes.len().saturating_add(chunk.len()) > MAX_AGENT_FILE_BYTES {
            return Err(too_large());
        }
        bytes.extend_from_slice(&chunk);
    }
    if let MediaSource::Encrypted(file) = source {
        let mut cursor = std::io::Cursor::new(bytes);
        let mut decryptor =
            matrix_sdk_base::crypto::AttachmentDecryptor::new(&mut cursor, (*file).into())
                .map_err(|_| unavailable())?;
        let mut plaintext = Vec::new();
        decryptor
            .read_to_end(&mut plaintext)
            .map_err(|_| NativeError {
                status: 422,
                code: "agent_file_integrity_failed".into(),
            })?;
        bytes = plaintext;
    }
    check()?;
    if bytes.len() > MAX_AGENT_FILE_BYTES
        || attachment
            .declared_size
            .is_some_and(|n| n != bytes.len() as u64)
    {
        return Err(NativeError {
            status: 422,
            code: "agent_file_length_mismatch".into(),
        });
    }
    Ok(bytes)
}
#[cfg(test)]
mod tests;
