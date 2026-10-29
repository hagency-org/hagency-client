//! No external network. The orchestration fixtures inject DNS/HTTP responses;
//! the transport fixture uses an isolated loopback socket to prove address pinning.
use super::*;
use matrix_sdk::ruma::exports::http;
use std::{collections::VecDeque, sync::Mutex};

type Request = (url::Url, Vec<SocketAddr>);
#[derive(Default)]
struct Fixture {
    answers: Mutex<VecDeque<Option<Vec<SocketAddr>>>>,
    responses: Mutex<VecDeque<Response>>,
    lookups: Mutex<Vec<(String, u16)>>,
    requests: Mutex<Vec<Request>>,
}
impl Fixture {
    fn new(answers: Vec<Option<Vec<SocketAddr>>>, responses: Vec<Response>) -> Self {
        Self {
            answers: Mutex::new(answers.into()),
            responses: Mutex::new(responses.into()),
            ..Self::default()
        }
    }
}
impl Network for Fixture {
    async fn resolve(&self, host: String, port: u16) -> Option<Vec<SocketAddr>> {
        self.lookups.lock().unwrap().push((host, port));
        self.answers.lock().unwrap().pop_front().flatten()
    }
    async fn get(&self, url: url::Url, addresses: Vec<SocketAddr>) -> Option<Response> {
        self.requests.lock().unwrap().push((url, addresses));
        self.responses.lock().unwrap().pop_front()
    }
}
fn addresses(values: &[&str]) -> Option<Vec<SocketAddr>> {
    Some(values.iter().map(|value| value.parse().unwrap()).collect())
}
fn response(status: u16, location: Option<&str>, body: Vec<u8>) -> Response {
    let mut builder = http::Response::builder().status(status);
    if let Some(location) = location {
        builder = builder.header("location", location);
    }
    builder.body(body).unwrap().into()
}
fn redirect(location: &str) -> Response {
    response(302, Some(location), Vec::new())
}
fn success() -> Response {
    response(200, None, b"image fixture".to_vec())
}

#[tokio::test]
async fn image_fetch_rejects_private_literals_and_credentials_before_io() {
    let network = Fixture::default();
    for url in [
        "file:///etc/passwd",
        "https://127.0.0.1/a.png",
        "https://localhost/a",
        "https://app.local/a",
        "https://app.local./a",
        "https://app.localhost/a",
        "https://user:password@example.org/a",
        "https://[::1]/a",
        "http://[::ffff:127.0.0.1]/a",
        "http://[::ffff:192.168.1.1]/a",
        "http://[::ffff:169.254.169.254]/a",
        "http://100.64.0.1/a",
        "http://198.18.0.1/a",
        "http://0.1.2.3/a",
        "http://240.0.0.1/a",
        "http://[64:ff9b::7f00:1]/a",
        "http://[2002:7f00:1::]/a",
    ] {
        assert!(fetch(url, &network).await.is_none(), "{url}");
    }
    assert!(network.lookups.lock().unwrap().is_empty());
    assert!(network.requests.lock().unwrap().is_empty());
    for url in [
        article_core::render::EDITORMD_LOGO,
        "https://[2606:4700:4700::1111]/a",
    ] {
        assert!(allowed(&url::Url::parse(url).unwrap()), "{url}");
    }
}

#[tokio::test]
async fn image_fetch_rejects_private_mixed_empty_and_failed_dns_before_http() {
    for answer in [
        addresses(&["127.0.0.1:80"]),
        addresses(&["192.168.1.1:80"]),
        addresses(&["[::ffff:127.0.0.1]:80"]),
        addresses(&["93.184.216.34:80", "10.0.0.1:80"]),
        addresses(&["93.184.216.34:81"]),
        Some(vec!["93.184.216.34:80".parse().unwrap(); MAX_ADDRESSES + 1]),
        Some(Vec::new()),
        None,
    ] {
        let fixture = Fixture::new(vec![answer], vec![success()]);
        assert!(fetch("http://images.example/a", &fixture).await.is_none());
        assert_eq!(fixture.lookups.lock().unwrap().len(), 1);
        assert!(fixture.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn image_fetch_pins_every_dns_answer_and_rechecks_relative_and_cross_host_redirects() {
    let first = addresses(&["93.184.216.34:80", "1.1.1.1:80"]);
    let second = addresses(&["93.184.216.35:80"]);
    let third = addresses(&["[2606:4700:4700::1111]:443"]);
    let fixture = Fixture::new(
        vec![first.clone(), second.clone(), third.clone()],
        vec![
            redirect("/second.png"),
            redirect("https://cdn.example/final.png"),
            success(),
        ],
    );
    assert_eq!(
        fetch("http://images.example/first.png", &fixture)
            .await
            .unwrap(),
        b"image fixture"
    );
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].1, first.unwrap());
    assert_eq!(requests[1].0.as_str(), "http://images.example/second.png");
    assert_eq!(requests[1].1, second.unwrap());
    assert_eq!(requests[2].0.as_str(), "https://cdn.example/final.png");
    assert_eq!(requests[2].1, third.unwrap());
    assert_eq!(fixture.lookups.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn image_fetch_rejects_redirects_to_private_hosts_and_dns_rebinding() {
    for target in [
        "http://127.0.0.1/private",
        "http://[::ffff:127.0.0.1]/private",
        "file:///tmp/image",
        "https://user:secret@cdn.example/a",
    ] {
        let fixture = Fixture::new(
            vec![addresses(&["93.184.216.34:80"])],
            vec![redirect(target)],
        );
        assert!(fetch("http://images.example/a", &fixture).await.is_none());
        assert_eq!(fixture.requests.lock().unwrap().len(), 1, "{target}");
    }
    for target in ["http://private.example/a", "/same-host-new-dns"] {
        let fixture = Fixture::new(
            vec![
                addresses(&["93.184.216.34:80"]),
                addresses(&["127.0.0.1:80"]),
            ],
            vec![redirect(target), success()],
        );
        assert!(fetch("http://images.example/a", &fixture).await.is_none());
        assert_eq!(fixture.lookups.lock().unwrap().len(), 2);
        assert_eq!(fixture.requests.lock().unwrap().len(), 1, "{target}");
    }
}

#[tokio::test]
async fn image_fetch_bounds_redirects_status_and_streamed_response_bytes() {
    let fixture = Fixture::new(
        vec![addresses(&["93.184.216.34:80"]); MAX_REDIRECTS + 2],
        (0..MAX_REDIRECTS + 2).map(|_| redirect("/loop")).collect(),
    );
    assert!(fetch("http://images.example/a", &fixture).await.is_none());
    assert_eq!(fixture.requests.lock().unwrap().len(), MAX_REDIRECTS + 1);
    for reply in [
        response(404, None, b"private error".to_vec()),
        response(304, None, Vec::new()),
        response(302, None, Vec::new()),
        response(200, None, vec![0; MAX_BYTES + 1]),
    ] {
        let fixture = Fixture::new(vec![addresses(&["93.184.216.34:80"])], vec![reply]);
        assert!(fetch("http://images.example/a", &fixture).await.is_none());
    }
    // An unknown-length streamed body must be bounded while consuming chunks,
    // even when neither HTTP metadata nor the body size hint advertises a size.
    let chunks = stream::iter([Ok::<_, std::io::Error>(vec![0; MAX_BYTES]), Ok(vec![0])]);
    let streamed = http::Response::builder()
        .status(200)
        .body(matrix_sdk::reqwest::Body::wrap_stream(chunks))
        .unwrap();
    let fixture = Fixture::new(
        vec![addresses(&["93.184.216.34:80"])],
        vec![streamed.into()],
    );
    assert!(fetch("http://images.example/a", &fixture).await.is_none());
    let fixture = Fixture::new(
        vec![addresses(&["93.184.216.34:80"])],
        vec![response(200, None, vec![0; MAX_BYTES])],
    );
    assert_eq!(
        fetch("http://images.example/a", &fixture)
            .await
            .unwrap()
            .len(),
        MAX_BYTES
    );
}

// Exercise the real reqwest transport on an isolated socket. Only this transport
// test injects loopback directly; production fetch rejects it before Network::get.
async fn transport_fixture(
    reply: &'static [u8],
) -> (url::Url, SocketAddr, tokio::task::JoinHandle<String>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(3), async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") && request.len() < 8192 {
                if stream.read(&mut byte).await.unwrap() == 0 {
                    break;
                }
                request.push(byte[0]);
            }
            stream.write_all(reply).await.unwrap();
            String::from_utf8(request).unwrap()
        })
        .await
        .unwrap()
    });
    let url = url::Url::parse(&format!("http://images.invalid:{}/image", address.port())).unwrap();
    (url, address, task)
}

#[tokio::test]
async fn image_transport_uses_pinned_address_and_leaves_redirect_for_validation() {
    let (url, address, task) = transport_fixture(
        b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1/private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    ).await;
    let response = SystemNetwork.get(url, vec![address]).await.unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(response.remote_addr(), Some(address));
    let request = task.await.unwrap();
    assert!(request.starts_with("GET /image HTTP/1.1\r\n"));
    assert!(
        request
            .to_ascii_lowercase()
            .contains(&format!("host: images.invalid:{}", address.port()))
    );
}

#[tokio::test]
async fn image_fetch_rejects_real_oversized_content_length_before_reading() {
    let (url, address, task) = transport_fixture(
        b"HTTP/1.1 200 OK\r\nContent-Length: 8388609\r\nConnection: close\r\n\r\n",
    )
    .await;
    let response = SystemNetwork.get(url, vec![address]).await.unwrap();
    assert_eq!(response.content_length(), Some(MAX_BYTES as u64 + 1));
    let fixture = Fixture::new(vec![addresses(&["93.184.216.34:80"])], vec![response]);
    assert!(fetch("http://images.example/a", &fixture).await.is_none());
    task.await.unwrap();
}
