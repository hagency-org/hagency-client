spec: task
name: "Programming local policy fence after Server authorization"
tags: [active, rust, desktop]
---

An exact native tool approval is valid only under its original current local
policy revisions and expiry. The final awaited Server execution proof can
interleave with an owner policy change; it cannot preserve an obsolete local
grant. Recheck the original proposal immediately after that proof and before
sending an acceptance. Do not consume the approval twice or mint replacement
custody. This applies to commands, file changes and configured MCP calls under
both owner decisions and exact allow rules, across all three policy layers.

- A concurrent policy revocation committed through another real SQLite handle
  during the final host check declines the original native tool callback.
  Test: programming_policy_revoked_during_final_server_check_never_accepts_native_tools
