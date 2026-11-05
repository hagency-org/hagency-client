---
kind: decision
id: ADR-192
status: Accepted
---

## Context

An owner's pool is bound to the owner root that created it:
- the executor device (id, generation, token) in `server-login-sdk-resume.json`, whose `scope` is a digest of the root's path;
- the management initializer, which names the ledger's registry id;
- the per-Agent E2EE devices;
- the provider namespaces, whose keychain references are derived from paths inside the root.

Two programs created roots for the same account:
- Desktop at `<desktop data dir>/native-owner/<blake3(origin, owner)>`;
- the daemon at `<daemon home>/owners/<sha256(origin, owner)>`, after its own sign-in.

A daemon root that holds no resources reports "No pooled resources are activated on this device". Its enrollment also makes Desktop fail with `daemon_owner_root_conflict`. Moving a pool needed a staged handover from Desktop to the separate Hagency Hosting app. Each Desktop data directory (profile or test directory) also produced its own root.

The resource ledger (`~/.hagency-owner-resources`) already exists once per OS user, and only one process can hold it.

On 2026-11-05 an owner signed in to Hosting directly and saw an empty pool. The stray enrollment interrupted the Desktop that was executing the pool. The owner decided that one account on one computer has one identity, executed by one background service, and that Hosting becomes part of Desktop.

## Decision

1. **One root per account per computer.** For each (normalized origin, owner) the OS user has exactly one owner root. It is recorded in `~/.hagency-owner-resources/owner-roots/<sha256(json([origin, owner]))>.json`.
   - The record is private and created with create-new, so the first writer wins. Only explicit removal clears it.
   - Resolution order:
     1. the registered root;
     2. otherwise an existing Desktop root, registered in place;
     3. otherwise `~/.hagency-owner-resources/owners/<same key>`.
   - Roots are never moved, because path-bound state would break.
   - The origin is normalized to its parsed URL form, which trims a trailing `/`, lowercases the scheme and host, and drops a default port.
   - Only Desktop registers a root, because it knows the account's existing roots: its own root first, then a root an older daemon enrolled, then the canonical path. The daemon only looks roots up and refuses with `owner_root_not_registered`, so it can never claim an account first with an empty root.
   - On a computer without Desktop, `daemon enroll --register-root` registers the canonical root explicitly.
2. **One execution path on macOS.** Only the per-user daemon opens the owner root and the ledger.
   - Desktop never runs the owner in-process there; it uses the daemon's `control.sock`.
   - A missing enrollment means "set up hosting", never a local fallback.
   - Windows and Linux keep in-process execution until they ship the daemon.
3. **Enrollment without handover.** `daemon enroll` signs the daemon in with its own SDK device, reusing the root's OAuth client when it has one, and records the registered root.
   - If the root holds an active executor resume record, the daemon proves that device in place, using the existing adoption checks.
   - Otherwise it registers a new executor device.
   - The staged, released and restaged handover states, and `daemon release`, are removed.
   - Sign-out keeps the root and its registration but revokes its executor, so the next enrollment registers a new executor device.
4. **One app.** The daemon runs from the Desktop binary (`rinx daemon …`), which dispatches this before the UI starts, as it already does for the guardian.
   - Desktop installs, pauses, resumes and uninstalls the LaunchAgent.
   - Its Hosting page shows status, a "keep the resource pool running after Desktop quits" switch (on by default), stop and uninstall.
   - The separate Hagency Hosting app, its helper bundle, packaging, smoke tests and release assets are removed.
5. **Locations.** The daemon home stays `~/Library/Application Support/Hagency/owner-daemon` everywhere, including the command line's default.
   - Release builds have no isolated-installation switch. Test-support builds keep their fixture hook.

## Consequences

Good, because:
- one account cannot end up with two pools on one computer;
- "No pooled resources", root conflicts and handovers disappear;
- one app and one execution path remain on macOS;
- the server is unchanged.

Bad, because:
- the daemon still needs its own browser sign-in once per account per computer;
- one OS user can host pools for one owner at a time, because only one process may hold the ledger; a second owner is refused with an explicit error;
- builds before this decision (0.5.0-rc.2 and earlier) are not compatible with it.

## Alternatives Considered

- **Move existing roots to one canonical path.** Rejected: the resume record's scope and the provider credential references are bound to the path.
- **Keep two apps and hand over automatically.** Rejected: it keeps two executors and the handover state machine.
- **Let the daemon use Desktop's Matrix device.** Rejected: two processes cannot share one SDK crypto store.
