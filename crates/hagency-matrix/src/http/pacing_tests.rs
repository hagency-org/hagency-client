use super::*;
use crate::collector::fixtures as common;
use serde_json::json;
use std::time::Duration;

async fn clients(interval: u64) -> (common::Fake, Http, Http, Arc<RequestPacing>) {
    let fake = common::Fake::start(true).await;
    // HTTP cadence has no domain-store dependency. Preserve the two clients'
    // shared pacing owner, real TLS roots, credentials and transport limits.
    let mut limits = common::limits();
    let pacing =
        Arc::new(RequestPacing::new(&fake.endpoint, Duration::from_millis(interval)).unwrap());
    limits.request_pacing = Some(pacing.clone());
    let endpoint = Url::parse(&fake.endpoint).unwrap();
    let roots =
        [reqwest::Certificate::from_pem(include_bytes!("../../tests/fixtures/ca.pem")).unwrap()];
    let authorization = HeaderValue::from_str(&format!("Bearer {}", common::TOKEN)).unwrap();
    let first = Http::for_host(&endpoint, Some(&authorization), &limits, &roots).unwrap();
    let authorization = HeaderValue::from_static("Bearer synthetic-independent-pacing-token");
    let second = Http::for_host(&endpoint, Some(&authorization), &limits, &roots).unwrap();
    (fake, first, second, pacing)
}

#[tokio::test]
async fn native_matrix_shared_request_pacing() {
    let (mut fake, first, second, _pacing) = clients(60).await;
    let cancel = CancellationToken::new();
    let started = Instant::now();
    let script = async {
        let mut reads = 0;
        let mut posts = 0;
        let mut puts = 0;
        for index in 0..4 {
            let request = fake.next().await;
            // Inspect actual received attempts without joining the pacing
            // mutex queue: a queued diagnostic read would observe a later slot.
            assert!(
                started.elapsed() >= Duration::from_millis(index * 60),
                "independent client bypassed the original pacing owner"
            );
            match request.method.as_str() {
                "GET" => {
                    assert_eq!(request.target, "/paced-read");
                    assert_eq!(
                        request.headers["authorization"],
                        format!("Bearer {}", common::TOKEN)
                    );
                    reads += 1;
                    if reads == 1 {
                        request.json(429, json!({"retry_after_ms":0}));
                    } else {
                        request.json(200, json!({"ok":true}));
                    }
                }
                "POST" => {
                    assert_eq!(
                        request.headers["authorization"],
                        "Bearer synthetic-independent-pacing-token"
                    );
                    posts += 1;
                    // Task #9: a permanent write refusal ends the write
                    // immediately (401/403 are not retried); a 429 would now
                    // retry like a read.
                    request.json(403, json!({"errcode": "M_FORBIDDEN"}));
                }
                "PUT" => {
                    puts += 1;
                    request.json(200, json!({"ok":true}));
                }
                _ => panic!("unexpected fixture method"),
            }
        }
        assert_eq!((reads, posts, puts), (2, 1, 1));
    };
    let (read, post, put, ()) = tokio::join!(
        first.request(&["paced-read"], None, &cancel),
        second.post(&["once"], "{}".into(), &cancel),
        first.put(&["once"], "{}".into(), &cancel),
        script
    );
    assert_eq!(read.unwrap().success().unwrap(), json!({"ok":true}));
    assert_eq!(post.unwrap().success(), Err(Error::Unauthorized));
    assert_eq!(put.unwrap().success().unwrap(), json!({"ok":true}));
    assert_eq!(fake.requests(), 4);
    fake.no_request().await;
    fake.close().await;
}

#[tokio::test]
async fn native_matrix_media_request_pacing() {
    let (mut fake, first, second, pacing) = clients(80).await;
    let cancel = CancellationToken::new();
    let (prime, ()) = tokio::join!(first.request(&["prime"], None, &cancel), async {
        fake.next().await.json(200, json!({}));
    });
    prime.unwrap().success().unwrap();
    let next = pacing.next.try_lock().unwrap().unwrap();
    let upload = first.prepare_upload(b"encrypted fixture", 128).unwrap();
    let script = async {
        for index in 0..2 {
            let request = fake.next().await;
            assert!(
                Instant::now() >= next + Duration::from_millis(index * 80),
                "binary transfer bypassed the shared cadence"
            );
            match request.method.as_str() {
                "POST"=>{assert_eq!(request.body,b"encrypted fixture");request.json(200,json!({"content_uri":"mxc://example.test/paced_media"}));},
                "GET"=>request.raw(b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 4\r\nConnection: close\r\n\r\ndata".to_vec()),
                _=>panic!("unexpected binary fixture method"),
            }
        }
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    let (upload, download, ()) = tokio::join!(
        first.upload(upload, deadline, &cancel),
        second.download(&["media"], 128, deadline, &cancel),
        script
    );
    assert_eq!(
        upload.unwrap().body(),
        br#"{"content_uri":"mxc://example.test/paced_media"}"#
    );
    assert_eq!(download.unwrap(), b"data");
    assert_eq!(fake.requests(), 3);
    fake.close().await;
}

#[tokio::test]
async fn native_matrix_request_pacing_bounds() {
    let (mut fake, first, mut second, pacing) = clients(1000).await;
    let cancel = CancellationToken::new();
    for interval in [0, 9, 1001, u64::MAX] {
        assert!(matches!(
            RequestPacing::new(&fake.endpoint, Duration::from_millis(interval)),
            Err(Error::Config)
        ));
    }
    assert!(matches!(
        Http::for_host(
            &Url::parse("https://other.example.test/").unwrap(),
            None,
            &first.limits,
            &[]
        ),
        Err(Error::Config)
    ));
    let (prime, ()) = tokio::join!(first.request(&["prime"], None, &cancel), async {
        fake.next().await.json(200, json!({}));
    });
    prime.unwrap().success().unwrap();
    let original = *pacing.next.lock().await;
    let stopped = CancellationToken::new();
    let (result, ()) = tokio::join!(second.post(&["never"], "{}".into(), &stopped), async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        stopped.cancel();
    });
    assert!(matches!(result, Err(Error::Cancelled)));
    assert_eq!(*pacing.next.lock().await, original);
    second.limits.request = Duration::from_millis(30);
    assert!(matches!(
        second.put(&["never"], "{}".into(), &cancel).await,
        Err(Error::Timeout)
    ));
    assert_eq!(*pacing.next.lock().await, original);
    let held = pacing.next.lock().await;
    assert!(matches!(
        second.request(&["never"], None, &cancel).await,
        Err(Error::Timeout)
    ));
    drop(held);
    assert_eq!(*pacing.next.lock().await, original);
    assert_eq!(fake.requests(), 1);
    fake.no_request().await;
    fake.close().await;
}
