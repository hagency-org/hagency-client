spec: task
name: "Original native owner remote approval evidence"
tags: [active, rust, desktop]
---

This is the durable evidence stage of A4 in the owner capability plan. Full A4
still requires encrypted publication, current native runtime/UI consumption,
private owner rooms, verified remote device decisions and encrypted Agent task
execution. This contract does not redefine A4 or enable those unfinished flows.

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

Desktop SDK guard tests in backend/remote_approvals/tests.rs run in the App suite,
separately from the retained core inventory. They explicitly synthesize SDK
metadata; they do not qualify real cross-signing, publication or runtime wiring.
The adapter fetches bounded current private encrypted room state, decrypts via
the actual SDK, rejects plain/unverified/edited/self-origin/forwarded events,
refreshes current owner identity/device keys and compares the original reply and
digest. No JSON field named verified can establish those facts.

- Sample the actual host clock again after acquiring the writer lock. A stale
  pre-lock time cannot extend the original proposal or SDK proof deadline.
  Test: remote_clock_sample_after_actual_writer_wait_rejects_expiry_without_moving_deadline
