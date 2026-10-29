spec: task
name: "Open Matrix room and event links with explicit membership and exact conversation scope"
satisfies: [REQ-CLIENT-QUALITY-REMEDIATION]
tags: [active, rust, desktop, matrix, navigation]
---

## Intent

Repair R13 application link routing for Matrix room IDs, aliases and event
permalinks. Preserve routing servers and explicit joining consent, discover a
thread only from the actual SDK decrypted event, and keep navigation tied to
its original account and newest user intent. System URL delivery on macOS and
Windows is outside this batch and remains unqualified.

## Constraints

### Must
- Parse Matrix links with the pinned Ruma APIs, retain via servers and consume recognized malformed or unavailable Matrix links with an explicit native error.
- Never join from link parsing, preview, event resolution or a URI action without a fresh explicit confirmation.
- Resolve aliases and inspect events through the original authenticated SDK client and preserve server authorization errors.
- Select the actual main room or thread before invoking the existing cancellable event focus, retaining composer and thread identity.
- Fence asynchronous results and confirmation by request nonce, account epoch, original WeakClient identity and current client ownership.
- Cancel superseded or dismissed navigation and discard late responses without claiming an already issued join was undone.
- Use actual native widget actions and SDK loopback HTTP/context fixtures; keep fixture evidence distinct from actual desktop OS URL delivery.
- Scope queued and already visible failure popups to the original input epoch.
- Carry the original Ticket on internal Home and Contacts routing; any ordinary user navigation replaces the old intent even while delivery or context remains pending.
- Finish the original lazy Home child draw before posting target delivery, and retain permalink initialization provenance through a stale delayed LoadDock without entering the management factory.
- Keep passive Room and Contacts observation free of owner creation and saved runtime startup; retain explicit login and mutation factory calls.

### Must Not
- Do not fall back to opening a recognized Matrix link externally to hide native failure.
- Do not create a DM or send a message from an incoming link.
- Do not touch real accounts, credentials, paid inference, deployments or Makepad source.
- Do not claim macOS or Windows operating-system deep-link delivery from application action tests.

## Boundaries

### Allowed Changes
- crates/hagency-app/src/matrix_links.rs
- crates/hagency-app/src/matrix_links/**
- crates/hagency-app/src/lib.rs
- crates/hagency-app/src/hagency/backend.rs
- crates/hagency-app/src/app.rs
- crates/hagency-app/src/utils.rs
- crates/hagency-app/src/join_leave_room_modal.rs
- crates/hagency-app/src/room/room_input_bar.rs
- crates/hagency-app/src/home/room_screen.rs
- crates/hagency-app/src/home/room_navigation.rs
- crates/hagency-app/src/home/project_tree.rs
- crates/hagency-app/src/home/agent_runtime_notice.rs
- crates/hagency-app/src/profile/agent_details.rs
- crates/hagency-app/src/shared/popup_list.rs
- crates/hagency-app/src/home/space_lobby.rs
- crates/hagency-app/src/home/main_desktop_ui.rs
- crates/hagency-app/src/home/home_screen.rs
- crates/hagency-app/src/home/mobile.rs
- crates/hagency-app/src/home/mobile_chat_info.rs
- crates/hagency-app/src/shared/web_browser.rs
- crates/hagency-app/src/sliding_sync.rs
- crates/hagency-app/src/sliding_sync/history_tests.rs
- crates/hagency-app/src/shared/html_or_plaintext.rs
- crates/hagency-app/resources/i18n/en.json
- crates/hagency-app/resources/i18n/zh-CN.json
- crates/hagency-app/specs/task-client-quality-matrix-links.spec.md
- dev-vibe/client-quality-matrix-links-2026-10-30.zh.md
- docs/agent-knowledge.md
- docs/progress.md

## Acceptance Criteria

Scenario: Native parsing keeps the exact Matrix target and routing servers
  Test:
    Package: hagency-app
    Filter: matrix_links::tests::parsing
    Level: unit
  Given valid room alias event and action links plus malformed Matrix links
  When the original native URL entry parses them
  Then the typed target via and action are retained and recognized failures never open externally

Scenario: Membership changes require actual native confirmation
  Test:
    Package: hagency-app
    Filter: matrix_links::tests::confirmation
    Level: integration
    Test Double: actual native modal actions and original SDK loopback HTTP
  Given a link to an unjoined room with routing servers
  When preview completes then the user confirms or dismisses
  Then no join precedes confirmation and only a fresh confirmation sends the original via servers

Scenario: Event context selects the original main or threaded conversation
  Test:
    Package: hagency-app
    Filter: matrix_links::tests::event
    Level: integration
    Test Double: actual SDK sync event and context endpoints and native navigation actions
  Given accessible main and thread events plus inaccessible or invalid event context
  When the application resolves and focuses each original permalink
  Then the exact event and verified thread root select the matching composer and unavailable events never trigger joining or external opening

Scenario: Superseded and retired account intents never navigate or join
  Test:
    Package: hagency-app
    Filter: matrix_links::tests::scope
    Level: integration
    Test Double: isolated original SDK accounts and delayed loopback replies
  Given queued resolution confirmation and navigation from an old intent or account
  When a newer request cancellation or account change occurs
  Then late results and stale native confirmations are rejected without alternate target substitution

Scenario: Desktop and compact layouts consume the same scoped event target
  Test:
    Package: hagency-app
    Filter: matrix_links::tests::navigation
    Level: integration
    Test Double: actual native widgets and timeline actions
  Given main and thread permalink navigation on desktop and compact layouts
  When the exact original target is delivered to the selected RoomScreen
  Then focus keeps its TimelineKind thread root and existing cancellation behavior

Scenario: Cancelling an original focused event drops its pending build
  Test:
    Package: hagency-app
    Filter: matrix_link_focus_stops_when_original_event_search_is_cancelled
    Level: integration
    Test Double: original production timeline request watch and pending build ownership
  Given an original event target whose focused build is still pending
  When its original search request is removed
  Then the original build is dropped before its deadline without changing a replacement composer

Scenario: Native link fixtures never start providers or use real account state
  Test:
    Package: hagency-app
    Filter: matrix_links::tests::navigation
    Level: integration
    Test Double: isolated loopback SDK accounts private temporary profiles and actual native widgets
  Given native room thread and direct-contact links in fresh private profiles
  When desktop and compact navigation consume the original SDK results
  Then no execution owner bootstrap provider connection paid inference or real account credentials are used

## Out of Scope

Local loopback fixtures do not qualify production federation, E2EE key recovery
on a real account, or system URL delivery. Preserve HTTP failures and original
unknown joining outcomes. Independent review and current-head CI remain required before merging this
branch. A draft pull request may collect that CI evidence.
