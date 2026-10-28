# Native initial management enrollment proof

This prerequisite creates a native-owned enrollment identity. It does not enable
resource adoption, a mini-app capability, native confirmation UI, grant
publication or provider execution. The canonical owner resource must already
exist under an explicit native owner grant; legacy profile resources are not
silently treated as owner-registry resources.

The server has no initial enrollment row or enrollment credential. Its existing
`GET /hagency/api/v1/management/contributions/{id}` supplies an authenticated
admitted contribution and its approved admission request. The existing allocation
transport recovers enrollment IDs only from original native grant receipts. It
cannot bootstrap a new ID. No server route/schema change is required here.

`OwnerGrantIntents::verified_enrollment(registry, contribution_id)` accepts only a
contribution selector. It requires native `ConsentGrants` and `ReadResources`,
plus `ReadProviders`/`ConnectProviders` for the existing trusted provider identity
observation. Production invokes `NativeManagement::verified_provider_account`;
raw provider subjects in fixtures exist only under `cfg(test)`. Codex currently
provides the required stable quota account identity. Unsupported runtimes fail
closed. Neither a login path nor a coordinator username is provider identity.

The factory verifies current SDK profile and execution device, the admitted
contribution's exact owner/device/generation/resource/revision/coordinator, and
the approved request linkage. It obtains the actual provider account observation
and checks the canonical owner's active resource access, resource runtime,
resource revision, physical seat mapping and provider-account digest. A second
central read after provider observation prevents using a contribution changed
during that work. Optional connection-verification UI status is descriptive and
does not create native authority.

A private `management-enrollments-v1.json` journal binds a once-generated random
ID to the full original profile/client/installation identity, registry, resource,
provider/resource commitment, contribution, coordinator and execution generation.
It holds at most 128 entries and 256 KiB. A first-use identity marker detects an
ordinary lost journal. If both files disappear, canonical grant history and the
separate original grant-intent journal prevent replacement issuance. Unknown or
changed original custody fails closed. Existing exact records reuse the original
ID; they do not silently amend or renew it. Journal access uses the validated
profile directory without opening or initializing personal-agent SQLite.

The returned `VerifiedManagementEnrollment` is not deserializable and contains a
restriction-only `VerifiedOwnerResourceBinding`. The grant transaction rechecks
that binding against canonical provider/seat/resource/access state before any
new hold. Existing immutable receipt recovery remains a separate historical
operation. This finite observation is not a central execution lease: publication
and later deliveries still require current central authority, and provider
execution requires its own current identity/custody checks.

Every asynchronous SDK check finishes before the factory constructs and attaches
the final provider-bound proof. The provider deadline can be shorter than the SDK
session. A deterministic test delays the final SDK response across that deadline:
the original implementation returned an expired proof; the corrected implementation
rejects it while retaining the original journal ID and creating no hold. Red and
green logs are `.run/owner-enrollment-final-expiry-{red,green}.log`.

Focused tests use an actual loopback HTTP transport and isolated owner-registry
SQLite with synthetic SDK/provider observations. They are not live provider or
deployed server E2E validation. No initial enrollment ID, raw receipt, provider
subject or arbitrary resource-binding input is exposed to Splash.


## Original admission identity across observation and restart

New private enrollment bindings record `admissionRequestId`. The before/after
provider reads compare that request ID, the contribution revision and the request
revision as well as the original resource/account/device fields. A subsequent
factory call, including after NativeOwner restart, must match the same stored
request. A later approved request for identical resource settings cannot reuse
an earlier enrollment or its pending grant review.

Previously written bindings have no request-ID field. The optional field is
omitted during serialization, preserving their original digest and bytes. They
may continue only at the first approval: contribution revision2 and request
revision2, with all previous binding checks still matching. Creation starts at
revision1 and the server's first approval increments it to2; withdrawal and
re-admission never reset these revisions. Later legacy states return
`management_enrollment_history_required`. The factory does not rewrite an old
journal, guess a later request or mint a replacement enrollment.

This change preserves immutable grant receipts and the existing one-grant/history
guards. It does not implement new-round consent, closure of old pending reviews,
resource amendment or renewal. Those require their own explicit native protocol.

Candidate evidence is in Desktop `.run/management46-stage/native-admission-v5`:
13 enrollment and20 grant/recovery tests plus strict default/enabled library
checks pass. The tests use actual loopback HTTP and isolated SQLite, synthetic
provider/SDK observations and NativeOwner reopen; they do not demonstrate a full
OS-process/UI/provider end-to-end flow. The two source files were promoted to the
Client working tree, with main-source verification recorded separately in
Desktop `.run/management46-native-admission-promotion/main-v1`.


## 2026-10-19: Original-admission grant confirmation and cancellation

New native grant intents persist the original admission request identity without
rewriting legacy digests. Prepare, re-review and confirmation check the current
authenticated admission; a cached confirmation cannot reserve capacity after its
request or observed revisions change. Re-review may refresh revisions of the same
admission while keeping the original command, grant and intent IDs. Legacy
unbound journals remain limited to the initial revision2 approval.

The native cancellation API separately reviews and closes only an unattempted
intent. It works after withdrawal, owner reopen and accounting-period expiry
without provider login. Closing its review has no effect. Applying/unknown and
recorded grants retain their original recovery path and receipts; cancellation
cannot refund a hold or reinterpret an unknown outcome. Desktop cancellation
controls are not yet integrated.

The stale-confirmation regression failed before the fix. The final candidate
and promoted main source each pass the same62 grant/enrollment/publication tests
and default/enabled strict Clippy checks; these counts overlap, not124 distinct
cases. Earlier runs retained one test-child stack overflow. The connection
fixture now uses a separate executor task as Desktop does, keeping the ordinary
stack limit and all assertions. These checks use synthetic provider/Matrix
authentication, actual loopback HTTP and SQLite, NativeOwner reopen, and existing
subprocess cases; they are not real-provider OAuth or full mini-app E2E.

Evidence: Desktop `.run/management46-stage/native-consent-v3/qualification.json`
and `.run/management46-consent-promotion/main-v1/qualification.json`; selected
source hashes/backups are in that promotion directory. Running desktop45 and
server32 were not replaced. Explicit new-round receipts, local old-grant fencing,
resource amendment and the mini-app workflow remain unfinished. In particular,
a current resource hash must not stand in for an original admission identity
when publishing a previously unpublished v1 receipt after re-admission.
