# Sub-project 1, plan 1b-4a-p4 — A Launch Without Its Hash: Brainstorm Notes

**Status:** brainstorm done (2026-11-04): C1 and C2 decided; the host run and C3 done (2026-11-05); the plan is drafted and reviewed ([plan 1b-4a-p4](../plans/2026-11-05-etw-sensor-1b-4a-p4-plan.md)). Comes before plan 1b-4b-1 ([1b-4b notes](2026-11-04-etw-sensor-1b-4b-brainstorm-notes.md), B2). The question: why a Launch can go out without its hash and signature under load (spec §15.3, plan 1b-4a-p3's F11), which fails `agent-live` now and then.

## Decisions

| # | Question | Decision |
|---|---|---|
| C1 | The hash workers | **A (2026-11-04): both hash workers run at normal priority.** The live test's Launch lost its hash in 0 of 56 runs, against 13 of 24 at below normal (P2), and module hashes keep up (P3). Rejected: (B) a dedicated Launch worker with modules below normal (2 of 32, and modules stop getting hashes on a busy machine, which an attacker could cause on purpose); (C) keep below normal and raise the 1 s deadline (jobs waited 10 s and more, and R4 rejected longer deadlines: they hold back every later event). |
| C2 | The seeder and the deadline clock | **A (2026-11-04): the seeder and its namer thread run at normal priority too; the deadline clock stays as it is and goes into the spec's Known Limitations.** The seeded check failed in 10 of 16 runs at below normal and 0 of 16 at normal, and the start-up pass fell from 12–30 s to under 1 s (P8); with on-miss seeding off that pass is nearly all the seeder does, once, at start-up (corrected 2026-11-05: this row first said the CPU budget bounds it, but §7.4's budget does not charge the start-up pass). Fixing the clock would save about 2 Launches in 3,000 under overload on a release build (P9). Rejected: (B) fix the clock now as well (a benefit too small to measure; corrected 2026-11-05: this row also said "a much larger plan", an estimate never traced, and the plan's review thought re-stamping a deadline at hand-off might be small, so the rejection rests on the benefit alone); (C) hash workers only (`agent-live` would keep a known starvation failure). |
| C3 | The cost of normal priority | **A (2026-11-05): keep normal priority, and carry Session A's stalls into the 1b-4b-1 brainstorm as an open problem.** The stalls happen on `main`'s build too, so going back would not remove them; even if normal priority doubled them, that is about 1.2% of Launches against 0.6%, only with every CPU busy, against about 4,750 misses a run below normal. The user first chose A from a summary that left out `actor_dropped` and called the bursts "not loss", which was wrong (the plan's review, M1), and chose it again with P13 and P14. Rejected: (B) 20 runs per build first, about 80 minutes (it would show only a large difference, and a large one would call for another fix, not a return to below normal); (C) find the cause inside this plan (a new brainstorm in a small, verified plan). |

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
- **P8, the seeder at normal priority** (run 41976765339, hash workers normal, all CPUs busy, 16 runs each, debug and release): `seeded_file_handle_update` failed in 10 of 16 at below normal and 0 of 16 at normal. The start-up pass took 12–30 s at below normal (eight runs reached the test's 30 s wait) and 0.26–0.75 s at normal. Both the seeder and its namer thread were raised. What bounds them is that the pass runs once and is short: the CPU budget (`seeder_cpu`) charges re-reads, not the start-up pass (§7.4), and with on-miss seeding off no re-read is ever sent (corrected 2026-11-05: this said re-reads come while the pass is pending, but `ask` returns at once when on-miss seeding is off). This does not settle H2's other case, a table read that fails.
- **P9, the deadline clock on a release build** (the same run, 8 runs each, overload of about 1.2M events on release and 2.5M on debug): release: 2 of 3,109 Launches missed on the pipeline's side and the live test's Launch kept its hash in 8 of 8. Debug: 1,639 of 4,363, the hash missing in 6 of 8, and most other checks failing too, because the debug pipeline cannot keep up. So the flaw is real but, at these loads, almost entirely a debug-build effect.
- **P10, queue drops did not get worse at normal priority** (2026-11-05, from the same runs' reports): `no_queue_drops` failed in 2 of 24 runs at below normal and 0 of 56 at normal. No thread of the agent runs above normal, so at normal the hash workers share the CPU with the ETW consumers and the pipeline as equals. The live test's report does not carry the ETW sessions' own loss, so these runs say nothing about it.
- **P11, the check can fail** (2026-11-05, run 42078592869): `the_workers_run_at_normal_priority` reads each thread's priority from the running agent. Putting the call back on the hash workers, the seeder or the namer failed that check and no other of the 52; the thread showed -1 (below normal), and 0 without a revert.
- **P12, `agent-live`'s tests watched at normal priority, no load added** (2026-11-05, run 42079443395, on `scratch/p4-reverts` at `f114fe7`: `scratch/p4`'s code plus its workflow; 8 fresh VMs, 2 passes each): the live test passed 16 of 16 and the binary test 16 of 16, every check. One pass happened to reach the real failure's scale, 257,826 events delivered against its 228k: 3 misses, and its Launch kept its hash. That is a single run. Misses were 0 in 13 passes, 3 in two, and 63 in one, a light pass of 14,476 events whose Launch also kept its hash. The 63 are not explained: the reports hold no timing, they do not follow `late_arrivals` (a pass with 310 had none), and being in flight at stop (P7) is possible but not shown. So misses remain at normal priority; what these runs show is that the live test's Launch stopped losing its hash.
- **P13, the host run: what normal priority costs** (2026-11-05, `spikes/run-p4-cost.ps1`, result `20261105-194041`; AC power, the Silent plan throughout, 22 CPUs, each run 90 s with every CPU busy and a cold hash for each launch; no agent, then B N B N N B N B, then no agent). All 8 agent runs stopped cleanly.

| Per run | No agent | Below normal (main) | Normal (`scratch/p4`) |
|---|---|---|---|
| Other program's work (chunks, mean) | 743,507 | 738,790 (-0.63%) | 739,152 (-0.59%) |
| Its chunks over 50 ms (mean) | 66 | 54 | 85 |
| Its p99.99 (mean) | 49.3 ms | 42.8 ms | 56.8 ms |
| Agent CPU (mean) | | 8.1 s | 11.0 s |
| Enrichment misses | | 10,323, 4,061, 2,647, 1,958 | 0, 0, 0, 0 |
| Service queue drops | | 1,592, 0, 0, 0 | 0, 0, 0, 0 |
| ETW session and kernel and user queue loss | | 0 | 0 |
| Dropped Launch halves (`actor_dropped`, Process) | | 0, 11, 0, 0 | 21, 0, 0, 0 |
| `launch_join_miss` | | 0, 22, 0, 0 | 42, 0, 0, 0 |

  - **Other programs' throughput:** no difference seen between the builds (362 chunks, 0.05%; p = 0.91 by exact permutation of the 8 runs), though 4 runs a build could not show one under about 0.8% (a 95% interval). Either build costs about 0.6%, close to the agent's own CPU.
  - **Other programs' worst moments:** possibly a little worse at normal, not shown. Chunks over 50 ms and p99.99 were higher at normal in all 4 adjacent pairs (+31 chunks, about 1 in 24,000; +14 ms), but p = 0.09 and 0.06 before allowing for 7 figures compared, and the two no-agent runs differ from each other by as much (52 and 80 chunks; 43 and 55 ms). A small cost is plausible: a normal-priority worker takes turns with the other program where a below-normal one waits.
  - **ETW loss:** none on either build, in the sessions or the kernel and user queues, in all 18 reports: 11.3M sensor events delivered at normal, 10.7M at below normal. This answers what P10 left open, for this load. The load was not the workers flat out: they used about 7.5 s of CPU between them in about 110 s, and the start-up pass ended before it began, so the seeder was not exercised.
  - **Misses:** the 0s are real: the field is present in all 9 reports at normal. The misses at below normal are not only P7's launches in flight at stop: 8,958 of the 18,989 are in reports that did not end at stop. This is the starvation of P1 on the real machine, not only on CI's runners. The counter holds Launches and module loads together, and each run's buffer was wiped, so how many were Launches is not known.
  - **Agent CPU:** the 3 s more is nearly all the hash workers (median 7.5 s against 4.8 s). The likely reason is that at normal they finish their work, not that a hash costs more; this was not measured.
  - **Session A's delivery stalled, on both builds, and that is loss** (corrected 2026-11-05: this said the bursts were not loss; `cost_summary.py`'s list left out `actor_dropped`, which §9.3 counts as loss, and the plan's review found it): about 20,000 late arrivals, about the 750 ms hold at this load, in each of two reports of one run at normal and one report of one run below normal; every other report had at most 326. Only those three had `actor_dropped` (Process: 10, 11, 11) and `launch_join_miss` (10, 32, 22). A classic half whose process is gone when its join window ends is dropped, one of each (`proc.rs`); a Kernel-Process half that then goes out alone is a second join miss. The counts fit that exactly (10 + 32 = 2 × 21, 22 = 2 × 11): about 32 of 3,600 Launches without their command line and user, 21 at normal and 11 below normal. That is inferred, since each run's buffer was wiped. Three stalls in 8 runs, two of them in one run, cannot separate the builds; one is on `main`'s build, so this plan did not bring them. Why delivery stalled is not known. A stall during the binary test would fail `every_report_measures_zero_loss`.
- **P14, the same signal on the CI runner** (2026-11-05, the reports of runs 41969285094, 41970601780 and 41976765339, which hold late arrivals but not `actor_dropped`): at the same loads, 1,000 late arrivals or more in 0 of 24 runs below normal and 0 of 112 with the hash workers at normal (at most 64), and the live test's Launch kept its command line in all of them. 7 of 8 overload runs on a debug build did stall (up to about 1M late arrivals; the Launch lost its command line in 6), on a pipeline that cannot keep up (P9), against 0 of 8 on release; and 1 of 32 with the Launch worker. Each run is about 7 s on 4 vCPUs, so this shows no link to priority, and does not rule one out.

## What is not shown

- The real failure was not reproduced: the probe's loads are about 2–8 times its 228k events, and no timing exists from that run. The same signature at a load 2–3 times higher makes starvation the likely cause, not a proven one.
- 0 of 56 is not never. The 1 s deadline stays.
- What normal priority costs other programs was measured on one laptop, on one plan, against one synthetic program (P13): nothing in throughput, and at most a small cost in the worst moments. An interactive program's feel was not measured.
- ETW session loss was 0 on both builds at this load (P13); a heavier load, the workers flat out or a slower machine is not covered.
- Why Session A's delivery stalled, and whether the workers' priority changes how often (P13, P14).
- P4's 1–3 Launches in about 400 on the pipeline's side, at light load, are not reconciled with P9's 2 of 3,109 under overload: passes under 50 ms leave the clock little to take, so the two may not measure the same thing.

## Next steps

1. Verify-first on `scratch/p4` (from `main`): Task 1 (C1) and Task 2 (C2) done, CI green, three reverts caught (P11), `agent-live`'s tests watched over 16 passes (P12).
2. The host run, `spikes/run-p4-cost.ps1`: done (2026-11-05, P13).
3. The spec text: `scratch/p4-review`, as the plan's Task 3 (§3.2, §6.3, §7.4, §13, §15.3, §16, §17).
4. The plan: drafted and reviewed (M1 and its fixes), C3 decided again. Then approval, build, and plan 1b-4b-1.
