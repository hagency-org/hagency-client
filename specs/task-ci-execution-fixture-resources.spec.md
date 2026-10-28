spec: task
name: "Give execution process fixtures independent CI host resources"
tags: [active, rust, fixture, ci]
---

Original Linux job122431362644 at8e0e50fd failed four execution fixtures:
owner-verdict OutcomeUnknown, an early31s control lease renewal timeout with
last_unknown_dequeued=true, a closed usage-control notice stream, and a late
midwrite marker. The HTTP tests passed. Local42-case library runs at16 threads
and one thread both pass; this does not identify an unobserved historic OS phase
or establish the cause of the two notification/marker failures.

The CI repair removes independent execution fixtures' concurrent host workload
as a variable. Run all execution targets in a separate one-thread stage; remove
only its package selector from the later workspace invocation so every ordinary
case still runs exactly once. Preserve the original ADR140/144 qualification
exclusions, spec-binding inventory, all other packages' parallelism, in-case
concurrency, real child/database behavior, all production bounds and assertions.
Keep the original failed stage fatal even if later diagnostic coverage passes.

Failure-only owner-verdict and usage-notice text must retain this attempt's
unknown/dequeued and original operation stop/cleanup evidence. No retry, new
skip, allowed error, manufactured receipt or widened deadline is introduced.

Allowed changes: the two CI workflows, approval_loss test diagnostics, this
contract, the publication/owned fixture contract and task documentation.

- The original quiet real child runs at least31s and completes successfully.
  Test: native_owned_turn_long_lifetime
- A pending begin receipt cannot conceal the actual retained usage failure.
  Test: native_owned_approval_usage_successful_control
- A terminal turn keeps an original untransmitted approval response unsent.
  Test: native_owned_approval_turn_end_untransmitted
- A terminal turn preserves exact transmitted/uncertain custody by platform.
  Test: native_owned_approval_turn_end_midwrite_uncertain
