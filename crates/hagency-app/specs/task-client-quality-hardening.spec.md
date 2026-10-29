spec: task
name: "Harden article image networking and retain bounded native client state"
satisfies: [REQ-CLIENT-QUALITY-REMEDIATION]
tags: [active, rust, desktop, security]
---

## Intent

Implement the first reviewed batch: R01 image network isolation, R09 bounded
profile state, R11 navigable Board drafts, R15 capability documentation and the
R16 protocol metadata correction. Apply compatible main-lock dependency security
updates as partial R02 work. The other findings remain open in the remediation
record; no complete release qualification is claimed by this batch.

## Constraints

### Must
- Audit every resolved image address before connecting, pin audited DNS answers and repeat validation for each redirect.
- Preserve public HTTP/HTTPS image loading, existing response bounds and account privacy.
- Bound user profiles, memberships, completion work and Board draft capacity without silently discarding unsubmitted card text.
- Retain the original idempotency key for unknown card saves and reject stale account/page completions.
- Match accepted discovery protocol metadata and retain existing authentication checks.
- Separate verified owner private encrypted execution from unqualified shared requester and recovery capabilities in both languages.

### Must Not
- Do not disable TLS verification, proxy network isolation through unvalidated DNS or authorize execution from a notification.
- Do not replace actual qualification records with fabricated passing data or describe advisory matches as proven exploits.
- Do not merge until independent review and all current-head platform checks succeed.

## Boundaries

### Allowed Changes
- ./Cargo.lock
- crates/hagency-app/apps/article-editor/native/remote_images.rs
- crates/hagency-app/apps/article-editor/native/remote_image_tests.rs
- crates/hagency-app/src/profile/user_profile_cache.rs
- crates/hagency-app/src/hagency/board/**
- crates/hagency-app/src/hagency/ui.rs
- crates/hagency-app/resources/i18n/en.json
- crates/hagency-app/resources/i18n/zh-CN.json
- crates/hagency/src/server_admission.rs
- crates/hagency-app/README.md
- crates/hagency-app/README.zh-CN.md
- crates/hagency-app/docs/hagency-quickstart.md
- crates/hagency-app/docs/hagency-quickstart.zh-CN.md
- crates/hagency-app/docs/privacy.md
- crates/hagency-app/docs/privacy.zh-CN.md
- crates/hagency-app/packaging/release-notes.md
- crates/hagency-app/specs/task-client-quality-hardening.spec.md
- knowledge/requirements/req-client-quality-remediation.md
- dev-vibe/client-quality-remediation-2026-10-29.zh.md
- dev-vibe/client-quality-review-2026-10-29.zh.md
- docs/agent-knowledge.md
- docs/progress.md

## Acceptance Criteria

Scenario: Image network isolation checks real production request orchestration
  Test:
    Package: hagency-app
    Filter: article_app::remote_images::tests
    Level: integration
    Test Double: controlled DNS and HTTP responses plus isolated loopback transport
  Given private mapped addresses mixed DNS answers redirects rebinding and oversized streams
  When the production image fetch and actual pinned reqwest transport run
  Then unsafe targets produce no request and public redirects and bounded bodies remain usable

Scenario: Unsubmitted card text survives all navigation without duplicate writes
  Test:
    Package: hagency-app
    Filter: quick_draft_
    Level: integration
    Test Double: actual native Board widgets and exact server-response fixtures
  Given title drafts pending original writes removed lists and an account change
  When the native Board binding changes or a stale response arrives
  Then text and original write keys remain scoped to their Board while cancellation and account changes clear them

Scenario: Profile and completion working sets remain bounded and retryable
  Test:
    Package: hagency-app
    Filter: profile::user_profile_cache::
    Level: unit
    Test Double: owned cache queue and controlled timestamps
  Given visible users many memberships completion floods and dropped responses
  When lookups and finite UI batches process the workload
  Then capacities hold visible data survives and deferred or missing data can be requested again

Scenario: Admission returns the exact supported discovery protocol
  Test:
    Package: hagency
    Filter: admission_requires_supported_protocol_capabilities_and_same_origin_issuer
    Level: unit
    Test Double: discovery metadata values
  Given protocol three metadata and unsupported protocol versions
  When admission validates the metadata
  Then the accepted result reports three and unsupported versions remain rejected

## Out of Scope

This batch does not qualify all independent Octos dependencies, stream ordinary
attachments, migrate secrets, implement system approval notifications, add media
playback or replace the existing real-host release qualification. Those remain
part of the operator's active overall objective.
