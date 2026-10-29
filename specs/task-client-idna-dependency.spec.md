spec: task
name: "Replace the obsolete mdurl IDNA dependency without changing Matrix formatting"
satisfies: [REQ-CLIENT-QUALITY-REMEDIATION]
tags: [active, rust, desktop, security, dependencies]
---

## Intent

Repair the remaining main-lock idna 0.3.0 path through mdurl 0.3.1 with a
provenance-preserving local copy and idna 1.1.0, plus two explicit anonymous
lifetime annotations required by the existing native strict-warning build. Keep the actual Matrix formatter's
percent encoding and raw punycode behavior unchanged. This is one dependency
repair, not a qualification of every independent lockfile or IDN display policy.

## Constraints

### Must
- Preserve all original mdurl source behavior tests documentation and complete MIT attribution with exact registry package and upstream commit identity; only two explicit anonymous return lifetime annotations may accompany the dependency change.
- Keep the package version 0.3.1 and disclose the local dependency and lifetime-syntax changes rather than claim a new upstream release.
- Require idna 1.1.0 through the local mdurl manifest and preserve both formatting API signatures.
- Preserve actual Matrix percent encoding decoding raw punycode relative Matrix URI and JavaScript oracle behavior.
- Run all133 original upstream unit and parser tests plus doctests and direct valid and invalid IDNA formatting regressions.
- Record a fresh main-lock audit and inverse dependency graph with actual remaining counts and keep independent Octos debt open.
- Keep native transport account authorization SDK and all source caches unchanged.

### Must Not
- Do not change the legacy JavaScript formatter fixtures or raw punycode policy to make a test pass.
- Do not modify registry or Git dependency checkouts or use advisory ignores.
- Do not claim all dependencies or independent Octos locks are repaired from this one path.
- Do not push or open a PR before independent review of this branch.

## Boundaries

### Allowed Changes
- .github/workflows/ci.yml
- .github/workflows/engine-checks.yml
- tools/native/check-rust-spec-bindings.mjs
- tools/native/check-rust-spec-bindings-vendor.test.mjs
- tools/check-workspace-layout.py
- typos.toml
- ./Cargo.toml
- ./Cargo.lock
- vendor/mdurl/**
- specs/task-client-idna-dependency.spec.md
- knowledge/decisions/adr-050-native-matrix-formatting.md
- dev-vibe/client-idna-dependency-2026-10-30.zh.md
- docs/progress.md
- docs/agent-knowledge.md

### Forbidden
- SDK App transport account session credentials Matrix formatting implementation and retained JavaScript oracle sources.
- Registry Git caches primary source checkouts and independent Octos dependency migrations.

## Acceptance Criteria

Scenario: Updated mdurl keeps both formatting APIs while refusing masked ASCII IDNA aliases
  Test:
    Package: mdurl
    Filter: quality_idna_
    Level: integration
    Test Double: actual mdurl formatters and idna 1.1 algorithms without network
  Given valid Unicode domains and invalid ASCII-only or empty ACE labels
  When the actual computer and human URL formatting APIs process them
  Then valid domains remain usable and invalid ACE labels do not collapse into a trusted ASCII hostname

Scenario: Actual Matrix formatting preserves its retained content oracle and safety boundaries
  Test:
    Package: hagency-matrix-format
    Filter: native_matrix_format_
    Level: integration
    Test Double: actual Matrix formatter and retained JavaScript-derived corpus
  Given fullwidth raw punycode Unicode percent encoded relative and Matrix links
  When original formatter behavior and capacity checks run with the patched dependency
  Then retained byte exact output content preservation and safety boundaries remain unchanged

## Out of Scope

- The separate Octos lockfile dependency migration.
- New browser or IDN presentation policy and raw punycode changes.
- SDK network behavior OS credential migration and release qualification.
