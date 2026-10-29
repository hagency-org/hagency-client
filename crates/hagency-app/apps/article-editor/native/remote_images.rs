//! Host-controlled HTTP(S) image loading. Renderers receive bounded PNG bytes only.
use article_makepad::content::{Images, prepare_image};
use futures_util::{StreamExt, stream};
use matrix_sdk::reqwest::{Response, StatusCode, redirect::Policy};
use std::{
    collections::BTreeMap,
    future::Future,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_ADDRESSES: usize = 64;
const MAX_REDIRECTS: usize = 5;
const TIMEOUT: Duration = Duration::from_secs(20);

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && a != 0
                && a < 224
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 192 && b == 0 && matches!(c, 0 | 2))
                && !(a == 192 && b == 88 && c == 99)
                && !(a == 198 && matches!(b, 18 | 19))
                && !ip.is_documentation()
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(mapped));
            }
            let [a, b, ..] = ip.segments();
            // Only native global unicast. Reject translation/tunneling ranges
            // which can hide private IPv4 destinations, and special-use ranges.
            (a & 0xe000) == 0x2000
                && !(a == 0x2001 && (b <= 0x01ff || b == 0x0db8))
                && a != 0x2002
                && !(a == 0x3fff && b < 0x1000)
        }
    }
}

fn allowed(url: &url::Url) -> bool {
    if !matches!(url.scheme(), "https" | "http")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    match url.host() {
        Some(url::Host::Domain(host)) => {
            let host = host.trim_end_matches('.').to_ascii_lowercase();
            host.contains('.') && !host.ends_with(".local") && !host.ends_with(".localhost")
        }
        Some(url::Host::Ipv4(ip)) => public_ip(ip.into()),
        Some(url::Host::Ipv6(ip)) => public_ip(ip.into()),
        None => false,
    }
}

// Keep DNS and HTTP at an explicit boundary so fixtures exercise the production
// per-hop validation/redirect/body path without accessing public or private hosts.
trait Network: Sync {
    fn resolve(
        &self,
        host: String,
        port: u16,
    ) -> impl Future<Output = Option<Vec<SocketAddr>>> + Send;
    fn get(
        &self,
        url: url::Url,
        addresses: Vec<SocketAddr>,
    ) -> impl Future<Output = Option<Response>> + Send;
}

struct SystemNetwork;
impl Network for SystemNetwork {
    async fn resolve(&self, host: String, port: u16) -> Option<Vec<SocketAddr>> {
        Some(
            tokio::net::lookup_host((host.as_str(), port))
                .await
                .ok()?
                .take(MAX_ADDRESSES + 1)
                .collect(),
        )
    }

    async fn get(&self, url: url::Url, addresses: Vec<SocketAddr>) -> Option<Response> {
        // A fresh client per hop cannot reuse a connection from a previous DNS
        // answer. Preserve the hostname for TLS/SNI, but never resolve it again.
        let client = crate::http::client_builder()
            .timeout(TIMEOUT)
            .user_agent("Rinx article preview")
            .no_proxy()
            .redirect(Policy::none())
            .resolve_to_addrs(url.host_str()?, &addresses)
            .build()
            .ok()?;
        client.get(url).send().await.ok()
    }
}

async fn fetch(url: &str, network: &impl Network) -> Option<Vec<u8>> {
    // Includes DNS, all redirect hops and the streamed body in one deadline.
    tokio::time::timeout(TIMEOUT, fetch_inner(url, network))
        .await
        .ok()?
}

async fn fetch_inner(url: &str, network: &impl Network) -> Option<Vec<u8>> {
    let mut url = url::Url::parse(url).ok()?;
    for hop in 0..=MAX_REDIRECTS {
        if !allowed(&url) {
            return None;
        }
        let port = url.port_or_known_default()?;
        let addresses = match url.host()? {
            url::Host::Ipv4(ip) => vec![SocketAddr::new(ip.into(), port)],
            url::Host::Ipv6(ip) => vec![SocketAddr::new(ip.into(), port)],
            url::Host::Domain(host) => network.resolve(host.to_owned(), port).await?,
        };
        // Reject the entire answer if even one candidate is private. Filtering
        // only the first answer leaves connector fallback/Happy Eyeballs unsafe.
        if addresses.is_empty()
            || addresses.len() > MAX_ADDRESSES
            || addresses
                .iter()
                .any(|addr| !public_ip(addr.ip()) || addr.port() != port)
        {
            return None;
        }
        let mut response = network.get(url.clone(), addresses).await?;
        if matches!(
            response.status(),
            StatusCode::MOVED_PERMANENTLY
                | StatusCode::FOUND
                | StatusCode::SEE_OTHER
                | StatusCode::TEMPORARY_REDIRECT
                | StatusCode::PERMANENT_REDIRECT
        ) {
            if hop == MAX_REDIRECTS {
                return None;
            }
            let location = response
                .headers()
                .get(matrix_sdk::reqwest::header::LOCATION)?
                .to_str()
                .ok()?;
            url = url.join(location).ok()?;
            continue;
        }
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|n| n > MAX_BYTES as u64)
        {
            return None;
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.ok()? {
            if chunk.len() > MAX_BYTES.saturating_sub(bytes.len()) {
                return None;
            }
            bytes.extend_from_slice(&chunk);
        }
        return Some(bytes);
    }
    None
}

pub async fn load(urls: Vec<String>) -> Images {
    let mut jobs = stream::iter(
        urls.into_iter()
            .take(article_core::document::MAX_IMAGES)
            .map(|url| async move {
                let bytes = fetch(&url, &SystemNetwork).await?;
                let image = tokio::task::spawn_blocking(move || prepare_image(&bytes))
                    .await
                    .ok()?
                    .ok()?;
                Some((url, image))
            }),
    )
    .buffer_unordered(4);
    let mut images = BTreeMap::new();
    let mut total = 0;
    while let Some(result) = jobs.next().await {
        let Some((url, image)) = result else {
            continue;
        };
        if total + image.png.len() <= 20 * 1024 * 1024 {
            total += image.png.len();
            images.insert(url, image);
        }
    }
    Arc::new(images)
}

#[cfg(test)]
#[path = "remote_image_tests.rs"]
mod tests;
