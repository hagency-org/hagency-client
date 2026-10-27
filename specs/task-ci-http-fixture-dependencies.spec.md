spec: task
name: "Keep pure Matrix HTTP fixtures independent of domain writer lifecycle"
tags: [active, rust, fixture, ci]
---

The original Windows job122393152490 at d53d31e6 passed every HTTP retry
assertion, then failed native_matrix_get_rate_limit at its unrelated
DomainStore shutdown with OutcomeUnknown. This establishes the failure site,
not the unobserved OS or database-close phase. The original failure remains.

Only the rate-limit and pacing test constructors change: construct the same
HTTP transport through Http::for_host, to which Http::new already delegates.
Retain the actual TLS peer, root certificate, endpoint, bearer credentials,
shared pacing owner, common limits and per-scenario request budget. These
fixtures never read or write domain state, so they must not start a repository
or writer solely to build an HTTP configuration.

All existing HTTP assertions and fake-peer cleanup remain. Production HTTP,
domain writer shutdown and its strict two-second deadlines do not change.
Store lifecycle tests and service/integration fixtures still use real stores;
an actual shutdown failure there remains fatal, never retried or accepted.

- Retry response, URL/authentication, declared delay and exact request count
  remain unchanged.
  Test: native_matrix_get_rate_limit
- Invalid hints, count, original deadline and cancellation bounds remain.
  Test: native_matrix_get_rate_limit_bounds
- Writes retry only complete429 responses and a lost response never repeats.
  Test: native_matrix_write_rate_limit_retries_like_get
- Independent credentials retain one actual pacing owner.
  Test: native_matrix_shared_request_pacing
- Binary upload/download retain the shared cadence and exact bytes.
  Test: native_matrix_media_request_pacing
- Cancellation, original deadline, invalid interval and endpoint refusal remain.
  Test: native_matrix_request_pacing_bounds
