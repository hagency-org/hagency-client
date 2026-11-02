# Sparkle release qualification (issue #4)

The product and publishing code is implemented; a genuine Developer ID update
has not yet been run. Do not describe macOS automatic updates as production
qualified until the acceptance matrix below has actual evidence.

## Provisioned inputs

Stable publication requires these effective Actions settings (repository or an
organization scope made available to this repository):

| Type | Name | Meaning |
| --- | --- | --- |
| Variable | MACOS_SIGNED_RELEASE | true |
| Variable | APPLE_SIGNING_IDENTITY | Developer ID Application identity |
| Variable | HAGENCY_SPARKLE_PUBLIC_KEY | Sparkle Ed25519 public key |
| Secret | APPLE_CERTIFICATE | base64 Developer ID Application p12 |
| Secret | APPLE_CERTIFICATE_PASSWORD | p12 export password |
| Secret | APPLE_ID | notarization Apple ID |
| Secret | APPLE_PASSWORD | app-specific notarization password |
| Secret | APPLE_TEAM_ID | expected signing team |
| Secret | HAGENCY_SPARKLE_PRIVATE_KEY | exported Sparkle private seed |

Windows signing settings remain required for the complete six-installer
release. Its updater key format is independent; do not reuse it as a Sparkle key.

The pinned SDK's generate_keys tool supports a named account such as
`hagency-client` and explicit private-key export. Provision it only in the
authorized signing environment. Keep the exported private file outside the
repository with owner-only permissions, upload it as the named Actions secret,
and record protected recovery custody. Never replace a live public key without
an explicit tested rotation plan.

The existing release-secrets.env.example and upload-release-secrets.sh now include
the Sparkle secret/public setting and Apple signing variables. The env file is
ignored; the uploader skips blank fields. This development task did not execute
that uploader or create signing keys.

On2026-10-22 both repository-level secret/variable name inventories and the lists
of organization secrets/variables available to this repository were empty.
The release job currently declares no environment-specific credential scope.
This prevents a genuine signed release qualification with the current effective
configuration. Presence must be checked again after provisioning; names alone
would not establish that a certificate/key/notarization credential is valid.

## Source and artifact gates

1. Cargo stable x.y.z, tag, bundle build/display versions and the ARM appcast agree.
   Starting with 0.6.2, published macOS installers and update feeds are arm64 only.
2. Download/check the fixed Sparkle2.10.0 archive with its pinned SHA256. The
   temporary packaging manifest enables the feature and copies the framework and
   license. Restore both source files on failure or completion.
3. Sign Sparkle helpers/XPC/framework from inside out without app entitlements,
   then sign the app/runtime. Require strict signature verification, app
   notarization/stapling/Gatekeeper, then final DMG notarization/stapling/Gatekeeper.
4. Inspect the app extracted from the exact final read-only DMG. Require the
   configured identity/team/key/version/feed/minimum OS and executable architecture.
   Invoke sign_update with an explicit protected file; missing private input never
   falls back to the user's Keychain.
5. Verify raw Ed25519 DMG and metadata signatures independently with OpenSSL.
   The signed metadata binds actual version/architecture/digest to the exact feed.
   Refuse modified archives/receipts, mutable/foreign URLs, missing or extra files.
6. Require all18 stable assets. Upload to a draft, download back and compare every
   filename/digest before publishing/latest. Refuse an already published release;
   no published installer is overwritten.
7. After publication check anonymous latest redirects, feed Content-Type/cache
   bytes and final immutable ARM DMG bytes. Transient public
   access is retried three times; failure leaves qualification red. This is not a
   rollback mechanism, and no real public-feed result is claimed by local mocks.

CI now defines ARM/Intel builds/lints/tests for the optional native feature using the pinned SDK and
the required test framework rpath link. It also checks ordinary builds without
the SDK. Python tests exercise crypto/tampering/asset/cache contracts offline;
they do not substitute for an Apple-signed installation.

## Actual acceptance still required

Record each result with the exact release/source, OS, executable architecture,
paths, old/new process/version/hash and preserved profile/runtime evidence.
Leave missing hardware and unavailable credentials explicitly unverified.

- Apple Silicon current macOS and macOS12. The local ad-hoc ARM prototype does not
  cover these signed-update qualifications. Intel/Rosetta installer updates are no
  longer part of the release matrix starting with 0.6.2.
- Two genuinely different signed N/N+1 builds; complete runtime/fonts/resources,
  persistent login and drafts, with no unknown Agent execution replay.
- Real authenticated Matrix-account task/approval completion and retirement,
  canceled waiting/download/extraction, repeated quit, state-save failure,
  retained cleanup deadline/failure and installer-helper failure.
- Offline/network errors, insufficient storage, corrupt archive, wrong EdDSA,
  Apple identity, architecture or bundle ID and lower-version rejection.
- User-writable Applications/Home locations, authorized privileged installation,
  canceled authorization, read-only DMG/restricted paths and interrupted recovery.
- Independent standalone assistant: the pinned API still provides no verified
  stop receipt. Starting it blocks installation until a fresh process with it off.
  Never report that refusal as a successful assistant shutdown.
- One manual first installation is necessary for clients that have no Sparkle
  engine. Subsequent upgrades use the in-app operation. No automatic downgrade or
  database rollback is promised after a new-version startup failure.

The prototype/settings/task-retirement evidence is retained separately in
[sparkle-prototype.md](sparkle-prototype.md). Keep issue #4 open until the remaining
qualification and assistant API boundary are resolved.


The Robius0.4.1 packaging hook owns the packaged executable's Frameworks LC_RPATH.
Direct feature builds add it in build.rs for local/prototype use, while packaging
builds leave the hook to add it once. Downloader alone preserves its upstream
entitlements when re-signed; other Sparkle code receives hardened runtime without
the app's audio/location grants, following the
[upstream non-Xcode signing sequence](https://sparkle-project.org/documentation/sandboxing/#code-signing).
