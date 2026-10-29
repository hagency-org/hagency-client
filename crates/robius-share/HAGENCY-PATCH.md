# Guarded native share presentation

Vendored from Project Robius revision `cca3cc3dc99d8fcb6c69a127cd28d47ba8f3e3eb`,
`crates/share` (version 0.3.1), under its original MIT license. Original authors
and copyright notices are retained. Only this package is patched; other Robius
packages retain their original Git source and lockfile revision.

Hagency adds background preparation and an authority callback around actual native
presentation and Windows delayed payload delivery. Original `ShareSheet::share`
retains its semantics: successful return means presented, not sharing completed.
Guarded Android/iOS and Linux mixed save bundles are explicitly unsupported.
There is no completion callback, so callers must retain staged files for their
stated TTL or until account retirement/shutdown; no completed/cancelled verdict
is inferred from presentation.

`ShareSheet::prepare` performs filesystem/WinRT and Linux bus authentication on a
blocking worker. `PreparedShare::present_guarded` returns an owned `PendingShare`.
On Linux the actual portal operation is a single nonblocking send, and response
fragments are polled outside the UI and account fence with a 64 KiB control limit
and five minute observation deadline. Fallback is allowed only when preparation
failed before any portal Open request; a partial send or lost reply is unknown,
not a trigger for a second launch. xdg-open is spawned under the guard and its
original Child is retained until actual exit/reap is observed.

The caller must retain/poll/cancel PendingShare on its original worker. Cancellation
closes this process's portal socket or kills and observes only its own launcher;
a descriptor or copy already handed to a portal or recipient cannot be revoked.
There is no promise of erasing external application copies.
