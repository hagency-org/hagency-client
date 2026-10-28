spec: task
name: "CI publication rotation budget and isolated long owned fixture"
tags: [active, rust, fixture, ci]
---

Changes are limited to the Palpo publication lifecycle fixture, read-only owned
failure diagnostics, and hosted CI routing. No production HTTP/RPC/SQLite/lease
bound, retry, scope check, cleanup assertion or allowed outcome changes.

- Publication rotation withholds an original HTTP reply while durable replacement
  work completes beyond the separate tight-header fixture budget. Use the
  existing production default finite network limits for this positive lifecycle
  scenario; preserve exact Generation, byte equality, restart and new sequence
  assertions. The independent tight-header/body deadline tests remain unchanged.
  Test: native_outbound_http_publication_frozen_restart_and_rotation
- The original 45s owned fixture must observe a real quiet child for at least31s
  and complete without lost authority. On Windows it runs once as a required
  exact selector before the concurrent suite, excluded only from that later
  invocation. Any failure is fatal; it is never retried or relabelled as passed.
  Test: native_owned_turn_long_lifetime

The historical Windows reply timeout proves only a failed2s domain reply, not
its disk or scheduler cause. Failure text can show the most recent domain
unknown/dequeued attribution; that diagnostic is never execution authority.
