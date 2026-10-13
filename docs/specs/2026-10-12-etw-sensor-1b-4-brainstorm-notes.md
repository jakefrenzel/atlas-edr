# Plan 1b-4 — Brainstorm Notes (handoff)

Status: **1b-4a planned** (plan written and independently reviewed 2026-10-13, awaiting approval); 1b-4b and 1b-4c to brainstorm, after the profiling step (D6). Started 2026-10-12. Sub-project 1, spec `2026-10-01-etw-sensor-design.md`. The contract from earlier plans: the "Interfaces for later plans" sections of plans 1b-1 and 1b-3c.

## Decisions

- **S1 A (2026-10-12): plan 1b-4 is split into three plans**, each brainstormed, planned, reviewed and built before the next:
  - **1b-4a, a runnable agent:** the `atlas-agent` binary, `run` (console, Ctrl+C clean stop, single-instance mutex), `agent.toml`, the data directory check (§11.1), the buffer writer thread as the sink, Sensor Health reports, `dump`, the diagnostic log.
  - **1b-4b, blinding detection:** the watchdog (sessions, `ProcessTrace` returning, provider checks, the Kernel-EventTracing decision), session restart (spare queues, the spare `FastRead`), the three canaries, Event Log Activity records. (The `file.op_end`-off enable fix moved into 1b-4a: verify-first found the intake half too, plan 1b-4a F1.)
  - **1b-4c, the service and acceptance:** `service install|uninstall|start|stop`, the stop handler's allowance, Windows Event Log start-up errors; the blinding, crash-safety and 24-hour tests, the §13 budget, the unpaired-Cleanup measurement, the runbook.
  - Rejected: (B) two plans (runnable + service, then watchdog + acceptance), each ~1.5× plan 1b-3c, with service install landing before blinding detection; (C) one plan, ~2× plan 1b-3c.

### Plan 1b-4a (chosen 2026-10-12: D1 A, D2 A, D3 A, D4 A, D5 A, D6 A)

- **D1 A, where Sensor Health is assembled:** a new health thread, which becomes the watchdog thread [7] in 1b-4b. Every 60 s it takes a counters snapshot from the pipeline through `control()` (one pass, ≤ 10 ms), reads the atomics, the session queries and the process's CPU time and working set, takes the difference from the last snapshot and sends the draft to the writer thread, which fills in the `buffer` group and the gap from its own stats as it appends. Rejected: (B) the pipeline thread in the driver loop (Windows-only sources injected into the portable loop; the watchdog still needs its own thread and route to the writer); (C) the writer thread (it would wait on the pipeline while not draining the sink's channel).
- **D2 A, the data directory under `run`:** 1b-4a builds the create-and-verify code (§11.1) and `run` uses it; 1b-4c's `service install` calls the same code. Verified: `C:\ProgramData` grants `BUILTIN\Users:(CI)(WD,AD,WEA,WA)`, so an inherited-ACL `Atlas\` is writable by any user. Rejected: (B) defer the ACL to 1b-4c (every host run until then uses a user-writable directory, and 1b-4c needs a migration); (C) no default directory (same weakness, wherever it is).
- **D3 A, `dump`'s format:** protobuf's standard JSON mapping, the one the replay snapshot already uses (`prost-reflect` with `atlas_proto::FILE_DESCRIPTOR_SET`), one rendering function shared. A `--readable` switch can come later. Rejected: (B) readable conversions (UUIDs, hex hashes, RFC 3339; an Atlas-only format to maintain); (C) OCSF JSON (the exporter is a later adapter).
- **D4 A, end-to-end proof:** a binary-level test in CI's `agent-live` job (`run`, an actor scenario, Ctrl+Break, `dump`: every scenario class, a Sensor Health report with zero loss, nothing lost on the clean stop, the protected DACL created, a loose-ACL directory refused, a second instance refused), plus one host run of about an hour of normal use by the user, summarised from the Sensor Health records and the log (losses, CPU and working set against §13, both unpaired-Cleanup parts). Verified: Ctrl+C cannot be sent to a child in its own process group; Ctrl+Break can. Rejected: (B) CI only (host figures wait for 1b-4c); (C) host only (no regression guard).
- **D5 A, defaults (accepted together):**
  1. the binary `atlas-agent` (`src/main.rs`, `clap` derive); 1b-4a has `run` and `dump`;
  2. `run`: elevation required, `Global\Atlas-Agent` mutex, `--data-dir` (default `C:\ProgramData\Atlas`), sessions `Atlas-Sensor`/`Atlas-Process`, Ctrl+C, Ctrl+Break and console close all stop cleanly;
  3. the directory: SDDL `O:BAD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)`; verified at start (owner Administrators, protected, only SYSTEM and Administrators full control, no reparse point), `buffer\` and `logs\` likewise;
  4. the sink moves each `Event` into a bounded channel (65,536) and never blocks; the writer thread encodes; drops count in `loss.buffer_backlog_drops` (doc comment widened, no new field);
  5. rolling retention, 1 GiB cap; a gap's time from decoding its record;
  6. Sensor Health every 60 s, early on a rising loss counter (checked every 5 s), a final one at the clean stop; the first carries the buffer's recovery; session loss counters differenced with wrapping arithmetic; `GetProcessTimes`, `GetProcessMemoryInfo`; time from QPC through the anchor; UUIDv7 ids;
  7. the two unpaired-Cleanup parts go to the diagnostic log with each report; the schema keeps the sum;
  8. `agent.toml` in the data directory (`serde`, `toml`, unknown keys rejected), the §11.2 keys that exist today, `transport = "none"` only; an invalid file stops start-up with a clear message (the Event Log comes with 1b-4c);
  9. the log: `tracing` to the console and `logs\`, daily, 7 files, level from config (default `info`), self-filtered;
  10. `dump --follow --from <segment:offset> --class <name>`, read-only, admin;
  11. one plan, one build PR, the spec revisions by a docs task; verify-first in a scratch worktree and on the CI runner (pushed scratch branch); independent review with reverted-fix checks.
  (D5.2's `Global\Atlas-Agent` mutex was replaced after the review by a private namespace bounded by the Administrators SID: any user can create a mutex in `Global\`. D5.6's "time from QPC through the anchor" was never what the code did: a report's time is the system clock the anchor reads.)
- **D6 A (2026-10-12), the host run's budget overrun:** the hour run lost nothing but averaged 10.1% of one core and grew to 200 MB, against §13's 2% and 150 MB. Plan 1b-4a goes ahead and records the figures; the cause is profiled (CPU per thread, then per stage) before plan 1b-4b, ahead of any §13 fallback; each fix or fallback is a decision-log entry. Rejected: (B) profile first, then write 1b-4a (a delay, for a cause most likely in earlier code); (C) apply the first fallbacks now (guesses the cause, gives up coverage).
- **The review (2026-10-13):** two major findings (any user could squat `Global\Atlas-Agent`; a panicked pipeline thread panicked `run`, losing the final report and the queued records) and six minor ones, all fixed with tests checked by reverting the fixes; details in the plan's Review Log. For finding 4, a `--data-dir` below a folder its user controls, the user chose **A**: check every folder above the data directory (rejected: document `--data-dir` as admin-only; let `run` use only the default).

## Facts gathered

- No binary exists yet; `config.rs` holds only `Config::default()`.
- Sensor Health's sources live on four kinds of thread: the pipeline's `Counters` (pipeline thread, read through `Running::control()`); `IntakeCounters`, `ServiceCounters` and `Consumer::panics()` (atomics); the sessions' events and buffers lost (`atlas_etw` `Session::query`); the buffer's `Stats`, gaps, `is_failing`, `disk_bytes` (the writer thread).
- The schema has one field for the unpaired Cleanups, `quality.file_cleanup_unpaired`; its two parts (`cleanup_unpaired`, `cleanup_outcome_late`) have no field of their own.
- No schema field counts events dropped between the sink and the writer thread.

## Next steps

1. 1b-4a brainstorm: done (D1–D5 above).
2. Verify-first: done (scratch worktree, CI runner, the user's elevated runs and hour run).
3. Plan 1b-4a: written (`docs/plans/2026-10-13-etw-sensor-1b-4a-plan.md`) and independently reviewed, the findings fixed. Next: the user's approval, the plan PR, then the build PR.
4. The profiling step (D6), then the 1b-4b brainstorm.
