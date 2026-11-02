# GitHub CI and desktop installers

The repository inherited Rinx workflows. Before this repair, main CI failed to
find `../hagency-client/crates/hagency`, and the inherited release required a
RINX_RELEASE token and Apple signing configuration. There were no Hagency
GitHub Releases at the 2026-10-10 audit.

## Source dependency

Desktop and the owner engine now live in the hagency-client workspace.
The `hagency` dependency resolves to `../hagency`; the repository root Cargo.lock
is the single dependency lock. Engine and UI changes are validated together.
Keep `--locked` in verification and packaging. Current workflows live at the
repository root as `app-main.yml`, `app-builds.yml` and `app-release.yml`.

## Checks and artifacts

`app-main.yml` retains the application/unit/portable crate and resource checks.
`app-builds.yml` checks native Linux amd64/arm64, macOS Intel/Apple Silicon, and
Windows amd64 on main/PR. Mobile compilation remains manual-only. Build summary
output is informational; protect main using the actual build/test jobs.

`app-release.yml` runs on `v*` tags or manual dispatch using Rust 1.98.0 and pinned
cargo-packager 0.10.1 / robius-packaging-commands 0.4.1. Each platform runs library
tests, then packages the app with its existing resource and Octos runtime hooks:

| Platform | Architecture | Artifact |
|---|---|---|
| Linux (Ubuntu 22.04 baseline) | amd64, arm64 | DEB and AppImage |
| macOS | Apple Silicon arm64 | DMG |
| Windows | amd64 | NSIS EXE installer |

Manual runs upload Actions artifacts only by default (14-day retention).
An explicit `publish=true` dispatch builds and verifies every installer first,
then creates the version tag at that exact source SHA and publishes the release.
This uses GITHUB_TOKEN and does not trigger a duplicate tag build. Tagged runs require
the tag to match the package version, and publish a GitHub Release
only when all four platform jobs pass. Six installers are verified against
per-platform checksums and a combined SHA256SUMS.txt. A rerun refuses to overwrite
an already published release. Assets are uploaded to a draft first; the workflow
then publishes it after the complete upload succeeds.
The standard GITHUB_TOKEN suffices; RINX_RELEASE is no longer required.
Starting with 0.6.2, Intel macOS installers and their update feeds are no longer
produced. Native Intel compilation and application tests remain separate CI checks.

## Signing

Default macOS/Windows packages are unsigned development builds; release notes
state this. To enable macOS Developer ID signing, notarization and stapling:

- Repository variable `MACOS_SIGNED_RELEASE`: `true`.
- Repository variable `APPLE_SIGNING_IDENTITY`: your Developer ID Application identity.
- Secrets: `APPLE_CERTIFICATE` (base64 P12), `APPLE_CERTIFICATE_PASSWORD`,
  `APPLE_ID`, `APPLE_PASSWORD` (app-specific password), `APPLE_TEAM_ID`.

The signing script derives bundle paths from `product_name` instead of the
inherited Rinx.app name and accepts the identity from the environment. The
workflow stamps the bundle identifier/display name/version. Failed signing is a
failed package job, with no unsigned fallback or partial release.
Windows code signing, Windows ARM64 installers, automatic updater metadata,
install-and-launch testing on each OS, and mobile store publishing remain future
work. They are not established by compilation or packaging alone.

The inherited Rinx AUR workflow is manual-only; publishing a Hagency release
must not publish the unrelated Rinx AUR package. The inherited license refresh
workflow remains manual and retains its explicit push-token requirement.
Dependabot proposes monthly GitHub Actions updates without auto-merge.

## Native CI repair (2026-10-12)

Strict native builds exposed unused feature/test-only Desktop helpers and two
Client values used only by Unix host-file callbacks. The latter are fixed in
the pinned Client revision above. Keep `-D warnings` enabled so platform-specific
mistakes continue to fail the build. Matrix ID examples and empty text resets
are literal values, while the four missing UI messages now have catalog entries.

The repair passed local strict library compilation, 450 Desktop library tests
(2 ignored), 196 Client runtime tests (5 ignored), and 70 agent-local tests
(1 ignored). The [native GitHub CI run](https://github.com/hagency-org/hagency-desktop/actions/runs/38201853621)
also passed its build, test, translation and theme checks. The final cross-platform
results are tracked on [PR #1](https://github.com/hagency-org/hagency-desktop/pull/1/checks).
Installer/AUR publishing and mobile build workflows were reviewed but not executed.

## Verification checkpoint (2026-10-10)

New/modified workflows passed actionlint syntax validation; new release/main and
server workflow scripts also passed actionlint's ShellCheck integration. The AUR
workflow has an existing nonblocking ShellCheck style finding (SC2129). macOS
signing script passed `bash -n` and ShellCheck error-level validation. No actual
GitHub runner build, signed package, installer launch or release upload is claimed.
The previously unpublished client prerequisite is resolved by the published pin
above. Real GitHub runner and installer verification still remain pending.

Publication validation: Desktop library tests passed locally (447 passed,
2 ignored). Client native binary compilation and Agent-local/runtime library
unit tests passed before publishing the pinned client source. Ignored tests
are not counted as passing. Full platform builds remain GitHub CI results.

## Windows checkout integrity (2026-10-13)

The first tagged installer run found Windows CRLF conversion changed embedded
contract SHA-256 fixtures and the example miniapp bundle digests. Repository
`.gitattributes` now keeps text files at LF on every checkout, preserving the
original producer hashes without disabling verification. A fresh checkout with
`core.autocrlf=true` verifies all 20 contract hashes and keeps every example
miniapp file byte-identical. The failed v1.1.0 tag is retained; the repair ships
in v1.1.1. The final v1.1.2 candidate also installs `libva2` and
`libva-drm2`, required by the packaging dependency scanner for Makepad video.
Failure caches are retained so packaging repairs can reuse compiled dependencies.

The second Windows gate passed445tests but found two nested-bundle failures:
the vendored contract hashed Windows path separators instead of the producer
encoding. `bundle::digest_dir` now joins components with `/`; original manifests
and the Unix digest remain unchanged. A pinned nested-bundle hash is checked on
every platform before the Desktop library tests. v1.1.2 is created only after
the complete installer gate passes.

The Linux ARM64 DEB and pinned Octos runtime built successfully before the
AppImage script failed. cargo-packager0.10.1 retains a relative intermediate
path when it creates a missing output directory, then changes its working
directory before executing that relative script path. Installer commands now
pass an absolute `--out-dir` (including signed macOS) and enable verbose script
logs. The output remains the same dist directory.

## 0.5.0 readiness (2026-10-16)

The packager now uses the generated `resources/icon.icns` explicitly. The new
`tools/check-release-icons.py` preflight catches missing/invalid icon inputs
before expensive builds. This avoids cargo-packager 0.10.1 trying to convert a
plain 1024px PNG into an unsupported 1x ICNS representation.

Compatible transport security updates pass the main CI and all five desktop
builds. Formal 0.5.0 publishing is on hold: the Desktop lockfile still has five
vulnerability matches, and the independently built pinned Octos workspace lock
has 36. These counts are not proof of reachability for every advisory. The
[release assessment](../../../dev-vibe/desktop/release-0.5.0/assessment.zh.md) records the actual
PDF/XML paths, feature boundaries, lock hashes and remaining upstream work.
Neither cancelled installer run published a v0.5.0 tag or Release.
