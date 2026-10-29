# Local mdurl 0.3.1 dependency repair

This is a locally patched copy, not a new upstream release. The authoritative
application patch lives in the root `Cargo.toml`; this package remains version
0.3.1 with its original MIT license and all attribution intact.

Origin:

- Registry package: <https://crates.io/crates/mdurl/0.3.1>
- Original `.crate` SHA-256: `5736ba45bbac8f7ccc99a897f88ce85e508a18baec973a040f2514e6cdbff0d2`
- Upstream: <https://github.com/rlidwka/mdurl.rs>
- Exact upstream commit: `0a7536e3ffc7f5688d3b06ab647ebcfbc511d1cb`
- `UPSTREAM-SHA256.json` records every original archive file before patching.
  `.cargo_vcs_info.json` and `Cargo.toml.orig` remain original provenance.

The normalized `Cargo.toml` IDNA requirement changes from `>= 0.1.0, < 0.4`
to `=1.1.0`. Two percent-function return types add the explicit anonymous
lifetime `Cow<'_, str>` in encode.rs and decode.rs; this
is the same inferred lifetime and changes no behavior. The local path dependency
must compile under the native build's existing `RUSTFLAGS=-D warnings`, which
otherwise rejects those original lifetime-syntax warnings. No lint is suppressed.
All other original source, parser, README and all 133 existing tests are unchanged.
Added integration tests exercise the formatting APIs with the newer dependency.

Both `idna::domain_to_ascii` and `idna::domain_to_unicode` retain their signatures
in 1.1.0. Invalid ASCII-only or empty ACE labels intentionally follow the fixed
UTS46 behavior instead of collapsing into an ordinary ASCII hostname. Human
formatting is display-only; a failed Unicode conversion is not network authority.

The application calls only `mdurl::urlencode::encode` / `decode`. Its separate
raw punycode policy and retained JavaScript oracle are unchanged. This dependency
repair does not establish a new IDN display policy or qualify other independent
lockfiles, including Octos.

Verification commands from the application root:

```sh
cargo test --locked -p mdurl
cargo test --locked -p hagency-matrix-format native_matrix_format_
cargo tree --locked -i idna@1.1.0
cargo audit --json
```

The package is explicitly excluded from workspace membership so vendoring does
not add upstream style lints to the application's own strict-lint scope. Its
upstream tests still run explicitly; no legacy assertion is removed or relaxed.
