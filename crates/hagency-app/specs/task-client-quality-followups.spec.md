spec: task
name: "Protect attachment sharing, schedule fair history, expose exact pending reviews and bind releases"
satisfies: [REQ-CLIENT-QUALITY-REMEDIATION]
tags: [active, rust, desktop, security, performance]
---

## Intent

Implement the second reviewed batch: R04 source-bound release gates, R06 private
plaintext share lifecycle, R08 callback lock and UI disk isolation, R10 fair
history replay and R12 account-wide pending approval discovery. Repair linked
worktree Git metadata rebuilds discovered during validation (R17). Other review
findings remain open; unavailable real-host evidence must still refuse release.

## Constraints

### Must
- Keep plaintext share files private and bounded, and retain cleanup custody until actual removal or explicit unknown outcome.
- Recheck original account and presentation custody at native IO, avoid blocking disk or portal replies on the UI, and invoke download callbacks outside the global lock.
- Schedule finite history pages fairly with exact account and room gates; cancellation cannot release a guard still owned by actual SQLite IO.
- Discover only pending work in existing live local owners without startup, restore, lease creation or model execution.
- Open the exact reviewed Agent, binding, room, proposal and digest; discovery never submits an approval.
- Require successful engine matrix and source-bound real qualification before public release; placeholders remain unqualified.
- Watch actual ordinary, packed and linked Git paths without nonexistent rebuild triggers.

### Must Not
- Do not reinterpret skipped, failed, cancelled or unknown observations as passing.
- Do not publish or mint passing qualification records without real evidence.
- Do not merge before independent review and current-head platform CI succeeds.

## Boundaries

### Allowed Changes
- .github/workflows/**
- ./Cargo.toml
- ./Cargo.lock
- crates/robius-share/**
- crates/hagency-app/Cargo.toml
- crates/hagency-app/build.rs
- crates/hagency-app/build_support/**
- crates/hagency-app/src/lib.rs
- crates/hagency-app/src/app.rs
- crates/hagency-app/src/temp_storage.rs
- crates/hagency-app/src/private_share_files.rs
- crates/hagency-app/src/private_share_tests.rs
- crates/hagency-app/src/shared/attachment_download.rs
- crates/hagency-app/src/sliding_sync.rs
- crates/hagency-app/src/history_store.rs
- crates/hagency-app/src/history_store/**
- crates/hagency-app/src/home/room_history.rs
- crates/hagency-app/src/home/rooms_sidebar.rs
- crates/hagency-app/src/home/home_screen.rs
- crates/hagency-app/src/hagency/backend.rs
- crates/hagency-app/src/hagency/ui.rs
- crates/hagency-app/src/hagency/ui/approval_discovery.rs
- crates/hagency-app/resources/i18n/en.json
- crates/hagency-app/resources/i18n/zh-CN.json
- crates/hagency/src/console/native.rs
- crates/hagency/src/console/native/pending_approvals.rs
- crates/hagency/src/console/owned_runtime.rs
- crates/hagency/src/console/owned_runtime/pending.rs
- crates/hagency/src/console/owned_runtime/http_tests.rs
- crates/hagency/tests/qualification.rs
- crates/hagency/qualification/two-agent.json
- tools/native/check-release-qualifications.py
- tools/native/test-release-qualifications.py
- crates/hagency-app/specs/task-client-quality-followups.spec.md
- dev-vibe/client-quality-remediation-2026-10-29.zh.md
- docs/progress.md
- docs/agent-knowledge.md

## Acceptance Criteria

Scenario: Private share custody remains bounded through presentation retirement and cleanup
  Test:
    Package: hagency-app
    Filter: temp_storage::
    Level: integration
    Test Double: isolated real private files and owned presentation workers
  Given staging failure account retirement replacement files quota exhaustion and worker shutdown
  When the original share lifecycle performs filesystem and UI operations
  Then private identities remain checked budgets stay charged until cleanup and stale presentations cannot act

Scenario: Download completion callbacks can safely reenter the registry
  Test:
    Package: hagency-app
    Filter: download_callback_tests
    Level: unit
    Test Double: real callback map and reentrant callbacks
  Given completed or cancelled download callbacks
  When the registered callback executes
  Then the registry mutex has already been released

Scenario: History pages converge without starving another room or releasing live IO custody
  Test:
    Package: hagency-app
    Filter: history_store::fairness_tests
    Level: integration
    Test Double: actual encrypted history index SDK and local HTTP source
  Given a held room history source another active room cancellation and account retirement
  When production forward catchup runs with finite workers
  Then the other room converges replay persists and original IO retains its exact gate

Scenario: Global approval discovery preserves exact visible review targets and never starts an owner
  Test:
    Package: hagency-app
    Filter: approval_discovery::tests
    Level: integration
    Test Double: actual native widgets isolated real SDK account and local HTTP observer
  Given existing pending targets stale bindings changed accounts and no existing owner
  When the inbox draws refreshes and routes a selected row
  Then only the painted exact target opens and observation cannot bootstrap login restore leases or decisions

Scenario: Engine pending summaries paginate only existing live owner work
  Test:
    Package: hagency
    Filter: pending_approvals
    Level: integration
    Test Double: actual NativeOwner pending requests and local HTTP counters
  Given two agents more than one page and retired cursors
  When the native summary command reads pending work
  Then pages stay bounded complete and scoped without provider calls or new leases

Scenario: Git metadata watches follow actual linked worktree and packed ref inputs
  Test:
    Package: hagency-app
    Filter: git_build_paths::
    Level: integration
    Test Double: actual temporary Git repositories and linked worktrees
  Given ordinary linked packed detached and source archive layouts
  When build metadata input paths are collected
  Then all watches refer to original existing paths and source archives have no invented Git paths

## Out of Scope

Ordinary attachment byte streaming and upload limits, operating system credential
migration, remaining dependency advisories, Matrix permalink routing and media
playback remain tracked under the overall requirement. Python release preflight
self-tests and actionlint are executed separately from Cargo-only lifecycle.
