spec: task
name: "Release native ownership locks before acknowledging store shutdown"
tags: [active, rust, storage, ownership, ci]
---

CI job 122450182413 at ccda4501 failed the inline factory restart: ordinary domain
shutdown returned success, then immediate DomainRepository::open returned Locked.
Retaining a cloned ownership descriptor reproduces the same acknowledged-but-
locked state deterministically in both domain and custody writers. The
[Rust File locking contract](https://doc.rust-lang.org/std/fs/struct.File.html#method.unlock) retains a lock until all duplicated/inherited handles close or
unlock is called. The old lock error mapping discarded the native error, so the
historic errno and any retaining descriptor/child were not recorded. The
controlled regression establishes a real release defect, not that identity.

This task addresses that successful-acknowledgement defect, separately from
ADR120's checkpoint/close-stall task. Keep its SQLite policy, phase meanings,
two-second bounds, receiver-before-ACK ordering and existing error vocabulary.
Only actual WouldBlock maps to Locked; other lock errors retain their IO detail.

## Boundaries

- Allowed: database ownership-release helper, domain/custody close methods and
  their workers, existing shutdown test call sites, the two actual-handle
  regression tests, this contract and task documentation.
- Forbidden: schema, dependency, private-file/share policy, live data, deadline,
  retry, skipped test or SQLite checkpoint changes. Never release ownership
  while its original database connection remains open.

After the real connection closes, explicitly unlock and close its original
ownership handle before a successful ACK. A release error is logged and closes
the reply without an ACK, retaining Unavailable/ReplyClosed (or the original
timeout if its bound expires), never manufacturing success. Reopening has no
retry. Observed shutdown records actual field release in the existing order.

Scenario: Domain shutdown releases its original lock despite a retained copy
  Test: native_domain_shutdown_releases_inherited_ownership
  Given a real repository, writer and retained clone of the original lock
  When ordinary or observed shutdown acknowledges
  Then the writer is closed and a replacement repository opens immediately
  And closing the old clone cannot unlock that still-exclusive replacement

Scenario: Custody shutdown releases its original lock despite a retained copy
  Test: native_custody_shutdown_releases_inherited_ownership
  Given the same actual-handle condition for the custody writer
  When ordinary or observed shutdown acknowledges
  Then immediate reopen and replacement exclusivity have the same guarantees

Scenario: Factory restart uses its original durable state without a lock retry
  Test-Platforms: [linux, macos]
  Test: native_configured_fleet_reattaches_after_restart
  Given the actual configured offline factory, SDK stores and original owner
  When every original service closes and the same domain store reopens
  Then the recorded agent reattaches without another registration or key upload

Existing paused connection/ownership shutdown tests remain fatal on their
original timeout and retain exact phase, release and acknowledgement assertions.
