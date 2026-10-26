spec: task
name: "Desktop owner client and retained engine boundary"
tags: [active, rust, desktop]
---

The desktop application is the only product entry point. The engine is a local
library dependency. There is no CLI Web console, static page bundle, user daemon
installer, or headless product release. Historical CLI and browser selectors in
other contracts are retired evidence, never relabelled as passed.

## Desktop engine and OAuth

- Native management uses the authenticated in-process facade, pinning the Matrix account.
  Test: native_in_process_facade_pins_matrix_account_without_exposing_credentials
  Production caller: hagency::native_owner::open_with_matrix
- The only actual listener serves the loopback OAuth callback; management APIs and documents are absent.
  Test: native_desktop_listener_exposes_only_oauth_callback
- Account switching revokes old grants and pending tickets.
  Test: account_switch_revokes_all_browser_grants_and_pending_ticket_without_retiring_host
- Account profiles keep Pasion identity and policy isolated.
  Test: native_account_profiles_switch_revoke_old_tabs_and_pin_pasion_identity
- Space discovery and creation retain owner authorization and bounded pagination.
  Test: native_owner_space_candidates_use_own_oauth_and_paginate_partial_failures
- Private owner state still refuses legacy data and permits only one process owner.
  Test: old_state_and_foreign_marker_are_refused_without_reading_or_rewriting_old_data
  Test: fresh_format_is_reopenable_and_has_one_process_owner

The desktop OAuth integration tests also live in hagency-app/src/login/oauth_tests.rs.
The normal desktop CI runs them with all application library tests. No test uses
personal provider credentials or paid model inference.

## Original file delivery history

- Owner history refreshes the exact ended execution and original request without
  acquiring a lease, restarting a model, preparing bytes or sending a replacement.
  SDK identity loss during the read cannot release a local capacity hold.
  Test-Platforms: [linux, macos]
  Test: native_owner_file_history_refreshes_ended_originals_without_restarting_or_resending
- Final preparation absence retains immutable original send and approval evidence
  across reopen/restore, and never grants a second send for the original identity.
  Test-Platforms: [linux, macos]
  Test: history_final_absence_is_permanent_scoped_and_never_a_new_send_grant
- Pre-history snapshots preserve existing file witnesses without inventing proof.
  Test-Platforms: [linux, macos]
  Test: pre_history_snapshot_preserves_existing_sender_witness_and_adds_no_absence
- Bounded owner/Agent pages preserve exact original scopes and stable ordering.
  Test-Platforms: [linux, macos]
  Test: file_history_pages_preserve_scope_order_and_exact_original_receipts
- Temporary absence retains the actual128MiB capacity hold; only authoritative
  final absence releases file capacity while retaining the original identity.
  Test-Platforms: [linux, macos]
  Test: final_absence_releases_actual_capacity_but_not_permanent_identity

- Cross-repository file qualification uses current Server PG/HTTP/AS/workers,
  authenticates original bytes larger than128KiB, retains a lost queue ACK as
  unknown audit evidence and reads original receipts after both sides restart.
  This external fixture scenario is ignored by ordinary CI and must be run
  explicitly; Pasion/Matrix/provider doubles are identified in its evidence.
  Test-Platforms: [linux, macos]
  Test: native_owner_current_server_file_round_trip_and_restart_preserve_original_bytes

Real Codex file qualification uses the public NativeOwner facade in the
native_owner_files_qualification example, whose executable implements the real
guardian entry before owner initialization. It requires explicit paid opt-in and
an isolated loopback Server fixture. A libtest executable is not a programming
guardian and cannot substitute for this entry. See the cross-repository file
validation report for exact commands, original failures and remaining gates.
