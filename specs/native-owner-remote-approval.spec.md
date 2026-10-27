spec: task
name: "Current native owner remote approval publication and decisions"
tags: [active, rust, desktop]
---

This covers the original execution device's private approval publication and
verified verdict consumption into its exact live PendingApproval. Full A4 still
requires the remote approver inbox/outgoing-verdict UI, actual Synapse/native
qualification, per-Agent crypto identity and encrypted Agent task/file execution.
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
alone do not qualify authentication. Real Synapse, native approver UI and provider
acceptance remain separate gates. No JSON field named verified establishes trust.

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
The remote approver inbox and outgoing-verdict UI remain separate work.
