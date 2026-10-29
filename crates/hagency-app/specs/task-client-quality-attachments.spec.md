spec: task
name: "Bound ordinary attachment transfer and explain existing media actions"
satisfies: [REQ-CLIENT-QUALITY-REMEDIATION]
tags: [active, rust, desktop, security, performance]
---

## Intent

Repair R03 ordinary attachment download resource growth with private streamed
files and finite operation and disk budgets. Repair the App-side actual-source
and byte checks in R07 without claiming SDK background upload streaming or a
global send-queue budget unless those are actually delivered. Clarify R14 audio
and video playback limitations while preserving existing download and share.

## Constraints

### Must
- Preserve authenticated Matrix plain and encrypted download endpoints, current session refresh and ciphertext integrity verification before publishing files.
- Bound network operations to four, queued metadata to sixteen, per-operation buffers to64KiB, ordinary files to1GiB and actual temporary bytes to4GiB per native client process across ciphertext plaintext Save As scratch and share copies; sharing retains its additional256MiB/32-file sublimits.
- Measure actual remote-transfer allocations with an isolated real workload: peak Rust heap at most32MiB and32MiB-to256MiB input peak difference at most8MiB; quota counters alone cannot prove bounded memory.
- Charge quota to the actual file and IO until verified removal; cancellation and account retirement cannot release still-live blocking IO custody.
- Preserve explicit Save As selection and existing sharing; a file path handed to a save API must not silently bypass the original dialog.
- Bound Theme previews to256KiB and Markdown to512KiB before conversion to in-memory content.
- For upload source checks open and inspect the actual file, bound accepted bytes independently of stale selection metadata and server-limit availability, and include avatar uploads.
- Preserve local echo, offline sends, replies, encrypted sources, progress, cancellation and original unknown outcomes.
- Keep explicit audio and video download/share guidance accurate in English and Chinese and verify actual visible widget actions.

### Must Not
- Do not call SDK get_media_file or AttachmentSource::File streaming without inspecting its actual buffering path.
- Do not claim SDK background send-queue or restored upload budgets from an App-only permit.
- Do not expose ciphertext or raw MXC URLs directly to an external application in place of authenticated decryption.
- Do not change OS credentials or fabricate release qualifications.
- Do not merge before independent review and current-head platform checks pass.

## Boundaries

### Allowed Changes
- .github/workflows/app-builds.yml
- ./Cargo.toml
- ./Cargo.lock
- crates/hagency-app/Cargo.toml
- crates/hagency-app/src/attachment_transport.rs
- crates/hagency-app/src/attachment_transport/**
- crates/hagency-app/examples/attachment_memory_soak.rs
- crates/hagency-app/examples/chat_web_browser.rs
- crates/hagency-app/examples/theme_mvp.rs
- tools/test_attachment_memory_soak.py
- crates/hagency-app/src/transfer_budget.rs
- crates/hagency-app/src/upload_source.rs
- crates/hagency-app/src/lib.rs
- crates/hagency-app/src/media_cache_reader.rs
- crates/hagency-app/src/media_cache_reader/**
- crates/hagency-app/src/media_transport.rs
- crates/hagency-app/src/media_transport/**
- crates/hagency-app/src/app.rs
- crates/hagency-app/src/account_session.rs
- crates/hagency-app/src/sliding_sync.rs
- crates/hagency-app/src/shared/attachment_download.rs
- crates/hagency-app/src/shared/popup_list.rs
- crates/hagency-app/src/settings/theme_studio.rs
- crates/hagency-app/src/login/login_screen.rs
- crates/hagency-app/src/shared/web_browser.rs
- crates/hagency-app/src/shared/file_upload_modal.rs
- crates/hagency-app/src/history_store/fairness_tests.rs
- crates/hagency-app/src/home/upload_progress.rs
- crates/hagency-app/src/home/room_screen.rs
- crates/hagency-app/src/home/room_screen/**
- crates/hagency-app/src/temp_storage.rs
- crates/hagency-app/src/private_share_files.rs
- crates/hagency-app/src/private_share_tests.rs
- crates/hagency-app/resources/i18n/en.json
- crates/hagency-app/resources/i18n/zh-CN.json
- crates/robius-file-picker/**
- crates/hagency-app/specs/task-client-quality-attachments.spec.md
- dev-vibe/client-quality-remediation-2026-10-29.zh.md
- dev-vibe/client-quality-review-2026-10-29.zh.md
- docs/progress.md
- docs/agent-knowledge.md

## Acceptance Criteria

Scenario: Ordinary authenticated attachments remain bounded and verified before use
  Test:
    Package: hagency-app
    Filter: attachment_transport::tests
    Level: integration
    Test Double: actual SDK authenticated HTTP transport controlled streaming server and real private files
  Given chunked oversized encrypted corrupt queued cancelled and retired-account transfers
  When the production transfer worker streams bytes and verifies encrypted content
  Then invalid transfers return errors without publishing files while actual IO memory and disk usage stay bounded and Theme previews above256KiB or Markdown above512KiB are rejected

Scenario: Actual upload sources cannot grow beyond accepted byte limits
  Test:
    Package: hagency-app
    Filter: upload_source::tests
    Level: integration
    Test Double: actual selected files and mutation fixtures
  Given source growth truncation replacement stale metadata and a missing server limit
  When the production upload-source reader opens and consumes the file
  Then actual file identity and accepted bytes decide the result within the independent application limit

Scenario: Native media rows expose correct existing operations
  Test:
    Package: hagency-app
    Filter: media_attachment_actions_
    Level: integration
    Test Double: actual native message widgets and Matrix media event fixtures
  Given plain and encrypted audio and video messages and a recycled text row
  When the timeline paints and its visible download or share control is clicked
  Then the correct complete attachment is selected and recycled controls cannot act on a stale source

Scenario: Original SDK cache IO retains bounded custody after observer cancellation
  Test:
    Package: hagency-app
    Filter: media_cache_reader::tests
    Level: integration
    Test Double: controlled SDK MediaStore wrapper around MemoryMediaStore plus actual SQLite fixtures
  Given a held SDK cache operation cancelled readers writers and thumbnail or local echo requests
  When an observing future retires or queued work exceeds its operation or byte budget
  Then original SDK IO remains owned until completion queued work stays bounded and stale results cannot publish

Scenario: Existing bounded image previews preserve authenticated transport and cache formats
  Test:
    Package: hagency-app
    Filter: media_transport::tests
    Level: integration
    Test Double: actual SDK cache and controlled authenticated HTTP fixtures
  Given thumbnail encrypted and locally queued image sources
  When the preview fetch uses the bounded original cache adapter
  Then original request formats limits endpoint negotiation and integrity checks remain enforced

Scenario: Late attachment diagnostics cannot leak into a replacement account
  Test:
    Package: hagency-app
    Filter: shared::popup_list::scoped_popup_tests
    Level: integration
    Test Double: original popup queue and actual native popup widget
  Given queued or displayed attachment notifications from the original account
  When the account epoch changes or logout starts before UI consumption
  Then stale scoped notifications are removed while current and ordinary global notifications retain their behavior

## Out of Scope

Operating system credential migration, remaining external dependency migrations,
Matrix permalink navigation and in-app media players remain separately tracked.
Any SDK send-queue buffering not changed remains an explicit R07 limitation.
