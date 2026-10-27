use super::*;
use crate::collector::fixtures as common;
use serde_json::json;
use std::time::Duration;

async fn client(budget: Duration) -> (common::Fake, Http) {
    let fake = common::Fake::start(true).await;
    // These transport tests need no domain repository or writer thread.
    // Use the same constructor that Http::new delegates to, with identical
    // credentials, roots and bounds, without unrelated database teardown.
    let mut limits = common::limits();
    limits.request = budget;
    limits.headers = limits.headers.min(budget);
    limits.connect = limits.connect.min(budget);
    limits.body_idle = limits.body_idle.min(budget);
    let authorization = HeaderValue::from_str(&format!("Bearer {}", common::TOKEN)).unwrap();
    let root =
        reqwest::Certificate::from_pem(include_bytes!("../../tests/fixtures/ca.pem")).unwrap();
    let http = Http::for_host(
        &Url::parse(&fake.endpoint).unwrap(),
        Some(&authorization),
        &limits,
        &[root],
    )
    .unwrap();
    (fake, http)
}
fn limited(header: Option<&str>, body: Value) -> Vec<u8> {
    let body = serde_json::to_vec(&body).unwrap();
    let header = header
        .map(|value| format!("Retry-After: {value}\r\n"))
        .unwrap_or_default();
    let mut bytes=format!("HTTP/1.1 429 Fixture\r\nContent-Type: application/json\r\n{header}Content-Length: {}\r\nConnection: close\r\n\r\n",body.len()).into_bytes();
    bytes.extend(body);
    bytes
}

#[tokio::test]
async fn native_matrix_get_rate_limit() {
    for (header, body, delay) in [
        (None, json!({"retry_after_ms":30}), 30),
        (Some("1"), json!({"retry_after_ms":20}), 1000),
        (Some("0"), json!({"retry_after_ms":30}), 30),
        (None, json!({"errcode":"M_LIMIT_EXCEEDED"}), 1000),
    ] {
        let (mut fake, http) = client(Duration::from_secs(3)).await;
        let cancel = CancellationToken::new();
        let script = async {
            let first = fake.next().await;
            let target = first.target.clone();
            let auth = first.headers["authorization"].clone();
            assert_eq!(first.method, "GET");
            let sent = Instant::now();
            first.raw(limited(header, body));
            let second = fake.next().await;
            assert!(sent.elapsed() >= Duration::from_millis(delay));
            assert_eq!(second.method, "GET");
            assert_eq!(second.target, target);
            assert_eq!(second.headers["authorization"], auth);
            second.json(200, json!({"fresh":true}));
        };
        let (result, ()) = tokio::join!(
            http.request(
                &["_matrix", "client", "v3", "sync"],
                Some(&[("since", "original-cursor")]),
                &cancel
            ),
            script
        );
        assert_eq!(result.unwrap().success().unwrap(), json!({"fresh":true}));
        assert_eq!(fake.requests(), 2);
        fake.close().await;
    }
}

#[tokio::test]
async fn native_matrix_get_rate_limit_bounds() {
    for (header, body) in [
        (None, json!({"retry_after_ms":900})),
        (None, json!({"retry_after_ms":-1})),
        (None, json!({"retry_after_ms":"100"})),
        (Some("invalid"), json!({})),
        (Some("18446744073709551616"), json!({})),
        (Some("18446744073709551615"), json!({})),
        (Some("1\r\nRetry-After: 2"), json!({})),
    ] {
        let (mut fake, http) = client(Duration::from_millis(900)).await;
        let cancel = CancellationToken::new();
        let (result, ()) = tokio::join!(http.request(&["bounded"], None, &cancel), async {
            fake.next().await.raw(limited(header, body));
        });
        assert_eq!(result.unwrap().success(), Err(Error::Remote(429)));
        assert_eq!(fake.requests(), 1);
        fake.no_request().await;
        fake.close().await;
    }
    let (mut fake, http) = client(Duration::from_millis(900)).await;
    let cancel = CancellationToken::new();
    let (result, ()) = tokio::join!(http.request(&["bounded"], None, &cancel), async {
        for _ in 0..RATE_LIMIT_TRIES {
            fake.next().await.json(429, json!({"retry_after_ms":0}));
        }
    });
    assert_eq!(result.unwrap().success(), Err(Error::Remote(429)));
    assert_eq!(fake.requests(), RATE_LIMIT_TRIES as u64);
    fake.no_request().await;
    fake.close().await;

    // A fresh deadline per attempt would allow more than these two requests.
    let (mut fake, http) = client(Duration::from_millis(300)).await;
    let cancel = CancellationToken::new();
    let (result, ()) = tokio::join!(http.request(&["bounded"], None, &cancel), async {
        for _ in 0..2 {
            fake.next().await.json(429, json!({"retry_after_ms":200}));
        }
    });
    assert_eq!(result.unwrap().success(), Err(Error::Remote(429)));
    assert_eq!(fake.requests(), 2);
    fake.no_request().await;
    fake.close().await;

    let (mut fake, http) = client(Duration::from_secs(3)).await;
    let cancel = CancellationToken::new();
    let (result, ()) = tokio::join!(http.request(&["cancel"], None, &cancel), async {
        fake.next().await.json(429, json!({"retry_after_ms":2000}));
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();
    });
    assert!(matches!(result, Err(Error::Cancelled)));
    assert_eq!(fake.requests(), 1);
    fake.no_request().await;
    fake.close().await;
}

#[tokio::test]
async fn native_matrix_write_rate_limit_retries_like_get() {
    // Task #9 (TS fetchWithRateLimit): a write that gets 429 retries through
    // the same six-try budget as a read, on the shared cooldown gate.
    for method in [reqwest::Method::POST, reqwest::Method::PUT] {
        let (mut fake, http) = client(Duration::from_secs(3)).await;
        let cancel = CancellationToken::new();
        let (result, ()) = tokio::join!(
            async {
                if method == reqwest::Method::POST {
                    http.post(&["single"], "{}".into(), &cancel).await
                } else {
                    http.put(&["single"], "{}".into(), &cancel).await
                }
            },
            async {
                for _ in 0..(RATE_LIMIT_TRIES - 1) {
                    let request = fake.next().await;
                    assert_eq!(request.method, method.as_str());
                    request.json(429, json!({"retry_after_ms":0}));
                }
                let last = fake.next().await;
                assert_eq!(last.method, method.as_str());
                last.json(200, json!({"event_id":"$ok"}));
            }
        );
        assert_eq!(
            result.unwrap().success().unwrap(),
            json!({"event_id":"$ok"})
        );
        assert_eq!(fake.requests(), RATE_LIMIT_TRIES as u64);
        fake.no_request().await;
        fake.close().await;
    }
    let (mut fake, http) = client(Duration::from_secs(3)).await;
    let cancel = CancellationToken::new();
    let (result, ()) = tokio::join!(http.request(&["lost"], None, &cancel), async {
        fake.next().await.unclean(Vec::new());
    });
    assert!(matches!(result, Err(Error::Transport)));
    assert_eq!(fake.requests(), 1);
    fake.no_request().await;
    fake.close().await;
}
