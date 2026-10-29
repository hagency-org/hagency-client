spec: task
name: "Retain original receive fixture receipts without weakening bounded production RPC"
satisfies: [REQ-CLIENT-QUALITY-REMEDIATION]
tags: [active, rust, desktop, tests]
---

## Intent

Repair the receive workspace fixture's assumption that durable grant setup must
always return inside the production writer's two-second observation bound.
Keep the real Started handoff and original unique receive grant while retaining
all production unknown authority retirement and filesystem assertions.

## Constraints

### Must
- Keep production DomainStore RPC reply bounds and OutcomeUnknown handling unchanged.
- Gate the specialized original-writer fixture receipt behind the explicit default-off test-fixture feature.
- Validate the original current Started capability scope event byte limit and facts before returning one noncloneable grant.
- Retain actual writer queue and byte admission and wait for the original receipt with a finite external fixture watchdog.
- Demonstrate controlled original production timeout remains Unknown while later commit does not recover or duplicate a grant.
- Preserve the actual execution probe Started handoff invalid capability expired write retirement and unchanged physical contents assertions.
- Preserve original Windows failure evidence and distinguish proven RPC semantics from unobserved physical Windows latency.

### Must Not
- Do not retry a nonidempotent write recover a lost grant fabricate Started or change unknown into success.
- Do not expose a general public mutable repository callback or open a second owner for fixture setup.
- Do not widen production deadlines add ignored cases or allow CI failure.
- Do not start a real provider model or modify credentials.

## Boundaries

### Allowed Changes
- ./Cargo.lock
- crates/hagency-store/Cargo.toml
- crates/hagency-store/src/domain_worker.rs
- crates/hagency-store/src/domain_worker/receive_fixture.rs
- crates/hagency-execution/Cargo.toml
- crates/hagency-execution/tests/owned/receive.rs
- specs/task-receive-fixture-receipt.spec.md
- dev-vibe/receive-fixture-receipt-2026-10-30.zh.md
- docs/progress.md
- docs/agent-knowledge.md

### Forbidden
- Production RPC deadline policy authorization state transitions schemas SDK runtime provider and original source checkouts.

## Acceptance Criteria

Scenario: Actual original writer timeout stays unknown and fixture receipt stays unique
  Test:
    Package: hagency-store
    Filter: native_receive_fixture_
    Level: integration
    Test Double: actual SQLite original writer with thread-local statement gate and original RPC timers
  Given a real begin transaction held beyond the production reply observation bound
  When the production request times out or the fixture retains its original receiver
  Then production returns Unknown without a grant and the fixture obtains exactly one original grant only after actual completion

Scenario: Original execution workspace retirement still fences actual file writes
  Test:
    Package: hagency-execution
    Filter: native_receive_workspace_authority_retirement
    Level: integration
    Test Double: actual offline child Started receipts original store and private filesystem
  Given a real Started workspace original grants and existing completed or not-yet-created files
  When the actual operation is cancelled after the original setup receipts
  Then retired bindings reject new writes and revalidation while already written bytes remain unchanged

## Out of Scope

- Attribution of the original Windows physical delay without original timing instrumentation.
- Production RPC latency optimization credential migration and provider execution.
- Broad execution harness or SDK refactoring.
