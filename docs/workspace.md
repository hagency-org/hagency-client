# Client workspace

Hagency and the Agent engine share one Cargo workspace and one reviewed
`Cargo.lock`. The desktop package is the default member; use explicit packages
for engine library checks and tests.

| Location | Purpose |
| --- | --- |
| `crates/hagency-app` | Native Makepad application, resources, mini apps, packaging and desktop tools |
| `crates/hagency` and other `crates/hagency-*` packages | Shared local Agent engine libraries |
| `crates/article-*`, `crates/miniapp-*`, `crates/system-apps`, `crates/theme-contract` | Shared editor, mini app and theme libraries and package tool |
| `crates/octosense-app-contract`, `crates/robius-file-picker` | Reviewed local third-party sources with their original notices |
| `tools/native` | Engine verification and development scripts |
| `tests/fixtures` | Shared engine test vectors |
| `dev-vibe/desktop` | Imported desktop collaboration records and release assessment |

## Build and test

Windows builds and caches must stay on D:. From the repository root:

```powershell
./tools/dev.ps1 check --locked -p hagency-app --lib
./tools/dev.ps1 test --locked -p hagency-app --lib
./tools/dev.ps1 test --locked -p article-core -p hagency-app-miniapp-core -p hagency-app-miniapp-catalog -p hagency-app-system-apps
./tools/dev.ps1 run --locked -p hagency-app --bin hagency
```

The wrapper sets `CARGO_HOME`, `CARGO_TARGET_DIR`, `TEMP`, `TMP` and npm's cache
under `D:/BuildCache/hagency-client`, then checks `cargo metadata`. For WSL,
verify `/mnt/d` is mounted from D: before selecting
`/mnt/d/BuildCache/hagency-client` for all these outputs. Docker builds additionally
require a builder whose data is on D: or remote storage; the existing Docker
Desktop disk on C: does not meet that condition.

On macOS and Linux, install the native dependencies listed in the
[Desktop build guide](../crates/hagency-app/docs/local-development.md), then run
`cargo run --locked -p hagency-app --bin hagency`. The workspace macOS runner creates a
development app bundle only for `hagency`; engine tools and test binaries run directly.

Engine library examples use explicit selection:

```sh
cargo check --locked -p hagency --lib
cargo test --locked -p hagency-runtime -p hagency-agent-local
python3 tools/check-workspace-layout.py
```

The desktop invokes `native_owner::NativeOwner` in process. Its loopback OAuth
callback listener remains on an ephemeral port and exposes no management pages
or APIs. The shared code under `crates/hagency/src/console/` remains engine logic;
its name does not imply a running Web console. No Node page build is required.

## App packaging

Run desktop tools and cargo-packager from `crates/hagency-app`.
`tools/package-app.py` discovers the unified Cargo output directory for
resource collection; `tools/package-octos.py` stages its independent pinned runtime
under that same build directory. They do not assume a crate-local `target/`.

Desktop CI, platform builds and installers are in
`.github/workflows/app-{main,builds,release}.yml`. Engine CI continues to select
the engine packages explicitly, so Linux's headless checks do not require Makepad.
Both engine and desktop CI enforce Clippy with `-D warnings`. Desktop, mini app
core, portable editor and theme targets include tests and examples; their Clippy
commands use `--no-deps` to lint the selected packages rather than inherited UI
dependencies. `.github/workflows/typos.yml` checks spelling on pushes and pull
requests, including documentation-only changes, using the pinned version and
repository `typos.toml`. Run `typos --config typos.toml` locally with that version.
The desktop `v*` installer workflow is the sole product release path.
Desktop and the engine workspace are currently 0.6.2.
To publish, run `app-release.yml` on `main` with `publish=true`, or push the
matching `v0.6.2` tag. All four platform jobs must test and build successfully;
only then does the workflow verify six installers and their checksums and publish
the complete GitHub Release as Latest. Manual runs with `publish=false` only upload
CI artifacts. Published releases cannot be overwritten by a rerun.
Starting with 0.6.2, macOS installers are Apple Silicon arm64 only. Linux retains
amd64/arm64 DEB and AppImage packages, and Windows retains the amd64 installer.
The outstanding dependency assessment remains in
[`dev-vibe/desktop/release-0.5.0`](../dev-vibe/desktop/release-0.5.0/assessment.zh.md).

## Source history and downloads

Desktop history through `52d88b2020a322d9c1883b412e74968c1a60ce01` was imported
with its original commits as a merge parent. Subsequent path moves live in the
same history. Original Apache/MIT and third-party notices remain with their code.

Historical installers remain available from
[the original Desktop releases](https://github.com/hagency-org/hagency-desktop/releases).
Current installers are published to
[Hagency Client Releases](https://github.com/hagency-org/hagency-client/releases/latest).
New builds use `hagency-{platform}-{arch}` installer names. The existing 0.6.0
release retains its published `hagency-desktop-*` assets and checksums; renaming
the source does not rebuild or relabel those binaries. Update Cloud's stable
download URLs when a release with the new names is published.

The frontend lives in `crates/hagency-app` (Rust package `hagency-app`, library
`hagency_app`, executable `hagency`). The engine package remains `hagency`, used as a library. Existing app storage
paths, published bundle identifiers, legacy URL schemes and mini-app protocol IDs
remain compatible. Use `HAGENCY_APP_DATA_DIR` for isolated app runs; older data-dir
variables remain aliases.

The former CLI Web console, `mockup/` prototype, static bundle build, user service
installer, CLI artifact release and Pages workflow have been removed. Existing
external user-service installations are not changed by source cleanup; stop and
remove an old user service with its OS supervisor before deleting its old binary.
