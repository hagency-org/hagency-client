spec: task
name: "Current native owner remote approval publication and decisions"
tags: [active, rust, desktop]
---

This covers the execution device's private approval publication and verified
verdict consumption into its exact live PendingApproval, plus the remote owner's
verified request inbox and irreversible encrypted verdict publication. Full A4
still requires manual desktop/production OAuth qualification, per-Agent crypto identity
and encrypted Agent task/file execution.
Manual/cross-platform gates and A3 remain. This contract preserves that scope.

- Commit original owner/profile/approval/execution/device/room/proposal scope
  before SDK publication; a retry or another ledger handle cannot regain custody.
  Test: remote_publication_commits_once_and_reopen_cannot_recreate_custody
- Retaining publication custody for receipts and verdicts does not permit another
  SDK send. Transfer an owned SendPermit once; cancellation/drop never rearm it.
  Test: remote_publication_retained_receipt_custody_cannot_mint_a_second_send_permit
- Record one exact original receipt and owner decision atomically with the audit;
  an intent is neither a reusable permission nor observed tool completion.
  Test: remote_decision_is_atomic_single_use_original_device_and_audit_evidence
- Foreign owner/room/device/receipt/digest and changed original proposal fail
  without consuming the actual offered approval.
  Test: remote_wrong_owner_scope_device_receipt_and_digest_never_consume_original
  Test: remote_request_cannot_change_original_proposal_or_replace_publication_destination
- Expiry, stale proof, changed policies, execution epoch and original stop state
  cannot produce a current decision grant.
  Test: remote_policy_change_and_expiry_preserve_offered_evidence_without_deciding
  Test: remote_proof_age_execution_epoch_and_unknown_stop_are_not_grants
- Recheck mutable original authority after the actual SQLite writer wait.
  Test: remote_policy_update_during_sqlite_writer_wait_cannot_commit_a_stale_decision
- Late acceptance remains evidence and cannot replace a receipt or undo a local
  denial. A remote denial remains permanent across snapshots.
  Test: remote_late_receipt_does_not_reopen_stopped_or_locally_decided_approval
  Test: remote_decision_retains_denial_and_permanent_witnesses_through_restore
- Restored metadata cannot reconstruct live publication custody. The exact
  previous schema adds empty metadata without inventing historical evidence.
  Test: remote_snapshot_restores_all_witnesses_without_copying_live_publication_custody
  Test: pre_remote_snapshot_preserves_original_audit_and_adds_no_remote_witness
- Bound actual serialized UTF-8 bytes and retained publication count before IO;
  failed reservations never appear as publications and witnesses cannot be pruned.
  Test: remote_actual_serialized_capacity_is_permanent_and_failed_reservation_is_not_a_publication

Desktop SDK/UI tests run separately from this retained core inventory. The
crypto_tests module uses actual SDK cross-signatures, Olm/Megolm, private request
publication, decryption and a different signed owner-device verdict over a local
HTTP homeserver double, followed by the actual original journal decision. It also
rejects a newly shared room and a real unsigned device's forged verified/device
claims. Older envelope guards explicitly synthesize SDK metadata; those fixtures
alone do not qualify authentication. The separate explicit
native_owner_remote_approvals_qualification example now passes actual Synapse,
two SDKs/two NativeOwners and real provider acceptance; ordinary CI compiles it
and runs only its offline command guard. Manual native UI/production OAuth remain
separate gates. No JSON field named verified establishes trust.

- Sample the actual host clock again after acquiring the writer lock. A stale
  pre-lock time cannot extend the original proposal or SDK proof deadline.
  Test: remote_clock_sample_after_actual_writer_wait_rejects_expiry_without_moving_deadline

- Native owner publication retains the original ledger/capsule in an owned task;
  exact current gates precede reservation, SDK IO and decision application. A
  remote decision signals only the original pending receiver, once, and does not
  itself mint a reusable tool approval.
  Test-Platforms: [linux, macos]
  Test: native_remote_approval_records_original_verdict_and_signals_only_the_live_pending_once
- Local denial/stop retires future publication authority while original possible
  IO remains joined and a late original acceptance is recorded historically.
  Test-Platforms: [linux, macos]
  Test: native_remote_approval_late_receipt_survives_local_denial_and_joined_shutdown
- Unknown publication status reads cannot republish or change the original room.
  Test-Platforms: [linux, macos]
  Test: native_remote_approval_unknown_never_republishes_or_changes_destination
- Current gate or policy changes leave the offered original pending undecided.
  Test-Platforms: [linux, macos]
  Test: native_remote_approval_current_gate_or_policy_change_cannot_decide_the_pending_request
- Remote denial closes only the original pending; failed pre-reservation gate
  leaves no publication and cannot start SDK IO.
  Test-Platforms: [linux, macos]
  Test: native_remote_approval_denial_closes_only_original_pending_and_gate_precedes_reservation
- A dropped provider receiver retires future writes even while the original SDK
  publication task remains owned. Status reads cannot recreate its sender.
  Test-Platforms: [linux, macos]
  Test: native_remote_approval_closed_receiver_retires_inflight_publication_before_next_write
- Weak liveness inspection cannot retain or recreate an original decision sender.
  Test: native_pending_approval_liveness_cannot_keep_or_recreate_decision_sender

Private SDK sends use detached one-event Megolm sessions shared only with the
current account's cross-signed remote devices (1..64). No reusable outbound room
session is inserted or reused. Runtime publication consumes SendPermit, checks
current private-room/account/device scope, disables SDK write retries and keeps
actual acceptance even after authority retires. Native API and English/Chinese
execution-device UI expose publish, original status and verified-verdict read.
The destination selector shows room names from cached candidates; fresh SDK facts
authorize actual publication. Verdict discovery reads a complete bounded set of
encrypted m.reference relations to the original event, rejects incomplete or
ambiguous input and rechecks actual SDK verification. No event ID entry is needed.
The remote owner's English/Chinese inbox reads bounded encrypted history pages,
shows the complete exact proposal and can copy it. It needs no local provider or
Agent execution assignment. Deciding re-observes the original signed event and
commits possible IO to the same owner-profile ledger before returning a single
send permit; unknown, conflicting choice, reopen and snapshot restore never rearm
it. Original verdict IO remains joined at shutdown, retaining late acceptance.

- The remote reviewer cannot resend a changed choice, reconstruct original
  custody from another handle or mint any tool grant.
  Test: remote_reviewer_original_publication_cannot_rearm_after_drop_reopen_or_conflicting_choice
- Require exact reviewed digest and fresh signed owner/profile/device scope
  before reserving any outgoing verdict.
  Test: remote_reviewer_fresh_verified_scope_and_exact_reviewed_digest_precede_any_reservation
- Preserve unknown outgoing decisions across snapshots, and migrate only the
  exact previous schema with empty new metadata.
  Test: remote_reviewer_snapshot_retains_unknown_decision_and_previous_schema_migrates_empty
- Bound actual UTF-8 metadata and retained rows without pruning for retries.
  Test: remote_reviewer_capacity_counts_actual_utf8_and_never_prunes_for_a_retry
- Re-sample current host time after the actual SQLite writer wait.
  Test: remote_reviewer_clock_after_actual_writer_wait_cannot_extend_request_or_sdk_proof
- Native outgoing decisions require original verified request/digest; recording
  publication does not signal a local pending execution.
  Test-Platforms: [linux, macos]
  Test: native_remote_reviewer_exact_request_and_durable_verdict_do_not_signal_local_pending
- Unknown cannot resend; shutdown joins original possible IO and retains only
  its actual late receipt, including a denial.
  Test-Platforms: [linux, macos]
  Test: native_remote_reviewer_unknown_is_not_republished_and_late_receipt_is_joined_on_shutdown

Desktop tests cover another-device UI complete-command rendering and disabled
unknown decisions. The actual SDK crypto test now also exercises verified request
inspection, bounded inbox pagination and the real irreversible verdict publisher.

Independent Agent identity is implemented separately from approval trust. The
identity-only Server broker and desktop SDK facade pin public account/device keys
without distributing AS or homeserver login credentials. Current Agent/device
authorization epochs fence old requests; same-host authorization rotation retains
original SDK keys. Account cross-signing pins survive host reassignment. Desktop
store scope/exclusivity/missing-key/symlink tests and explicit real Synapse/native
qualification are recorded in dev-vibe/agent-crypto-identity-contract-2026-10-27.zh.md
and its validation report. Ordinary CI compiles the external qualification example
but does not run it. This does not verify Agent trust for encrypted task execution,
transfer/reset private keys, or open the existing encrypted-Room refusal gates.


- Native managed identities retain private filesystem credentials per owner/Agent/
  stable execution device; authorization and Agent epochs do not replace keys.
  Test: agent_crypto_private_material_is_scoped_and_retains_original_keys_across_epochs
- Missing or mismatched original credentials refuse replacement and preserve the
  original SDK store. A symlink cannot redirect secret reads or writes.
  Test: agent_crypto_missing_or_misbound_original_secret_cannot_generate_replacement
  Test-Platforms: [linux, macos]
  Test: agent_crypto_symlink_secret_cannot_redirect_original_credentials
- Canceled stop/shutdown observers retain shared original initialization and close
  receipts. Checked shutdown cannot report success while their work remains held.
  Test: agent_crypto_canceled_stop_waiter_keeps_original_cleanup_joined_by_shutdown
  Test: agent_crypto_shutdown_retains_initialization_receipt_after_waiter_cancellation

Desktop checks separately exercise public identity rendering and execution-device/
Agent-generation UI fences. The explicit agent_crypto_managed_qualification entry
uses actual native CreateAgent/AgentCrypto commands and the production SDK facade;
ordinary CI compiles the driver without accessing its external fixture. Private
filesystem credential protection is not an OS keychain or cross-host recovery.


- Explicit owner verification binds every original identity field before SDK IO;
  changed fingerprints or serialized trust flags cannot reach the signer. Repeated
  observers share the original verification task; stop and shutdown join it and a
  late positive observation cannot restore a retired native session.
  Test: agent_crypto_exact_verification_rejects_changed_keys_and_joins_original_on_stop

- Agent owner-event decryption yields host-only crypto evidence, never a task,
  admission, ACK or lease grant. Malformed transport headers refuse before IO.
  Test: agent_crypto_receive_rejects_malformed_headers_before_authority_or_crypto_io
- Dropping a receiving observer retains the original crypto task and its update
  guard. Checked shutdown joins it; a retired registry cannot start a new one.
  Test: agent_crypto_canceled_receive_observer_cannot_discard_original_shutdown_join

Actual owner ciphertext receipt is separately qualified by
`agent_crypto_owner_receive_qualification` against isolated Synapse. CI unit
fixtures do not prove real Olm/Megolm receipt or encrypted task execution.

- Ciphertext response data requires exact headers and the canonical original source
  commitment; serialized verification flags and plaintext fields cannot replace it.
  Test: ciphertext_wire_requires_original_canonical_commitment_and_exact_headers
- Ciphertext host transport has fixed poll/read operations with bounded batches
  and opaque original inbox IDs, never arbitrary Room/history/media URLs.
  Test: ciphertext_wire_data_cannot_choose_history_urls_filters_or_unbounded_batches
