# Sub-project 1, plan 1b-4a-p4 — A Launch Without Its Hash: Brainstorm Notes

**Status:** brainstorm done (2026-11-04): C1 and C2 decided; the spec change and the plan are next. Comes before plan 1b-4b-1 ([1b-4b notes](2026-11-04-etw-sensor-1b-4b-brainstorm-notes.md), B2). The question: why a Launch can go out without its hash and signature under load (spec §15.3, plan 1b-4a-p3's F11), which fails `agent-live` now and then.

## Decisions

| # | Question | Decision |
|---|---|---|
| C1 | The hash workers | **A (2026-11-04): both hash workers run at normal priority.** The live test's Launch lost its hash in 0 of 56 runs, against 13 of 24 at below normal (P2), and module hashes keep up (P3). Rejected: (B) a dedicated Launch worker with modules below normal (2 of 32, and modules stop getting hashes on a busy machine, which an attacker could cause on purpose); (C) keep below normal and raise the 1 s deadline (jobs waited 10 s and more, and R4 rejected longer deadlines: they hold back every later event). |
| C2 | The seeder and the deadline clock | **A (2026-11-04): the seeder and its namer thread run at normal priority too; the deadline clock stays as it is and goes into the spec's Known Limitations.** The seeded check failed in 10 of 16 runs at below normal and 0 of 16 at normal, and the start-up pass fell from 12–30 s to under 1 s (P8); with on-miss seeding off that pass is nearly all the seeder does, once, at start-up (corrected 2026-11-05: this row first said the CPU budget bounds it, but §7.4's budget does not charge the start-up pass). Fixing the clock means reworking how the pipeline's main loop sends requests, to save about 2 Launches in 3,000 under overload on a release build (P9). Rejected: (B) fix the clock now as well (a much larger plan, for a benefit too small to measure); (C) hash workers only (`agent-live` would keep a known starvation failure). |

## How it was measured

A probe on `scratch/p4-probe` (never merged): each hash job stamped at request, submit, pickup, finish and reply, with the worker's thread cycles. `agent-live`'s live test ran on GitHub's `windows-latest` runner (4 vCPUs, a cold machine, elevated) under a load generator, on debug and release builds. The figure that counts is the live test's own `cmd.exe` Launch: did it keep its hash? Runs 41967245364, 41969285094 and 41970601780.

## Facts

- **P1, two causes, seen under heavy overload** (run 1: about 2 million events in the test, debug build):
  - **Starvation:** at below-normal priority a hash took about 4,700 ms of wall time for about 19 ms of CPU, and jobs waited a median of 10–14 s. At normal priority the median wait was under 20 ms.
  - **The deadline clock:** at normal priority, 1,905 of 1,980 late Launches were on the pipeline's side. `tick` reads the clock once, processes everything due, and submits requests after, so a Launch processed late in a long tick has used up part of its 1 s before its request exists. A tick over 1 s leaves it none.
- **P2, the live test's Launch at a heavier load than the real failure** (runs 2 and 3; 530k–1.75M events delivered against the failure's 228k; hash missing in):

| Workers | Light load | Light load + all CPUs busy | Total |
|---|---|---|---|
| Below normal (today) | 2 of 12 | 11 of 12 | **13 of 24** |
| All normal priority | 0 of 28 | 0 of 28 | **0 of 56** |
| Normal, plus a cap of 2,000 events a tick | 0 of 12 | 1 of 12 | 1 of 24 |
| A dedicated normal-priority Launch worker, modules below normal | 1 of 16 | 1 of 16 | 2 of 32 |

- **P3, what each option does to everything else** (run 3, all CPUs busy):
  - Launch hashes over 1 s: all normal 3 of 248; Launch worker 8 of 254. With light load, 0 of 793 and 7 of 867.
  - Module hashes: all normal finished 5,751 in the test, median 9 ms; the Launch worker's setup finished 288, median 13.7 s, so modules stop getting hashes on a busy machine.
- **P4, the tick cap showed no benefit at these loads:** on release builds no pass took more than 50 ms in the light-load runs. The deadline clock is a real flaw but it needs overload; 1–3 Launches in about 400 per cell still miss on the pipeline's side.
- **P5, the spec gives no measured reason for below normal:** §6.3 says only that work runs on below-normal workers, never on an ETW or pipeline thread.
- **P6, the seeder starves the same way:** it is below normal too, and with all CPUs busy `seeded_file_handle_update` failed in 22 of 32 runs (0 of 32 with light load): the start-up pass made 3 of its 4 table reads within the test's 30 s wait. This is the case plan 1b-4a-p3 carried as H2.
- **P7, `enrichment_misses` also counts what is pending at stop:** a quiet run reported 61 with no late reply in its log; two `wsl.exe` launches came at the very end. Miss counts in the live test's notes overstate real misses. The same goes for "no hash result by end of run" in the probe's figures: most are launches in flight at the end.
- **P8, the seeder at normal priority** (run 41976765339, hash workers normal, all CPUs busy, 16 runs each, debug and release): `seeded_file_handle_update` failed in 10 of 16 at below normal and 0 of 16 at normal. The start-up pass took 12–30 s at below normal (eight runs reached the test's 30 s wait) and 0.26–0.75 s at normal. Both the seeder and its namer thread were raised. What bounds them is that the pass runs once and is short: the CPU budget (`seeder_cpu`) charges re-reads, not the start-up pass (§7.4), and with on-miss seeding off re-reads come only while the pass is pending. This does not settle H2's other case, a table read that fails.
- **P9, the deadline clock on a release build** (the same run, 8 runs each, overload of about 1.2M events on release and 2.5M on debug): release: 2 of 3,109 Launches missed on the pipeline's side and the live test's Launch kept its hash in 8 of 8. Debug: 1,639 of 4,363, the hash missing in 6 of 8, and most other checks failing too, because the debug pipeline cannot keep up. So the flaw is real but, at these loads, almost entirely a debug-build effect.
- **P10, queue drops did not get worse at normal priority** (2026-11-05, from the same runs' reports): `no_queue_drops` failed in 2 of 24 runs at below normal and 0 of 56 at normal. No thread of the agent runs above normal, so at normal the hash workers share the CPU with the ETW consumers and the pipeline as equals. The live test's report does not carry the ETW sessions' own loss, so these runs say nothing about it.
- **P11, the check can fail** (2026-11-05, run 42078592869): `the_workers_run_at_normal_priority` reads each thread's priority from the running agent. Putting the call back on the hash workers, the seeder or the namer failed that check and no other of the 52; the thread showed -1 (below normal), and 0 without a revert.
- **P12, `agent-live`'s tests watched at normal priority, no load added** (2026-11-05, run 42079443395, `scratch/p4`, 8 fresh VMs, 2 passes each): the live test passed 16 of 16 and the binary test 16 of 16, every check. One pass happened to reach the real failure's scale, 257,826 events delivered against its 228k: 3 misses, and its Launch kept its hash. That is a single run. Misses were 0 in 13 passes, 3 in two, and 63 in one, a light pass of 14,476 events whose Launch also kept its hash. The 63 are not explained: the reports hold no timing, they do not follow `late_arrivals` (a pass with 310 had none), and being in flight at stop (P7) is possible but not shown. So misses remain at normal priority; what these runs show is that the live test's Launch stopped losing its hash.

## What is not shown

- The real failure was not reproduced: the probe's loads are about 2–8 times its 228k events, and no timing exists from that run. The same signature at a load 2–3 times higher makes starvation the likely cause, not a proven one.
- 0 of 56 is not never. The 1 s deadline stays.
- What normal priority costs other programs was not measured. On average the hash workers are a small load (the whole agent used 0.97–1.59% of one core over the two hour runs), but a cold burst can take two threads at equal priority.
- Whether normal priority costs ETW session loss was not measured either (P10).

## Next steps

1. Verify-first on `scratch/p4` (from `main`): Task 1 (C1) and Task 2 (C2) done, CI green, three reverts caught (P11), `agent-live`'s tests watched over 16 passes (P12).
2. The host run, `spikes/run-p4-cost.ps1`: what normal priority costs other programs, and whether it costs ETW loss. The two builds, each with every CPU busy and cold hashes.
3. The spec: §3.2, §6.3 and §7.4 (priorities), §13 (the host run), §15.3 (P1–P11 and the host run), §16 (the deadline clock), §17 (clarifications).
4. The plan, review, approval, build. Then plan 1b-4b-1.
