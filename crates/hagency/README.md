# Desktop owner engine

This crate is a library dependency of `hagency-app`. The desktop constructs
`native_owner::NativeOwner::open_with_matrix` and invokes typed commands in
process. Login, account isolation, Agent execution, budgets, approvals, recovery,
provider handling and speech remain shared engine functionality.

The only actual HTTP listener is the loopback OAuth callback on an ephemeral
port. It accepts `/console/server-login/callback`; it does not serve console
pages or management APIs. Internal request routing preserves the engine's
existing authentication and policy checks without exposing them over a socket.

There is no product CLI, static Web bundle, headless user service installer or
CLI release workflow. Explicit probe binaries under `tests/fixtures` are test
harnesses, not desktop product entry points.

See [the desktop guide](../hagency-app/README.md) and [workspace guide](../../docs/workspace.md).

Native Agent and standalone server-login persistence use the same explicit OS
credential backend as the desktop. Linux therefore requires a working Secret
Service and user D-Bus session; a headless/container environment without them
fails explicitly. These library paths do not silently select files or Mock as
an alternative backend. Real Linux service/container credential behavior remains
unqualified in this remediation; automated fixtures use explicit isolated fake
stores. See [credential storage and recovery](../hagency-app/docs/privacy.md).
