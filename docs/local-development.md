# Local desktop and server development

Keep Client and Server as sibling checkouts. The desktop is in `crates/hagency-app`;
see [workspace builds](workspace.md) and [native dependencies](../crates/hagency-app/docs/local-development.md).
On Windows use `./tools/dev.ps1`; all outputs, caches and temporary files stay on D:.

Configure the server with [its README](../../hagency-server/README.md). Enter its
origin in the desktop login page and complete Pasion authorization. The desktop
receives the OAuth redirect on an ephemeral loopback callback; no CLI management
server or page build is needed. OAuth integration tests run with
`./tools/dev.ps1 test --locked -p hagency-app --lib login::oauth_tests`.
# One-command local environment (macOS)

Keep `hagency-client` and `hagency-server` next to each other. Install Docker Desktop,
Python 3, Rust 1.99.0, Xcode command-line tools, CMake and libpq. From Client run:

```sh
./tools/local-dev.sh start --pull
./tools/local-dev.sh doctor
```

The launcher fast-forwards both `main` branches, builds the complete Server,
its two browser applications and the native Client from source, then checks
provenance, artifact hashes and the actual HTTP resources. Source changes trigger
complete coherent bundles; loose resources and assets copied from older images
are never reused. Build failure leaves the previous running version untouched.

Use `http://127.0.0.1:8088` in Desktop, or visit `/login` in a browser. Local
registration is enabled; contact fields are optional. The initial `admin` password
is in Server `.run/local-dev/admin-password`. `stop`, `restart`, `status`, `logs`,
and `update` are available. `start` without `--pull` uses the current checkout.
Data survives stop/restart. See the [full Chinese guide](local-development.zh-CN.md)
for paths, isolation and failure handling.
