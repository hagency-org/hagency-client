[English](README.md) | [中文](README.zh-CN.md)

# Hagency Client — archived

Hagency Client is now maintained as part of [Hagency Desktop](https://github.com/hagency-org/hagency-desktop). This repository is retained for historical source, releases and discussion; it no longer accepts development work.

- [Download Hagency Desktop](https://github.com/hagency-org/hagency-desktop/releases)
- [Report issues and request features](https://github.com/hagency-org/hagency-desktop/issues)
- [Contribute to the native client libraries](https://github.com/hagency-org/hagency-desktop/tree/main/crates/hagency-client)
- [Desktop build and development instructions](https://github.com/hagency-org/hagency-desktop#readme)

Desktop owns the 18-crate native library workspace, daemon integration, tests and release pipeline. The import was merged in [Desktop PR #11](https://github.com/hagency-org/hagency-desktop/pull/11); the old standalone UI is retired. New fixes belong in Desktop rather than this repository.

The remaining open issues were transferred with their discussion history:

| Former Client issue | Desktop issue |
| --- | --- |
| #4 — macOS updates | [#44](https://github.com/hagency-org/hagency-desktop/issues/44) |
| #23 — button text and icon alignment | [#45](https://github.com/hagency-org/hagency-desktop/issues/45) |
| #24 — room encryption options | [#46](https://github.com/hagency-org/hagency-desktop/issues/46) |
| #25 — Matrix verification and recovery | [#47](https://github.com/hagency-org/hagency-desktop/issues/47) |
| #29 — mobile build gaps | [#48](https://github.com/hagency-org/hagency-desktop/issues/48) |

Client [PR #52](https://github.com/hagency-org/hagency-client/pull/52), recovery of partially created owner-private rooms, is carried forward in [Desktop PR #49](https://github.com/hagency-org/hagency-desktop/pull/49).

A previously uncommitted, test-only change is preserved on [`archive/startbug-test-corrections-20261110`](https://github.com/hagency-org/hagency-client/tree/archive/startbug-test-corrections-20261110). It is a historical patch, not a tested Desktop release change.

Historical releases remain available for reference. Use Desktop for current downloads and support. The original license and notices remain in this repository.
