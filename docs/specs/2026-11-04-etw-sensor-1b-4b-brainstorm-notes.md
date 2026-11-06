# Plan 1b-4b — Blinding Detection: Brainstorm Notes (handoff)

**Status:** in brainstorm (started 2026-11-04). Sub-project 1, spec `2026-10-01-etw-sensor-design.md` §9. The contract from earlier plans: the "Interfaces for later plans" sections of plans 1b-3c, 1b-4a, 1b-4a-p2 and 1b-4a-p3. Earlier scope: [1b-4 notes](2026-10-12-etw-sensor-1b-4-brainstorm-notes.md), S1.

## Decisions

| # | Question | Decision |
|---|---|---|
| B1 | One plan or two | **A (2026-11-04): two plans.** **1b-4b-1:** the watchdog (session identity, `ProcessTrace` returning, provider state), session restart, Event Log Activity records. **1b-4b-2:** the three canaries and the Kernel-EventTracing decision. Restart replaces a live consumer while events flow, the riskiest piece, so it gets its own review; §14's blinding test needs only 1b-4b-1; the canaries touch other code (pipeline matching, a `canary` subcommand, a registry key). Rejected: (B) one plan, likely larger than 1b-4a's 6,300 lines, with three parts that barely overlap; (C) canaries first: they would report on sessions the agent cannot yet repair. |
| B2 | Where F11 goes | **A (2026-11-04): a small plan of its own, 1b-4a-p4, before 1b-4b-1:** measure with the synthetic load first, then fix what the figures show. The blinding tests join `agent-live`, the job that flakes, so a failure on a restart PR could not otherwise be told from the flake; and if load can strip a Launch of its hash, an attacker can cause that on purpose. Rejected: (B) the first task of 1b-4b-1 (mixes an unrelated fix into the plan meant to focus on restart); (C) leave it to 1b-4c and rerun when it flakes (rerun until green, across two plans). |

## Facts gathered

- **F11's run was a burst:** `agent-live`'s sensor events delivered in the test of about 7 s, with enrichment misses: 13k–23k in six sampled runs (0–2 misses), 29k (32), 47k (2), and 228k (79, the failure, about 32,000 events/s). The link is loose: one failure, and the 29k and 47k runs do not line up. It points at load, not at plan 1b-4a-p3.
- **What the Launch lane does not cover** (read from `win/services.rs`, not measured): a queued Launch image still waits for a free worker; there are 2, and each may be in a slow file; both run below normal priority, so a saturated machine starves them. The deadline is 1 s (`enrich_deadline`). Which of these uses it up is not known.
- **No way to add a late hash:** a late result is cached for the next event on the file; the Launch already emitted stays without it.

## Carried in

- **F11 (spec §15.3):** under load at start-up a launch can go out without its hash and signature, which fails `agent-live` now and then. Goes to plan 1b-4a-p4 (B2).
- **H2 (plan 1b-4a-p3, Interfaces):** if the start-up pass never replies, `startup_pending` stays true for the whole run and H2 never applies; Sensor Health should see it, and a deadline may end the pass.
- **Session A's stalls (plan 1b-4a-p4, P13, C3):** with every CPU busy, delivery stalled for about the 750 ms hold three times in 8 host runs, on both priorities, dropping 21 and 11 Launch halves (`actor_dropped`) in 1,800 launches each: probably as many Launches without their command line and user. Not explained, and not tied to the workers' priority or cleared of it. A stall during the binary test would fail `every_report_measures_zero_loss`. The watchdog looks at delivery, so 1b-4b-1's brainstorm takes this up; the ETW consumer's priority is one thing to try.
- **A restarted session's counters start from 0** (plan 1b-4a, Interfaces): reset the health base, or the 32-bit wrap reports about 4.29e9 lost.
- **A spare `FastRead`** (plan 1b-3c, Interfaces): made before the driver takes the services.

## Next steps

1. Plan 1b-4a-p4: brainstorm and host run done ([p4 notes](2026-11-04-etw-sensor-1b-4a-p4-brainstorm-notes.md): C1, C2, C3); [plan](../plans/2026-11-05-etw-sensor-1b-4a-p4-plan.md) built (2026-11-05).
2. Brainstorm 1b-4b-1.
