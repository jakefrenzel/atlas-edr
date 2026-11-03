# Sub-project 1b-4a-p3 — The File Map's Memory Implementation Plan

> **Status:** Draft (2026-11-02), after an independent review (Review Log; its fixes are in), for the user's approval. **For agentic workers:** steps use checkbox (`- [ ] `) syntax for tracking. Task 4 is a run on the host by the user, from a script. Nothing in this plan needs the VM or a kernel driver.

**Goal:** Bring the agent's working set within §13's 150 MB over a workday. The hour run (decision log, 2026-10-31) met the CPU budget (1.59% of one core) but reached 281 MB: the file map kept FileObjects whose Close Windows sends late or not at all. Two fixes, each decided from measured facts (the p3 notes, H1 and H2), and two the review found next to them (H3 and Task 3):
- **entries leave the file map after their Cleanup** (H1), not at their Close: past its Cleanup an entry serves only the Cleanup's outcome and late writes; a 60 s record of cleaned FileObjects absorbs late writes;
- **no provisional entry that nothing can name** (H2): after the start-up pass, with on-miss seeding off, a write on a handle the agent never saw opened makes no entry;
- **one outcome window** (H3): the callback pairs a Cleanup with its outcome within 1 s, the pipeline kept the Cleanup's record for only 250 ms, and a Delete whose outcome came in between was lost; one constant, 1 s, now sets the pairing, the pipeline's record and the retire delay;
- **an 8.3 expansion names only the file it was asked about** (Task 3): a late answer no longer puts one file's path on the next file at the same address.

**Architecture:** in `pipeline::file`, the FileObject map's stage, with one constant in `cleanup`, one line in `intake`, one check in `pipeline::expand`, and two names in `sizes=`.
- **`cleanup::OUTCOME_WINDOW`** (1 s): the intake's pairing window (it was the QPC frequency, 1 s), the pipeline's Cleanup record (it was the 250 ms confirm window) and the retire delay.
- **`FileEntry`:** `cleaned: bool` becomes `cleaned: Option<i64>`, the QPC of its Cleanup.
- **`Files`:** `outcome`, the window in ticks; `retiring`, a queue of entries past their Cleanup, in Cleanup order, each with the stream time it is due; `cleaned`, a `Recent<u64>` of FileObjects whose entry left (60 s, at most 65,536); `retire(fo, at)` takes an entry out if it is still the one cleaned at `at` and no event waits on it.
- **`on_file_cleanup`** queues the entry, or retires it at once with `file.op_end` off; **`file_confirm`** (every stream step) retires what is due (strictly before the stream), each queued entry at most once a pass, and requeues an entry an event still waits on; it expires the record. An entry made for an outcome on an unknown handle is queued too.
- **`on_file_write`:** on a miss, `on_unknown_write`: a FileObject in the record counts `writes_after_cleanup`; after the start-up pass, with on-miss seeding off, an unknown one makes no entry.
- **`on_file_create`** and **`on_file_close`** take the FileObject out of the record; a Create that finds it there counts `file_object_replaced`.
- **`Expands`:** each slot keeps the NT path it asked about; `on_expanded` fills a map entry only if it still has that path.
- **`Pipeline::sizes`:** `cleaned_fos`, `retiring_fos`.

**Tech stack:** Rust 1.97 (edition 2024). No new dependency. Measurement tools (git-ignored, Task 4): plan 1b-4a's hour script and summary, the log copier, and `run-p3-host.ps1` (the agent while Git's `du` walks a folder).

**Spec:** `docs/specs/2026-10-01-etw-sensor-design.md`, revision 3, with the clarifications of plans 1b-2 to 1b-4a-p2. Section numbers (§) refer to it. The p3 notes (`docs/specs/2026-10-31-etw-sensor-1b-4a-p3-brainstorm-notes.md`) have the facts behind each decision (F1–F9).

**Verification note (2026-10-31 to 2026-11-02, host, Windows 11 build 26200, and the CI runner, build 26100):** every code block below was compiled and run in a scratch worktree of `main` (`e3c7211`), `scratch/p3-review`, one commit per task (Task 1 `027585a`, Task 2 `a9cf02c`, Task 3 `afa65fa`, Task 5's spec text `cbd46a1`). The draft's commits (`scratch/p3`) were rebased with the review's fixes folded into the task they belong to.
- **Lint:** `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` are clean at the end of every task.
- **Unelevated:** `cargo test --workspace` passes 452 tests and ignores 19 after Task 1, 454 tests and ignores 19 after Task 2 and 455 tests and ignores 19 after Task 3 (plan 1b-4a-p2 left 441 and 19). `atlas-agent` has 263, 265 and 266 unit tests (252 before). Counts were measured at each task's commit in the round trip.
- **On the CI runner** (the scratch branch pushed, no PR): run 41259913685 on Task 3's commit passed every job, the binary test and the agent live test among them; the draft's two commits passed too (runs 41250556287, 41253970989).
- **On the host:** the probe runs behind F3 and F4 (a probe build on `scratch/fm-probe`, never for `main`); `run-p3-host.ps1` on the draft's H1 build (F6), its H1 + H2 build (F7), and Task 3's build, the reviewed code (F9).
- **Reverts:** every rule in the Review Focus was checked by reverting it in the source and running its test, which must then fail (`spikes/p3-plan-src/mutate.py`, plan 1b-4a-p1's with one change: a revert whose test filter matches no test now fails the check). 23 reverts, all caught on the final code:
  - h1: entries never leave in the confirm pass: `a_walk_without_closes_leaves_only_open_handles_in_the_map` (caught)
  - h1: the cleaned record never expires: `a_walk_without_closes_leaves_only_open_handles_in_the_map` (caught)
  - h1: a late write after the entry left makes a provisional entry: `a_late_write_after_the_entry_left_is_absorbed` (caught)
  - h1: with OperationEnds the entry leaves at its Cleanup, before the outcome: `the_cleanup_outcome_still_finds_the_entry` (caught)
  - h1: without OperationEnds the entry waits a window anyway: `without_operation_ends_an_entry_leaves_at_its_cleanup` (caught)
  - h1: an entry leaves while an event waits on it for the seeder: `an_entry_waited_on_stays_until_answered` (caught)
  - h1: a waited-on entry is not looked at again: `an_entry_waited_on_stays_until_answered` (caught)
  - h1: retirement takes whatever entry holds the address: `a_reused_address_inside_the_window_keeps_its_new_entry` (caught)
  - h1: retirement takes any cleaned entry, not the one cleaned then: `a_second_cleanup_on_a_reused_address_waits_for_its_own_outcome` (caught)
  - h1: a Create leaves the address in the cleaned record: `a_reused_or_closed_address_leaves_the_cleaned_record` (caught)
  - h1: a Close leaves the address in the cleaned record: `a_reused_or_closed_address_leaves_the_cleaned_record` (caught)
  - h1: cleaned_fos missing from sizes=: `the_sizes_name_each_bounded_structure` (caught)
  - m1-major: the pipeline's Cleanup record expires after the confirm window: `the_cleanup_outcome_still_finds_the_entry` (caught)
  - m1-major: the entry retires after the confirm window: `the_cleanup_outcome_still_finds_the_entry` (caught)
  - nit: an outcome exactly one window late finds no entry: `an_outcome_exactly_one_window_late_still_finds_its_entry` (caught)
  - minor m1: a Create on a cleaned address is not counted: `a_create_on_a_cleaned_address_counts_a_replaced_file_object` (caught)
  - minor m3: an entry an outcome makes never leaves: `an_entry_an_outcome_makes_leaves_too` (caught)
  - nit: retiring_fos missing from sizes=: `the_sizes_name_each_bounded_structure` (caught)
  - h2: a write on an unknown handle after the pass makes an entry: `without_on_miss_seeding_only_the_start_up_pass_is_waited_for` (caught)
  - h2: no entry during the start-up pass either: `without_on_miss_seeding_only_the_start_up_pass_is_waited_for` (caught)
  - h2: no entry with on-miss seeding on: `with_on_miss_seeding_an_unknown_write_still_asks` (caught)
  - minor m2: H2's rule comes before the cleaned record: `after_the_start_up_pass_a_late_write_is_still_absorbed` (caught)
  - t3: an expansion fills whatever entry holds the address: `a_late_expansion_does_not_name_the_next_file_at_its_address` (caught)
- **Round trip:** the code was rebuilt from this document alone in a fresh LF worktree of `main` (`spikes/p3-plan-src/roundtrip.py`): every diff applied, and every task's tree is identical to its verified commit; the test counts above were measured there.
- **Not run yet:** Task 4 (the hour run) on the build branch; the Linux build of the tests is run by CI only.

## Global Constraints

- **Branches.** This plan is reviewed on `docs/1b-4a-p3-plan`. The build runs on `feat/1b-4a-p3`, created from `main` after this plan merges. Never commit to `main`. (Claude may merge a PR itself once every check on its current head is green; the user granted that on 2026-10-05.)
- **Portable code:** `pipeline` stays portable; Linux CI builds and tests it.
- **The ETW callback is untouched** (§3.2), but for its pairing window's source: the same 1 s, from `cleanup::OUTCOME_WINDOW`.
- **No behaviour change but the decided ones:** the same events, fields and counters as before for the same input, except: a write on an unknown handle after the start-up pass no longer leads to an `unknown_file_object` count at its Cleanup (H2); a write more than 60 s after its entry left is an unknown handle's, not `writes_after_cleanup` (H1.2); a SetAttributes or rename on a FileObject whose entry left is an unknown handle's (F4 saw none); a Delete whose outcome comes 250 ms–1 s after its Cleanup is now reported (H3); a late 8.3 expansion no longer names the next file at its address (Task 3). `file_object_replaced` keeps its meaning: a Create on a cleaned FileObject counts it. The replay tests still turn the CI recording into the same snapshot.
- **Elevation:** Claude never runs elevated on the host (plan 1a, D2). Task 4's run is the user's.
- **The host's data stays on the host:** logs and dumps stay in the git-ignored `spikes\results\`; only summary figures enter the docs.
- **No schema change.**
- **Formatting and lint** at the end of every task, as in the verification note.
- **Shell:** PowerShell 7 (`cargo` commands are the same in bash). Edits that contain backslashes are made with an editor, never a heredoc.

## Decisions

The p3 notes have the facts; the decision log has each entry.

### H1, file map entries whose Close never comes (chosen 2026-10-31: A)

- **(A) Chosen: an entry leaves the map once its Cleanup's outcome is handled; a small bounded record of recently cleaned FileObjects absorbs late writes.** The map then holds open handles, and memory no longer follows how late Windows closes files (F2, F3).
- (B) Rejected: smaller entries (the opener by reference, shared paths): a walk like `du`'s still fills the map to its cap.
- (C) Rejected: a lower cap only: evicts long-open handles' names and still lets memory reach the cap.

### H1.1–H1.5, the details (chosen 2026-10-31, accepted together after F4)

1. **When:** one outcome window after the Cleanup (H3: 1 s), the window the Cleanup's outcome record lives for; at the Cleanup itself with `file.op_end` off; an entry that an event waits on for the seeder stays until the event is answered or leaves the completion stage, then leaves at the next pass.
2. **The record:** FileObject values only, kept 60 s, at most 65,536, oldest out first (about 4 MB at the cap, its order queue included). F4 found no SetAttributes, Rename or SetDelete on a FileObject past its Cleanup, so no path is kept; 99.6% of late writes came within 60 s. A write that finds its FileObject there counts `writes_after_cleanup`; past it a late write is an unknown handle's, today's rule.
3. **Reuse:** a Create or a Close on a recorded FileObject takes it out; a Create there counts `file_object_replaced` (review m1).
4. **Observability:** `sizes=` gains `cleaned_fos=` and (review) `retiring_fos=`; the map's 262,144 cap stays as a safety net; no new counter.
5. **Tests:** each rule a unit test checked by reverting it; a walk test leaves the map at its open-handle count; then the hour run again.

### H2, provisional entries nothing can name (chosen 2026-10-31: A)

- **(A) Chosen: after the start-up pass, with on-miss seeding off, a write on a FileObject the agent never saw opened makes no entry** (F5). Its Update would be dropped as `unknown_file_object` anyway, and the entry would hold memory until a Close that may not come. During the start-up pass, and with on-miss seeding on, writes make provisional entries as before. A write on a FileObject in the cleaned record is absorbed first (review m2). `unknown_file_object` no longer counts the Update a later Cleanup of such a handle would have dropped (in F5's run none of them got a Cleanup).
- (B) Rejected: expire provisional entries through the same queue: more code for the same effect; it only keeps the count.
- (C) Rejected: leave it for 1b-4c: bounded by the cap, but a 24-hour run would sit near it, about 100 MB of map.

### H3, the Cleanup's outcome window (chosen 2026-10-31: A, after the review's M1)

- **(A) Chosen: one window, 1 s (`cleanup::OUTCOME_WINDOW`), for the callback's pairing, the pipeline's Cleanup record and the retire delay.** The callback already paired within 1 s (§5.1); the pipeline's record expired after the 250 ms confirm window, so a Delete whose outcome came in between was lost and nothing counted it (reproduced on `main`). F4's outcomes all came within 100 ms on local NTFS, but a filter that scans at Cleanup, or SMB, can take longer, and a delete-on-close of a freshly written file is the dropper pattern. The cost: about one second of Cleanups in the map, a few hundred entries during a `du` walk (F9: `retiring_fos` at most 1,177).
- (B) Rejected: keep 250 ms and count the drop: the Delete is still lost.
- (C) Rejected: narrow the callback to 250 ms: a slow outcome would then be counted as unpaired, and the Delete still lost.

### The review's other fixes (accepted together, 2026-10-31)

m1 (`file_object_replaced` counts a Create on a cleaned FileObject), m2 (a test after the start-up pass), m3 (an entry made for an outcome leaves too), the window edge (an outcome exactly one window late still finds its entry), `retiring_fos` in `sizes=`, the record looked at only on a miss, the text fixes, and Task 3 (the expansion bug, on `main` since plan 1b-3a, fixed here). Not changed: the seeder's `None` branch does not look at the record (safe, as the review found); H2 when the start-up pass never replies goes to plan 1b-4b (Interfaces). The Review Log has each.

## Findings

The p3 notes' F1–F9, in short:
- **F1, the hour run:** 74.7 minutes of normal use: CPU 1.59% of one core, nothing lost; the working set 51 → 281 MB.
- **F2:** only the file map grew (`sizes=`), 3,000–5,000 entries a minute, to its cap; each working-set jump was one doubling of its table.
- **F3:** 204,526 of 231,340 entries were a `du` walk's files past their Cleanup, with no Close (a probe build, 30 minutes, 312 MB); Close reaches the pipeline, so nothing is lost; Windows sends it late or not at all.
- **F4:** past its Cleanup a FileObject sees only the Cleanup's outcome (within 100 ms), late writes (99.6% within 60 s) and its Close; no SetAttributes, Rename or SetDelete.
- **F5:** provisional entries grew 400–600 a minute, never cleaned up or closed.
- **F6, F7:** a 10-minute `du` walk with the draft's H1 build: the file map 4.6k → 9.6k, at most 70 MB; with H2 too: 4,485 → 4,511, at most 71 MB, nothing lost.
- **F8, tooling:** `Start-Process` in Windows PowerShell 5.1 does not quote arguments; the agent's log rolls over at UTC midnight; a revert whose filter matches no test passed as a miss.
- **F9, the reviewed code (Task 3's build):** the same walk: the file map 4,512–4,873 entries through the 13 minutes, `cleaned_fos` 4.0k–8.4k during the walk, `retiring_fos` at most 1,177, the working set at most 65 MB; nothing lost, no unpaired or late outcome.

## Deliberate clarifications of the spec (applied to the spec text in Task 5)

1. **§7.1:** an entry leaves one outcome window after its Cleanup, not at its Close; the cleaned record; late writes; `file_object_replaced` on a recorded FileObject (H1, m1, m3).
2. **§7.1:** no provisional entry after the start-up pass with on-miss seeding off (H2).
3. **§9.3:** `cleaned_fos` and `retiring_fos` in `sizes=`.
4. **§13, §15.3:** the hour run's figures and F1–F9.
5. **§17:** the clarifications, and the review's fixes (H3 and Task 3 among them).

## Review Focus

1. **An entry past its Cleanup leaves one window later, and the map then holds open handles only** (`a_walk_without_closes_leaves_only_open_handles_in_the_map`: 1,000 files created and cleaned up with no Close, one open handle; after the window the map holds 1 and the record 1,000; after 60 s the record holds none).
2. **The Cleanup's outcome still finds its entry, inside the whole 1 s window** (`the_cleanup_outcome_still_finds_the_entry`: a delete-on-close handle's outcome 894 ms after its Cleanup gives a Delete with the path and the opener; `an_outcome_exactly_one_window_late_still_finds_its_entry`).
3. **Without OperationEnds the entry leaves at its Cleanup** (`without_operation_ends_an_entry_leaves_at_its_cleanup`).
4. **A late write is absorbed while the record holds its FileObject, and is an unknown handle's after** (`a_late_write_after_the_entry_left_is_absorbed`; after the start-up pass too, `after_the_start_up_pass_a_late_write_is_still_absorbed`).
5. **An entry an event waits on stays until the event is answered, then leaves** (`an_entry_waited_on_stays_until_answered`: the window passes with the Update waiting on the start-up seeder; the answer still names it; then the map is empty).
6. **A retirement takes only the entry it queued** (`a_reused_address_inside_the_window_keeps_its_new_entry`: a Close and a new Create inside the window keep the new open handle; `a_second_cleanup_on_a_reused_address_waits_for_its_own_outcome`: a second Cleanup on the address waits for its own outcome).
7. **A Create or a Close takes an address out of the record, and a Create there counts `file_object_replaced`** (`a_reused_or_closed_address_leaves_the_cleaned_record`, `a_create_on_a_cleaned_address_counts_a_replaced_file_object`).
8. **An entry made for an outcome on an unknown handle leaves too** (`an_entry_an_outcome_makes_leaves_too`).
9. **`sizes=` names `cleaned_fos` and `retiring_fos`** (`the_sizes_name_each_bounded_structure`).
10. **After the start-up pass, with on-miss seeding off, an unknown write makes no entry; during the pass, and with on-miss seeding on, it still does** (`without_on_miss_seeding_only_the_start_up_pass_is_waited_for`, changed: its second half now expects no entry and no count; `with_on_miss_seeding_an_unknown_write_still_asks`).
11. **A late 8.3 expansion fills only the entry it was asked about** (`a_late_expansion_does_not_name_the_next_file_at_its_address`).
12. **The pipeline's bookkeeping still empties** (`bookkeeping_is_freed_when_no_reply_comes`, changed: it runs two more seconds, since an entry past its Cleanup leaves a window after the events waiting on it; the test-only list gains the queue and the record).
13. **Nothing else changes:** the CI recording still gives the scenario and the snapshot, the driver loop the synchronous replay's events, and every other earlier test passes unchanged.

Every rule in 1–11 was checked by reverting it: its test fails (verification note). One rule has no revert check of its own: the retirement pass looks at each queued entry at most once (a final flush runs at the end of time, where a requeued entry is due again at once). Without it `a_clean_stop_drops_what_a_deadline_would` hung in the draft; a revert would hang the suite, so it is not in the revert list.

## File Structure

```
crates/atlas-agent/src/cleanup.rs         OUTCOME_WINDOW (H3)
crates/atlas-agent/src/intake.rs          the pairing window from OUTCOME_WINDOW
crates/atlas-agent/src/pipeline/file.rs   FileEntry.cleaned as a time; Files' retiring queue and cleaned record; the rules
crates/atlas-agent/src/pipeline/mod.rs    Files::new gets the outcome window; sizes: cleaned_fos, retiring_fos
crates/atlas-agent/src/pipeline/expand.rs the NT path each expansion asked about (Task 3)
crates/atlas-agent/src/pipeline/tests.rs  the tests of the Review Focus
spikes/                                   git-ignored; Task 4: run-1b4a-hour.ps1, summarize-1b4a.ps1, copy-1b4a-hour-log.ps1, run-p3-host.ps1
docs/specs/2026-10-01-etw-sensor-design.md   Task 5
```

## Interfaces for later plans

- **The hour run** (Task 4) ends the profiling step: normal use for an hour, judged against §13 from the Sensor Health reports and the `sizes=` lines. If the working set is still over budget, what holds it comes back as a decision.
- **Plan 1b-4b (blinding detection):** plan 1b-4a-p2's interfaces, plus one item from this review: if the start-up pass never replies (its handle-table read fails), `startup_pending` stays true for the whole run and H2 never applies; Sensor Health should see it, and a deadline may end the pass.
- **Plan 1b-4c (the service):** the 24-hour run reads `file_map=`, `cleaned_fos=` and `retiring_fos=` in `sizes=`; with H1 and H2 the map should stay near the machine's open-handle count. A machine with on-miss seeding turned on still makes provisional entries (H2 leaves it), bounded by the cap.

---

### Task 1: File map entries leave after their Cleanup (H1, H3)

**Files:**
- Modify: `crates/atlas-agent/src/cleanup.rs`, `crates/atlas-agent/src/intake.rs`, `crates/atlas-agent/src/pipeline/file.rs`, `crates/atlas-agent/src/pipeline/mod.rs`, `crates/atlas-agent/src/pipeline/tests.rs`

- [ ] **Step 1: One outcome window** (H3). `cleanup::OUTCOME_WINDOW`, 1 s; the intake's pairing window comes from it.

```diff
--- a/crates/atlas-agent/src/cleanup.rs
+++ b/crates/atlas-agent/src/cleanup.rs
@@ -8,8 +8,15 @@
 //! doing it, and an undelete (a disposition set, then cleared) reports
 //! `FILE_REMAINS`.
 
+use std::time::Duration;
+
 use atlas_etw::parse::RawEvent;
 
+/// How long after a Cleanup its Irp's OperationEnd is taken as its outcome
+/// (§5.1): the callback pairs them within it, and the pipeline keeps the
+/// Cleanup's record and its file map entry as long (plan 1b-4a-p3, review M1).
+pub const OUTCOME_WINDOW: Duration = Duration::from_secs(1);
+
 /// The file system did not say (SMB shares).
 pub const UNKNOWN: u64 = 0;
 pub const FILE_REMAINS: u64 = 0x2;
```

```diff
--- a/crates/atlas-agent/src/intake.rs
+++ b/crates/atlas-agent/src/intake.rs
@@ -228,7 +228,7 @@ impl Cleanups {
             requested: Recent::new(DELETE_REQUESTS_CAP),
             request_irps: FastMap::default(),
             on_close: Recent::new(DELETE_REQUESTS_CAP),
-            window: ticks.frequency.max(1),
+            window: ticks.of(cleanup::OUTCOME_WINDOW).max(1),
             counters,
         }
     }
```

- [ ] **Step 2: The file stage** (H1, H1.1–H1.4, H3, review m1, m3). `FileEntry.cleaned` becomes the QPC of the Cleanup. `on_file_cleanup` queues the entry for one outcome window, or retires it at once without OperationEnds (an entry an event waits on is queued then too). `file_confirm` retires what is due strictly before the stream (an outcome exactly one window late still finds its entry), each queued entry at most once a pass (a final flush runs at `i64::MAX`, where a requeued entry is due again at once), expires the record, and keeps the Cleanup record for the outcome window. `retire` checks the entry is still the one cleaned then, so an address closed and reused inside the window keeps its new entry. On a miss, `on_unknown_write`: a late write that finds its FileObject in the record counts `writes_after_cleanup` and makes no provisional entry. A Create or a Close takes the FileObject out, and a Create that finds it there counts `file_object_replaced`. An entry made for an outcome on an unknown handle is queued like any other.

```diff
--- a/crates/atlas-agent/src/pipeline/file.rs
+++ b/crates/atlas-agent/src/pipeline/file.rs
@@ -10,6 +10,7 @@
 //! taken as the delete (`file_delete_outcome_unknown`).
 
 use std::collections::VecDeque;
+use std::time::Duration;
 
 use atlas_etw::parse::{FileCreate, FileHandle, FileOpEnd, FilePath, FileSetInfo};
 use atlas_schema::classes::file::{FileAction, FileSystemActivity};
@@ -33,6 +34,11 @@ const CONFIRMED_CAP: usize = 1 << 18;
 const OPENED_CAP: usize = 1 << 16;
 const CLEANUPS_CAP: usize = 1 << 16;
 const REQUESTS_CAP: usize = 1 << 14;
+/// FileObjects whose entry left after its Cleanup, kept to recognise a late
+/// write: at most this many, for at most this long (plan 1b-4a-p3, H1; 99.6%
+/// of late writes on the host came within 60 s).
+const CLEANED_CAP: usize = 1 << 16;
+const CLEANED_KEEP: Duration = Duration::from_secs(60);
 
 /// A watchlist Open's coalescing key: (actor, logged path lowercased).
 type OpenKey = (ProcessUid, String);
@@ -53,7 +59,8 @@ pub(crate) struct FileEntry {
     pub(crate) opener: Option<ProcessRef>,
     written: bool,
     delete_on_close: bool,
-    cleaned: bool,
+    /// QPC of its Cleanup, once the last handle closed.
+    cleaned: Option<i64>,
     /// QPC of the Create (or snapshot) that set it.
     pub(crate) since: i64,
     touched: u64,
@@ -91,6 +98,8 @@ pub struct Files {
     clock: u64,
     evictions: u64,
     window: i64,
+    /// A Cleanup's outcome window (`cleanup::OUTCOME_WINDOW`), in ticks.
+    outcome: i64,
     /// Operations in their failure-confirm window: (QPC, Irp, what), in stream
     /// order; a failed one is taken out (`None`) and skipped.
     held: VecDeque<(i64, u64, Option<Held>)>,
@@ -120,6 +129,13 @@ pub struct Files {
     request_irps: RecentMap<u64, u64>,
     /// POSIX-style deletes already reported: (FileKey, kind) → when.
     posix_deleted: RecentMap<(u64, u64), i64>,
+    /// Entries past their Cleanup, in Cleanup order: (when due, QPC of the
+    /// Cleanup, FileObject). Each leaves the map once its outcome's window has
+    /// passed: Windows may send the Close long after, or not for hours (H1).
+    retiring: VecDeque<(i64, i64, u64)>,
+    /// FileObjects whose entry left after its Cleanup, so that a late write is
+    /// absorbed (`writes_after_cleanup`) instead of making a provisional entry.
+    cleaned: Recent<u64>,
 }
 
 /// Who asked for a delete (§5.3: the actor of the Delete).
@@ -133,13 +149,14 @@ enum Requester {
 }
 
 impl Files {
-    pub fn new(cap: usize, window_ticks: i64) -> Self {
+    pub fn new(cap: usize, window_ticks: i64, outcome_ticks: i64) -> Self {
         Files {
             map: FastMap::default(),
             cap,
             clock: 0,
             evictions: 0,
             window: window_ticks,
+            outcome: outcome_ticks,
             held: VecDeque::new(),
             held_base: 0,
             held_by_irp: FastMap::default(),
@@ -152,21 +169,42 @@ impl Files {
             early_outcomes: RecentMap::new(REQUESTS_CAP),
             request_irps: RecentMap::new(REQUESTS_CAP),
             posix_deleted: RecentMap::new(REQUESTS_CAP),
+            retiring: VecDeque::new(),
+            cleaned: Recent::new(CLEANED_CAP),
         }
     }
 
-    /// (entries, capacity) of the confirmed and failed IRPs and the watchlist
-    /// opens (R6).
-    pub(super) fn recent_sizes(&self) -> [(usize, usize); 3] {
+    /// (entries, capacity) of the confirmed and failed IRPs, the watchlist
+    /// opens (R6), the cleaned FileObjects and the entries past their Cleanup (H1).
+    pub(super) fn recent_sizes(&self) -> [(usize, usize); 5] {
         [
             (self.confirmed.len(), self.confirmed.capacity()),
             (self.failed.len(), self.failed.capacity()),
             (self.opened.len(), self.opened.capacity()),
+            (self.cleaned.len(), self.cleaned.capacity()),
+            (self.retiring.len(), self.retiring.capacity()),
         ]
     }
 
+    /// Takes `fo`'s entry out of the map if it is still the one cleaned at
+    /// `at` and no event waits on it for the seeder. False if it must stay
+    /// longer; true once it is done with (gone, or replaced by a new Create).
+    fn retire(&mut self, fo: u64, at: i64) -> bool {
+        match self.map.get(&fo) {
+            Some(e) if e.cleaned == Some(at) => {
+                if !e.waiting.is_empty() {
+                    return false;
+                }
+                self.map.remove(&fo);
+                self.cleaned.insert(fo, at);
+                true
+            }
+            _ => true,
+        }
+    }
+
     #[cfg(test)]
-    pub(super) fn sizes(&self) -> [(&'static str, usize); 7] {
+    pub(super) fn sizes(&self) -> [(&'static str, usize); 9] {
         [
             ("file waiters", self.map.values().map(|e| e.waiting.len()).sum()),
             ("file waiter index", self.waiter_fo.len()),
@@ -175,6 +213,8 @@ impl Files {
             ("confirmed irps", self.confirmed.len()),
             ("watchlist opens", self.opened.len()),
             ("cleanups awaiting an outcome", self.cleanups.len() + self.early_outcomes.len() + self.request_irps.len()),
+            ("entries past their cleanup", self.retiring.len()),
+            ("cleaned file objects", self.cleaned.len()),
         ]
     }
 
@@ -217,7 +257,7 @@ fn entry(nt: Option<String>, opener: Option<ProcessRef>, since: i64) -> FileEntr
         opener,
         written: false,
         delete_on_close: false,
-        cleaned: false,
+        cleaned: None,
         since,
         touched: 0,
         waiting: Vec::new(),
@@ -294,7 +334,10 @@ impl<L: Lookups> Pipeline<L> {
     pub(super) fn on_file_create(&mut self, h: &Header, c: FileCreate) {
         let nt = c.file_name.to_string_lossy();
         let opener = self.actor_sync(h, Class::File);
-        if self.files.map.contains_key(&c.file_object) {
+        let mapped = self.files.map.contains_key(&c.file_object);
+        // An address in the cleaned record is reused too: its Close never came (H1).
+        let cleaned = self.files.cleaned.remove(&c.file_object).is_some();
+        if mapped || cleaned {
             self.counters.file_object_replaced += 1;
         }
         self.seeding.changed(HandleKind::File, c.file_object, h.ts);
@@ -350,20 +393,27 @@ impl<L: Lookups> Pipeline<L> {
 
     /// A Write, or a truncation (the overwrite case, §5.1).
     pub(super) fn on_file_write(&mut self, h: &Header, fo: u64) {
-        match self.files.touch(fo) {
-            Some(e) => {
-                if e.cleaned {
-                    self.counters.writes_after_cleanup += 1;
-                }
-                e.written = true;
-            }
-            None => {
-                let mut e = entry(None, None, h.ts);
-                e.written = true;
-                self.files.insert(fo, e);
-                self.seeding.ask(HandleKind::File, fo, self.cfg.seed_on_miss);
-            }
+        let Some(e) = self.files.touch(fo) else {
+            self.on_unknown_write(h, fo);
+            return;
+        };
+        if e.cleaned.is_some() {
+            self.counters.writes_after_cleanup += 1;
+        }
+        e.written = true;
+    }
+
+    /// A write on a FileObject with no entry (§7.1).
+    fn on_unknown_write(&mut self, h: &Header, fo: u64) {
+        if self.files.cleaned.get(&fo).is_some() {
+            // A lazy-writer flush or a mapped page after the entry left (H1).
+            self.counters.writes_after_cleanup += 1;
+            return;
         }
+        let mut e = entry(None, None, h.ts);
+        e.written = true;
+        self.files.insert(fo, e);
+        self.seeding.ask(HandleKind::File, fo, self.cfg.seed_on_miss);
     }
 
     pub(super) fn on_file_set_info(&mut self, h: &Header, i: FileSetInfo) {
@@ -509,7 +559,17 @@ impl<L: Lookups> Pipeline<L> {
                 let ev = self.file_event(h.ts, actor, file, FileAction::Delete);
                 self.push_with(ev, vec![], expansion(expand, Expansion::file(None)));
             }
-            None => self.file_action_on(h.ts, fo, actor, FileAction::Delete, Fill::File, true),
+            None => {
+                let mapped = self.files.map.contains_key(&fo);
+                self.file_action_on(h.ts, fo, actor, FileAction::Delete, Fill::File, true);
+                // A handle the agent never saw opened: the entry made to wait for
+                // the seeder is past its Cleanup, and leaves like any other (H1).
+                if !mapped && let Some(e) = self.files.map.get_mut(&fo) {
+                    e.cleaned = Some(h.ts);
+                    let due = h.ts.saturating_add(self.files.outcome);
+                    self.files.retiring.push_back((due, h.ts, fo));
+                }
+            }
         }
     }
 
@@ -599,17 +659,26 @@ impl<L: Lookups> Pipeline<L> {
         if let Some((oh, info)) = early {
             self.on_cleanup_outcome(&oh, x.file_object, x.file_key, info);
         }
+        // Past its Cleanup an entry serves only the outcome: it leaves once
+        // the outcome's window has passed, or now without OperationEnds,
+        // whenever its Close comes (H1).
+        let fo = x.file_object;
+        if self.files.map.get(&fo).is_some_and(|e| e.cleaned == Some(h.ts)) && (op_end || !self.files.retire(fo, h.ts))
+        {
+            let due = h.ts.saturating_add(self.files.outcome);
+            self.files.retiring.push_back((due, h.ts, fo));
+        }
     }
 
     fn cleanup_handle(&mut self, h: &Header, x: &FileHandle) {
         let op_end = self.cfg.file_op_end;
         let Some(e) = self.files.touch(x.file_object) else { return };
-        let update = e.written && !e.cleaned;
+        let update = e.written && e.cleaned.is_none();
         // With OperationEnds the outcome reports the delete, possibly on another
         // handle's Cleanup: the FileKey remembers who asked.
         let delete = e.delete_on_close && !op_end;
         let asked = if op_end && e.delete_on_close { e.opener.clone() } else { None };
-        e.cleaned = true;
+        e.cleaned.get_or_insert(h.ts);
         let opener = e.opener.clone();
         if let Some(a) = asked {
             self.files.requests.insert(x.file_key, h.ts, Requester::Opener(a));
@@ -637,6 +706,7 @@ impl<L: Lookups> Pipeline<L> {
 
     pub(super) fn on_file_close(&mut self, h: &Header, x: FileHandle) {
         self.files.map.remove(&x.file_object);
+        self.files.cleaned.remove(&x.file_object);
         self.seeding.changed(HandleKind::File, x.file_object, h.ts);
     }
 
@@ -678,9 +748,25 @@ impl<L: Lookups> Pipeline<L> {
                 self.files.confirmed.insert(irp, ts);
             }
         }
+        // Entries past their Cleanup leave once the outcome's window has passed,
+        // as the Cleanup's record does (H1); one an event still waits on is
+        // looked at again a window later (each at most once a pass: a final
+        // flush runs at the end of time).
+        let outcome = self.files.outcome;
+        for _ in 0..self.files.retiring.len() {
+            let Some(&(due, at, fo)) = self.files.retiring.front() else { break };
+            if due >= stream {
+                break;
+            }
+            self.files.retiring.pop_front();
+            if !self.files.retire(fo, at) {
+                self.files.retiring.push_back((stream.saturating_add(outcome), at, fo));
+            }
+        }
+        self.files.cleaned.expire(stream.saturating_sub(self.ticks.of(CLEANED_KEEP)));
         self.files.failed.expire(stream.saturating_sub(window));
-        // A Cleanup's OperationEnd follows it at once: a window is plenty.
-        self.files.cleanups.expire(stream.saturating_sub(window));
+        // As long as the callback pairs a Cleanup with its outcome (§5.1).
+        self.files.cleanups.expire(stream.saturating_sub(outcome));
         // Kept longer: a slow request's OperationEnd, and a Cleanup that comes
         // late, are recognised for 40 windows (10 s), like late failures.
         self.files.request_irps.expire(stream.saturating_sub(window.saturating_mul(40)));
```

- [ ] **Step 3: The window, and `cleaned_fos` and `retiring_fos` in `sizes=`.**

```diff
--- a/crates/atlas-agent/src/pipeline/mod.rs
+++ b/crates/atlas-agent/src/pipeline/mod.rs
@@ -98,7 +98,7 @@ impl<L: Lookups> Pipeline<L> {
             completion: Completion::new(config.pending_cap),
             procs: ProcessCache::new(t(config.process_retention), config.process_cap),
             launches: Launches::default(),
-            files: Files::new(config.file_map_cap, t(config.confirm_window)),
+            files: Files::new(config.file_map_cap, t(config.confirm_window), t(crate::cleanup::OUTCOME_WINDOW)),
             keys: KeyMap::new(config.key_map_cap),
             reg: Registry::default(),
             net: Network::new(config.flow_cap, t(config.udp_idle)),
@@ -293,7 +293,7 @@ impl<L: Lookups> Pipeline<L> {
     pub fn sizes(&self) -> crate::health::Sizes {
         let (line, line_cap, pending, pending_cap) = self.completion.sizes();
         let (tcp, tcp_cap, flows, flows_cap) = self.net.sizes();
-        let [confirmed, failed, opened] = self.files.recent_sizes();
+        let [confirmed, failed, opened, cleaned, retiring] = self.files.recent_sizes();
         let n = |len: usize, cap: usize| (len as u64, cap as u64);
         let entries = [
             ("ordering", n(self.ordering.len(), self.ordering.capacity())),
@@ -305,6 +305,8 @@ impl<L: Lookups> Pipeline<L> {
             ("confirmed_irps", n(confirmed.0, confirmed.1)),
             ("failed_irps", n(failed.0, failed.1)),
             ("watch_opens", n(opened.0, opened.1)),
+            ("cleaned_fos", n(cleaned.0, cleaned.1)),
+            ("retiring_fos", n(retiring.0, retiring.1)),
             ("address_history", n(self.seeding.history_len(), self.seeding.history_capacity())),
             ("negative_cache", n(self.seeding.negative_cache_len(), self.seeding.negative_cache_capacity())),
             ("tcp", n(tcp, tcp_cap)),
```

- [ ] **Step 4: The tests** (Review Focus 1–9, 12). The name list of `sizes=`; two more seconds for the bookkeeping test; eleven new tests at the end of the file.

```diff
--- a/crates/atlas-agent/src/pipeline/tests.rs
+++ b/crates/atlas-agent/src/pipeline/tests.rs
@@ -625,6 +625,8 @@ fn the_sizes_name_each_bounded_structure() {
             "confirmed_irps",
             "failed_irps",
             "watch_opens",
+            "cleaned_fos",
+            "retiring_fos",
             "address_history",
             "negative_cache",
             "tcp",
@@ -1469,6 +1471,10 @@ fn bookkeeping_is_freed_when_no_reply_comes() {
     }
     let out = t.settle();
     assert!(!out.is_empty());
+    // An entry past its Cleanup leaves at the first pass after the events
+    // waiting on it for the seeder, a window later (plan 1b-4a-p3, H1).
+    let later = t.now / (FREQ / 1000) + 2_000;
+    t.at(later);
     no_bookkeeping_left(&t);
 }
 
@@ -1895,3 +1901,193 @@ fn a_request_that_succeeded_stands_against_a_later_failure_on_its_irp() {
     t.ev(50, 200, Some(key(20)), outcome(10, crate::cleanup::FILE_DELETED));
     assert_eq!(files(&t.settle()), [("delete", r"C:\kept.txt".into(), 100)]);
 }
+
+// ---- plan 1b-4a-p3: entries leave the file map after their Cleanup (H1) ----
+
+fn size_of(t: &T, name: &str) -> u64 {
+    t.p.sizes().into_iter().find(|s| s.0 == name).map(|s| s.1).expect(name)
+}
+
+/// A walk like `du`'s: thousands of files opened and cleaned up whose Close
+/// Windows sends late or not for hours. Only the open handle stays mapped.
+#[test]
+fn a_walk_without_closes_leaves_only_open_handles_in_the_map() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    t.ev(5, 100, Some(key(10)), create(1, 0xA, r"\Device\HarddiskVolume3\open.log", 0));
+    for i in 0..1_000u64 {
+        let f = format!(r"\Device\HarddiskVolume3\src\f{i}.js");
+        t.ev(6, 100, Some(key(10)), create(10 + i, 0x10_000 + i, &f, 0));
+        t.ev(7, 100, Some(key(10)), cleanup(10 + i, 0x10_000 + i, 0x90_000 + i));
+    }
+    t.at(2_000);
+    assert_eq!((size_of(&t, "file_map"), size_of(&t, "cleaned_fos")), (1, 1_000));
+    t.at(63_000); // the cleaned record keeps them 60 s
+    assert_eq!((size_of(&t, "file_map"), size_of(&t, "cleaned_fos")), (1, 0));
+    assert_eq!(t.p.counters().file_map_evictions, 0);
+}
+
+/// A lazy-writer flush after the entry left is absorbed, as before (§7.1);
+/// past the record's 60 s it is an unknown handle's write, today's rule.
+#[test]
+fn a_late_write_after_the_entry_left_is_absorbed() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    t.ev(5, 100, Some(key(10)), create(1, 0xA, r"\Device\HarddiskVolume3\a.txt", 0));
+    t.ev(6, 100, Some(key(10)), write(0xA));
+    t.ev(7, 4, None, cleanup(1, 0xA, 0x50));
+    t.at(2_000);
+    assert_eq!(size_of(&t, "file_map"), 0, "the entry left");
+    t.ev(10_000, 4, None, write(0xA)); // 10 s after its Cleanup
+    t.at(12_000);
+    assert_eq!((t.p.counters().writes_after_cleanup, size_of(&t, "file_map")), (1, 0));
+    assert_eq!(files(&t.out), [("update", r"C:\a.txt".into(), 100)], "one Update");
+    t.ev(70_000, 4, None, write(0xA)); // past the record
+    t.at(72_000);
+    assert_eq!((t.p.counters().writes_after_cleanup, size_of(&t, "file_map")), (1, 1));
+}
+
+/// The outcome that follows a Cleanup still finds the entry: a delete-on-close
+/// handle's Delete keeps its path and its opener (§5.1).
+#[test]
+fn the_cleanup_outcome_still_finds_the_entry() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    let f = r"\Device\HarddiskVolume3\tmp\d.txt";
+    t.ev(5, 100, Some(key(10)), create(1, 0xA, f, FileCreate::DELETE_ON_CLOSE));
+    t.ev(6, 4, None, cleanup(2, 0xA, 0x50));
+    t.ev(900, 4, None, outcome(2, crate::cleanup::FILE_DELETED)); // late, inside the 1 s window (review M1)
+    let out = t.settle();
+    assert_eq!(files(&out), [("delete", r"C:\tmp\d.txt".into(), 100)]);
+    assert_eq!(t.p.counters().unknown_file_object, 0);
+}
+
+/// Without OperationEnds no outcome follows: the entry leaves at its Cleanup.
+#[test]
+fn without_operation_ends_an_entry_leaves_at_its_cleanup() {
+    for (op_end, mapped) in [(true, 1), (false, 0)] {
+        let mut t = T::with(Config { file_op_end: op_end, ..Config::default() }, FakeLookups::default());
+        running(&mut t, 100, 10, EXPLORER);
+        t.ev(5, 100, Some(key(10)), create(1, 0xA, r"\Device\HarddiskVolume3\a.txt", 0));
+        t.ev(6, 100, Some(key(10)), cleanup(1, 0xA, 0x50));
+        t.ev(7, 100, Some(key(10)), create(2, 0xB, r"\Device\HarddiskVolume3\b.txt", 0));
+        t.at(757); // stream time 7 ms: the Cleanup's window is still open
+        assert_eq!(size_of(&t, "file_map") - 1, mapped, "op_end {op_end}");
+    }
+}
+
+/// A Create or a Close on a recorded FileObject takes it out of the record:
+/// the address belongs to another file object now.
+#[test]
+fn a_reused_or_closed_address_leaves_the_cleaned_record() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    t.ev(5, 100, Some(key(10)), create(1, 0xA, r"\Device\HarddiskVolume3\a.txt", 0));
+    t.ev(6, 100, Some(key(10)), cleanup(1, 0xA, 0x50));
+    t.ev(7, 100, Some(key(10)), create(2, 0xB, r"\Device\HarddiskVolume3\b.txt", 0));
+    t.ev(8, 100, Some(key(10)), cleanup(2, 0xB, 0x51));
+    t.at(2_000);
+    assert_eq!(size_of(&t, "cleaned_fos"), 2);
+    t.ev(1_300, 100, Some(key(10)), create(3, 0xA, r"\Device\HarddiskVolume3\c.txt", 0));
+    t.at(2_100);
+    assert_eq!(size_of(&t, "cleaned_fos"), 1, "a Create reuses 0xA");
+    t.ev(1_400, 4, None, RawEvent::FileClose(handle(0xB)));
+    t.at(2_200);
+    assert_eq!(size_of(&t, "cleaned_fos"), 0, "0xB closed at last");
+}
+
+/// An address closed and opened again inside the first Cleanup's window: the
+/// first retirement leaves the new, open handle alone.
+#[test]
+fn a_reused_address_inside_the_window_keeps_its_new_entry() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    t.ev(5, 100, Some(key(10)), create(1, 0xA, r"\Device\HarddiskVolume3\a.txt", 0));
+    t.ev(6, 100, Some(key(10)), cleanup(1, 0xA, 0x50));
+    t.ev(7, 100, Some(key(10)), RawEvent::FileClose(handle(0xA)));
+    t.ev(8, 100, Some(key(10)), create(2, 0xA, r"\Device\HarddiskVolume3\b.txt", 0));
+    t.ev(9, 100, Some(key(10)), write(0xA));
+    t.ev(2_000, 100, Some(key(10)), cleanup(3, 0xA, 0x51)); // long after the first window
+    assert_eq!(files(&t.settle()), [("update", r"C:\b.txt".into(), 100)]);
+}
+
+/// The same address cleaned up twice inside one window: the second entry
+/// waits for its own outcome's window, not the first's.
+#[test]
+fn a_second_cleanup_on_a_reused_address_waits_for_its_own_outcome() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    let f = r"\Device\HarddiskVolume3\tmp\d.txt";
+    t.ev(5, 100, Some(key(10)), create(1, 0xA, r"\Device\HarddiskVolume3\a.txt", 0));
+    t.ev(6, 100, Some(key(10)), cleanup(1, 0xA, 0x50));
+    t.ev(7, 100, Some(key(10)), RawEvent::FileClose(handle(0xA)));
+    t.ev(200, 100, Some(key(10)), create(2, 0xA, f, FileCreate::DELETE_ON_CLOSE));
+    t.ev(210, 4, None, cleanup(3, 0xA, 0x51));
+    // After the first Cleanup's window (6 + 1,000 ms), inside the second's.
+    t.ev(1_100, 4, None, outcome(3, crate::cleanup::FILE_DELETED));
+    assert_eq!(files(&t.settle()), [("delete", r"C:\tmp\d.txt".into(), 100)]);
+}
+
+/// An entry an event waits on for the start-up seeder stays past its
+/// Cleanup's window, so the answer still names the event; then it leaves.
+#[test]
+fn an_entry_waited_on_stays_until_answered() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    t.ev(5, 100, Some(key(10)), write(0xA)); // waits for the start-up pass
+    t.ev(6, 4, None, RawEvent::FileCleanup(handle(0xA)));
+    t.at(2_000); // stream time 1,250 ms: the Cleanup's window has passed
+    assert_eq!(size_of(&t, "file_map"), 1, "kept for the waiting Update");
+    t.reply(Reply::Snapshot(Snapshot {
+        kind: HandleKind::File,
+        taken: ms(1_500),
+        asked: vec![],
+        named: vec![Named { address: 0xA, owner_pid: 100, name: r"\Device\HarddiskVolume3\held.txt".into() }],
+        unnamable: vec![],
+    }));
+    t.at(3_000);
+    assert_eq!(files(&t.out), [("update", r"C:\held.txt".into(), 100)]);
+    t.at(4_000);
+    assert_eq!(size_of(&t, "file_map"), 0, "gone once answered");
+}
+
+/// An outcome exactly one window after its Cleanup is still the Cleanup's,
+/// as the callback pairs them, and still finds the entry (review nit).
+#[test]
+fn an_outcome_exactly_one_window_late_still_finds_its_entry() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    let f = r"\Device\HarddiskVolume3\tmp\d.txt";
+    t.ev(5, 100, Some(key(10)), create(1, 0xA, f, FileCreate::DELETE_ON_CLOSE));
+    t.ev(6, 4, None, cleanup(2, 0xA, 0x50));
+    t.ev(1_006, 4, None, outcome(2, crate::cleanup::FILE_DELETED));
+    assert_eq!(files(&t.settle()), [("delete", r"C:\tmp\d.txt".into(), 100)]);
+}
+
+/// A Create on an address in the cleaned record is a reuse after a Close that
+/// never came: `file_object_replaced` counts it, as it did while such entries
+/// stayed until their Close (review m1).
+#[test]
+fn a_create_on_a_cleaned_address_counts_a_replaced_file_object() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    t.ev(5, 100, Some(key(10)), create(1, 0xA, r"\Device\HarddiskVolume3\a.txt", 0));
+    t.ev(6, 100, Some(key(10)), cleanup(1, 0xA, 0x50));
+    t.ev(2_000, 100, Some(key(10)), create(2, 0xA, r"\Device\HarddiskVolume3\b.txt", 0));
+    t.settle();
+    assert_eq!(t.p.counters().file_object_replaced, 1);
+}
+
+/// An outcome on a handle the agent never saw opened makes an entry that waits
+/// for the seeder; it is past its Cleanup, and leaves like any other (review m3).
+#[test]
+fn an_entry_an_outcome_makes_leaves_too() {
+    let mut t = T::new(); // the start-up pass never answers: the Delete waits
+    running(&mut t, 100, 10, EXPLORER);
+    t.ev(6, 100, Some(key(10)), cleanup(2, 0xA, 0x50));
+    t.ev(7, 100, Some(key(10)), outcome(2, crate::cleanup::FILE_DELETED));
+    t.settle();
+    let later = t.now / (FREQ / 1000) + 2_000;
+    t.at(later);
+    assert_eq!(size_of(&t, "file_map"), 0);
+}
```

- [ ] **Step 5: Check and commit**

```powershell
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass. Workspace: 452 tests and ignores 19 (`atlas-agent` unit tests: 263).
```powershell
git add crates/atlas-agent
git commit -m "feat(agent): file map entries leave after their Cleanup (H1)"
```

### Task 2: No provisional entry that nothing can name (H2)

**Files:**
- Modify: `crates/atlas-agent/src/pipeline/file.rs`, `crates/atlas-agent/src/pipeline/tests.rs`

- [ ] **Step 1: `on_unknown_write`** (H2, F5). After the cleaned record is looked at, and after the start-up pass, with on-miss seeding off, a write on an unknown FileObject makes no entry.

```diff
--- a/crates/atlas-agent/src/pipeline/file.rs
+++ b/crates/atlas-agent/src/pipeline/file.rs
@@ -410,6 +410,13 @@ impl<L: Lookups> Pipeline<L> {
             self.counters.writes_after_cleanup += 1;
             return;
         }
+        // Once the start-up pass is done, and with on-miss seeding off,
+        // nothing can name a handle the agent never saw opened: its Update
+        // would be dropped, and an entry would hold memory until a Close
+        // that may never come (H2).
+        if !self.cfg.seed_on_miss && !self.seeding.startup_pending(HandleKind::File) {
+            return;
+        }
         let mut e = entry(None, None, h.ts);
         e.written = true;
         self.files.insert(fo, e);
```

- [ ] **Step 2: The tests** (Review Focus 4, 10). The start-up pass test's second half now expects no entry and no count; a test with on-miss seeding on; a late write after the pass (review m2).

```diff
--- a/crates/atlas-agent/src/pipeline/tests.rs
+++ b/crates/atlas-agent/src/pipeline/tests.rs
@@ -1607,11 +1607,14 @@ fn without_on_miss_seeding_only_the_start_up_pass_is_waited_for() {
     }));
     t.at(1_800);
     assert_eq!(files(&t.out), [("update", r"C:\held.txt".into(), 100)]);
-    // After the pass, an unknown handle is dropped at once.
+    // After the pass nothing can name an unknown handle: its write makes no
+    // entry, so its Cleanup has nothing to report (plan 1b-4a-p3, H2).
     t.ev(1_900, 100, Some(key(10)), write(0xB));
+    t.at(2_000);
+    assert!(!t.p.files.map.contains_key(&0xB), "no provisional entry");
     t.ev(1_901, 4, None, RawEvent::FileCleanup(handle(0xB)));
     t.at(2_700);
-    assert_eq!((t.p.counters().unknown_file_object, t.p.completion.pending_len()), (1, 0));
+    assert_eq!((t.p.counters().unknown_file_object, t.p.completion.pending_len()), (0, 0));
 }
 
 #[test]
@@ -2091,3 +2094,49 @@ fn an_entry_an_outcome_makes_leaves_too() {
     t.at(later);
     assert_eq!(size_of(&t, "file_map"), 0);
 }
+
+/// With on-miss seeding on, a write on an unknown handle after the start-up
+/// pass still makes an entry and asks the seeder about it (H2 leaves it).
+#[test]
+fn with_on_miss_seeding_an_unknown_write_still_asks() {
+    let mut t = T::seeding_on_miss();
+    running(&mut t, 100, 10, EXPLORER);
+    t.at(1_000);
+    t.reply(Reply::Snapshot(Snapshot {
+        kind: HandleKind::File,
+        taken: ms(1_000),
+        asked: vec![],
+        named: vec![],
+        unnamable: vec![],
+    }));
+    t.at(2_000); // the start-up pass is applied
+    t.requests();
+    t.ev(1_900, 100, Some(key(10)), write(0xB));
+    t.at(2_700);
+    assert_eq!(seed_asks(&mut t, HandleKind::File), [vec![0xB]]);
+}
+
+/// After the start-up pass a late write on a cleaned FileObject is still
+/// absorbed: the cleaned record is looked at before H2's rule (review m2).
+#[test]
+fn after_the_start_up_pass_a_late_write_is_still_absorbed() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    t.at(1_000);
+    t.reply(Reply::Snapshot(Snapshot {
+        kind: HandleKind::File,
+        taken: ms(1_000),
+        asked: vec![],
+        named: vec![],
+        unnamable: vec![],
+    }));
+    t.at(2_000); // the start-up pass is applied
+    t.ev(1_300, 100, Some(key(10)), create(1, 0xA, r"\Device\HarddiskVolume3\a.txt", 0));
+    t.ev(1_301, 100, Some(key(10)), write(0xA));
+    t.ev(1_302, 4, None, cleanup(1, 0xA, 0x50));
+    t.at(4_000);
+    assert_eq!(size_of(&t, "file_map"), 0, "the entry left");
+    t.ev(10_000, 4, None, write(0xA));
+    t.at(12_000);
+    assert_eq!(t.p.counters().writes_after_cleanup, 1);
+}
```

- [ ] **Step 3: Check and commit**

```powershell
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass. Workspace: 454 tests and ignores 19 (`atlas-agent` unit tests: 265).
```powershell
git add crates/atlas-agent
git commit -m "feat(agent): no provisional file entry once nothing can name it (H2)"
```

### Task 3: An 8.3 expansion fills only the entry it was asked for (review)

**Files:**
- Modify: `crates/atlas-agent/src/pipeline/expand.rs`, `crates/atlas-agent/src/pipeline/tests.rs`

- [ ] **Step 1: `Expands`.** Each slot keeps the NT path it asked about when it fills a map entry; `on_expanded` writes the long path into the entry only if the entry still has that path (a Close and a Create at the same address, or a rename, since the question).

```diff
--- a/crates/atlas-agent/src/pipeline/expand.rs
+++ b/crates/atlas-agent/src/pipeline/expand.rs
@@ -50,7 +50,9 @@ impl Expansion {
 
 #[derive(Default)]
 pub struct Expands {
-    by_slot: FastMap<(PendingId, u8), Expansion>,
+    /// Each slot's expansion, with the NT path it asked about when it fills a
+    /// map entry: the FileObject may hold another file by the time it comes.
+    by_slot: FastMap<(PendingId, u8), (Expansion, Option<String>)>,
     /// Per event: slots still unanswered, and how many it asked for.
     outstanding: FastMap<PendingId, (u8, u8)>,
 }
@@ -106,14 +108,15 @@ impl<L: Lookups> Pipeline<L> {
         }
         for (slot, (nt_path, x)) in expansions.into_iter().enumerate() {
             let slot = slot as u8;
-            self.expands.by_slot.insert((id, slot), x);
+            let asked = x.fo.map(|_| nt_path.clone());
+            self.expands.by_slot.insert((id, slot), (x, asked));
             self.requests.push(Request::Expand { id, slot, nt_path });
         }
         Some(id)
     }
 
     pub(super) fn on_expanded(&mut self, id: PendingId, slot: u8, long: Option<String>) {
-        let Some(x) = self.expands.by_slot.remove(&(id, slot)) else { return };
+        let Some((x, asked)) = self.expands.by_slot.remove(&(id, slot)) else { return };
         let left = match self.expands.outstanding.get_mut(&id) {
             Some((n, _)) => {
                 *n = n.saturating_sub(1);
@@ -137,8 +140,11 @@ impl<L: Lookups> Pipeline<L> {
         if let Some(l) = long {
             let file = self.file_obj(&l);
             self.completion.update(id, |e| set_path(e, x.slot, file));
+            // Only into the entry it was asked for: the address may have been
+            // closed and reused, or the file renamed, since (plan 1b-4a-p3 review).
             if let Some(fo) = x.fo
                 && let Some(e) = self.files.map.get_mut(&fo)
+                && e.nt == asked
             {
                 e.expanded = Some(l.clone());
             }
```

- [ ] **Step 2: The test** (Review Focus 11).

```diff
--- a/crates/atlas-agent/src/pipeline/tests.rs
+++ b/crates/atlas-agent/src/pipeline/tests.rs
@@ -2140,3 +2140,31 @@ fn after_the_start_up_pass_a_late_write_is_still_absorbed() {
     t.at(12_000);
     assert_eq!(t.p.counters().writes_after_cleanup, 1);
 }
+
+/// A late expansion does not name the next file at the same address: A's long
+/// path stays A's when B reuses its FileObject before the answer comes
+/// (plan 1b-4a-p3 review).
+#[test]
+fn a_late_expansion_does_not_name_the_next_file_at_its_address() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    t.ev(5, 100, Some(key(10)), create(1, 0xA, r"\Device\HarddiskVolume3\PROGRA~1\a.txt", 0));
+    t.ev(6, 100, Some(key(10)), write(0xA));
+    t.ev(7, 100, Some(key(10)), cleanup(1, 0xA, 0x50));
+    t.ev(8, 100, Some(key(10)), RawEvent::FileClose(handle(0xA)));
+    t.ev(10, 100, Some(key(10)), create(2, 0xA, r"\Device\HarddiskVolume3\b.txt", 0));
+    t.ev(11, 100, Some(key(10)), write(0xA));
+    t.at(1_000);
+    for r in t.requests() {
+        if let Request::Expand { id, slot, .. } = r {
+            t.reply(Reply::Expanded {
+                id,
+                slot,
+                long_path: Some(r"\Device\HarddiskVolume3\Program Files\a.txt".into()),
+            });
+        }
+    }
+    t.ev(1_100, 100, Some(key(10)), cleanup(3, 0xA, 0x51));
+    let out = t.settle();
+    assert_eq!(files(&out), [("update", r"C:\Program Files\a.txt".into(), 100), ("update", r"C:\b.txt".into(), 100)]);
+}
```

- [ ] **Step 3: Check and commit**

```powershell
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass. Workspace: 455 tests and ignores 19 (`atlas-agent` unit tests: 266).
```powershell
git add crates/atlas-agent
git commit -m "fix(agent): an 8.3 expansion fills only the file map entry it was asked for"
```

### Task 4: The hour run (the user)

The re-measure (H1.5): an hour of normal use on the build, judged against §13 from its Sensor Health reports and the `sizes=` lines. It needs elevation, so the user runs it. The scripts are git-ignored under `spikes\`; they write the power source and plan at the start and the end, and the summary gives each report's CPU spread.

- [ ] **Step 1:** `cargo build --release -p atlas-agent` on `feat/1b-4a-p3`. Before handing it over, Claude says that it runs on the user's main machine and what it changes there (`C:\ProgramData\Atlas`, the ETW sessions, 150–235 MB of buffer an hour), and asks for AC power, the usual plan, no clicks in the console, and no folder walk in another session unless on purpose.
- [ ] **Step 2: The user runs, elevated:**

```powershell
& C:\Users\jakef\Desktop\atlas-edr\spikes\run-1b4a-hour.ps1
```
After about an hour, Ctrl+C once. Then, in the same window (the log folder is admin-only):
```powershell
& C:\Users\jakef\Desktop\atlas-edr\spikes\copy-1b4a-hour-log.ps1
```
- [ ] **Step 3:** Claude reads `spikes\results\1b4a-hour\`: the summary (CPU, working set, losses, both unpaired-Cleanup parts) and the `sizes=` lines per minute (`file_map`, `cleaned_fos`, `retiring_fos`, and any structure that grows). Expected: nothing lost; the working set within 150 MB; the file map near its open-handle count. The figures go into Task 5 as F10.

### Task 5: Documentation

- [ ] **Step 1: The spec.** In `docs/specs/2026-10-01-etw-sensor-design.md`: the status line (its date is the build's merge day: change `2026-11-02` if it differs); §7.1 (H1, H2, H3, m1, m3); §9.3 (`cleaned_fos`, `retiring_fos`); §13 (the hour run's figures; Task 4's figures added); §15.3 (a "Plan 1b-4a-p3" addendum, F1–F9, and Task 4's run as F10); §17 ("Plan 1b-4a-p3 clarifications" and "review fixes"). `docs/architecture-overview.md` (roadmap row 1 and a decision-log row for the build) is a second commit, `docs(1): plan 1b-4a-p3 built: roadmap and decision log`, made once Task 4's figures are in.

```diff
--- a/docs/specs/2026-10-01-etw-sensor-design.md
+++ b/docs/specs/2026-10-01-etw-sensor-design.md
@@ -1,6 +1,6 @@
 # Sub-project 1 — ETW Sensor Design (Agent Core)
 
-**Status:** Approved (2026-10-02), revision 3. Revision 2 was approved on 2026-10-01; revision 3 folds the spike results (§15.3) and the decisions made during the spikes into the design sections (§17). Revision 1 had an independent review; its findings are folded in. Implementation: **plan 1a** (spikes S1–S10, §15.2) is done; **plan 1b** (the build) comes in six parts, each reviewed and approved before it runs (decision log, 2026-10-02; plan 1b-3 was split in two on 2026-10-05, and a plan 1b-3c added on 2026-10-06); plan 1b-1 (schema additions + `atlas-buffer`) is done (2026-10-04); plan 1b-2 (`atlas-etw`) is done (2026-10-05); plan 1b-3a (the `atlas-agent` pipeline core) is done (2026-10-05); plan 1b-3b (the Windows services behind it) is done (2026-10-06); plan 1b-3c (the driver, the agent-level live test, and deletes from the Cleanup outcome) is done (2026-10-06); plan 1b-4 is split in three (decision log, 2026-10-12): plan 1b-4a (a runnable agent: `run`, `dump`, `agent.toml`, the buffer as the sink, Sensor Health reports) is done (2026-10-13); the profiling step that comes before 1b-4b (decision log, 2026-10-12, D6) produced plan 1b-4a-p1 (the hash cache no longer forgets a file on a SetAttributes; the events each session delivers and each thread's CPU are measured), done (2026-10-15), and plan 1b-4a-p2 (on-miss seeding off; the pipeline's measured fixes; cache invalidations that wake no thread; Launch images hashed first; each bounded structure's size in the log; memory that follows contents; no power throttling), done (2026-10-27); the hour run that ends the profiling step is next, then plan 1b-4b (blinding detection), then 1b-4c (the service and acceptance). Brainstorm handoff: [etw-sensor-brainstorm-notes](2026-10-01-etw-sensor-brainstorm-notes.md).
+**Status:** Approved (2026-10-02), revision 3. Revision 2 was approved on 2026-10-01; revision 3 folds the spike results (§15.3) and the decisions made during the spikes into the design sections (§17). Revision 1 had an independent review; its findings are folded in. Implementation: **plan 1a** (spikes S1–S10, §15.2) is done; **plan 1b** (the build) comes in six parts, each reviewed and approved before it runs (decision log, 2026-10-02; plan 1b-3 was split in two on 2026-10-05, and a plan 1b-3c added on 2026-10-06); plan 1b-1 (schema additions + `atlas-buffer`) is done (2026-10-04); plan 1b-2 (`atlas-etw`) is done (2026-10-05); plan 1b-3a (the `atlas-agent` pipeline core) is done (2026-10-05); plan 1b-3b (the Windows services behind it) is done (2026-10-06); plan 1b-3c (the driver, the agent-level live test, and deletes from the Cleanup outcome) is done (2026-10-06); plan 1b-4 is split in three (decision log, 2026-10-12): plan 1b-4a (a runnable agent: `run`, `dump`, `agent.toml`, the buffer as the sink, Sensor Health reports) is done (2026-10-13); the profiling step that comes before 1b-4b (decision log, 2026-10-12, D6) produced plan 1b-4a-p1 (the hash cache no longer forgets a file on a SetAttributes; the events each session delivers and each thread's CPU are measured), done (2026-10-15), and plan 1b-4a-p2 (on-miss seeding off; the pipeline's measured fixes; cache invalidations that wake no thread; Launch images hashed first; each bounded structure's size in the log; memory that follows contents; no power throttling), done (2026-10-27); the hour run (2026-10-31) met the CPU budget but not the working set, and produced plan 1b-4a-p3 (file map entries leave after their Cleanup; no provisional entry that nothing can name; one 1 s outcome window), done (2026-11-02); the hour run again ends the profiling step, then plan 1b-4b (blinding detection), then 1b-4c (the service and acceptance). Brainstorm handoff: [etw-sensor-brainstorm-notes](2026-10-01-etw-sensor-brainstorm-notes.md).
 **Depends on:** 0a (event schema: domain types, `process_uid`, validating `TryFrom`), 0b (CI, nightly fuzz workflow).
 **Depended on by:** sub-project 2 (adds enrollment + gRPC on top of the buffer's read API), sub-project 3 (fills the agent-side detection hook), sub-project 6 (the driver becomes a second sensor feeding the same pipeline).
 
@@ -319,7 +319,7 @@ Querying `ProcessTelemetryIdInformation` for the new PID at Launch is **not** a
 - `written` and `delete_on_close` flags;
 - its source: an ETW Create, a seeder answer, or provisional.
 
-**Entries from Create.** Every Create (12) adds an entry, and `delete_on_close` comes from `CreateOptions` bit `0x1000`. A failed Create is removed when its failure OperationEnd arrives within the confirm window (§5.5). A Create on a `FileObject` already in the map replaces the entry (address reuse after a lost Close) and counts `file_object_replaced`.
+**Entries from Create.** Every Create (12) adds an entry, and `delete_on_close` comes from `CreateOptions` bit `0x1000`. A failed Create is removed when its failure OperationEnd arrives within the confirm window (§5.5). A Create on a `FileObject` already in the map replaces the entry (address reuse after a lost Close) and counts `file_object_replaced`; so does a Create on a `FileObject` in the cleaned record (below), whose Close never came (plan 1b-4a-p3).
 
 **Writes.** The first Write (16), or a SetInformation 17 with `InfoClass` 19 (a truncation, the overwrite case), marks the entry written. Further writes are absorbed.
 
@@ -331,17 +331,20 @@ Querying `ProcessTelemetryIdInformation` for the new PID at Launch is **not** a
 - The actor of an Update is the entry's opener, never the Cleanup's header. The cache manager, or the agent's own duplicate during seeding (§7.4), can be the closing context.
 - A File Update or Delete from a provisional entry waits in [5] with a "drop at deadline" seeder reason (§3.2): if the entry is still unresolved at the deadline, the event is dropped and `unknown_file_object` counts it. This is a deliberate carve-out from E11: a file event without a path, and usually without a trustworthy actor, has no detection value.
 
-**Close and after.**
-- Close (14) removes the entry.
-- Writes that arrive after Cleanup (lazy-writer flushes, memory-mapped paging writes) are absorbed into the Update already emitted and counted as `writes_after_cleanup`.
+**After the Cleanup** (plan 1b-4a-p3, H1).
+- The entry leaves once the Cleanup's **outcome window** has passed: 1 s, the window in which the callback pairs a Cleanup with its outcome (§5.1) and the pipeline keeps the Cleanup's record (`cleanup::OUTCOME_WINDOW`, H3). With `file.op_end` off it leaves at the Cleanup itself. Past its Cleanup an entry serves only that outcome (the delete's path and requester, the POSIX-style check) and late writes. Windows may send the Close (14) long after the Cleanup, or not for hours, for a file it keeps cached or mapped: a `du` walk once left 204,526 entries past their Cleanup, and the working set at 281 MB (§15.3, plan 1b-4a-p3 F3).
+- An entry that an event waits on for the seeder stays until the event is answered or leaves the completion stage, and leaves at the next pass after that. An entry made for a Cleanup's outcome on a handle the agent never saw opened (to wait for the seeder) is past its Cleanup too, and leaves the same way.
+- A FileObject whose entry left is kept in a **cleaned record**, FileObject values only, for 60 s (at most 65,536, oldest out first). A Create or a Close on it takes it out: the address belongs to another file object then.
+- Close (14) removes the entry if it is still there.
+- Writes that arrive after Cleanup (lazy-writer flushes, memory-mapped paging writes) are absorbed into the Update already emitted and counted as `writes_after_cleanup`, whether the entry is still there or its FileObject is in the cleaned record. Past the record's 60 s such a write is an unknown handle's (on the host, 99.6% of late writes came within 60 s). Any other event on a FileObject whose entry left (a SetAttributes, a rename) is an unknown handle's; on the host none came (F4).
 - A confirmed rename (§5.5) changes the entry's path to the new name, so a later Update carries the name the file has now.
 
 **Unknown handles, refined** (plan 1b-3a):
 - A SetAttributes on a handle that is never named is dropped and counted (`unknown_file_object`), like Update and Delete.
 - A RenamePath on a handle whose name is unknown is emitted at once, with an empty source. It does not wait for the seeder, because a snapshot read after the rename could only give the new name. The handle takes the new name once the rename stands.
-- With on-miss seeding off (§11.2), such events wait only for the start-up pass, while it is outstanding.
+- With on-miss seeding off (§11.2), such events wait only for the start-up pass, while it is outstanding. After the pass a write or truncation on an unknown handle makes no provisional entry: nothing can name it, so its Update would be dropped, and on the host such entries got no Cleanup or Close (plan 1b-4a-p3, H2). `unknown_file_object` then does not count that Update. A write on a FileObject in the cleaned record is absorbed first, as above.
 
-**Bounds.** The map is bounded (cap + counter; past the cap, the least recently used eighth goes at once, plan 1b-3a). Seeded entries do not know `delete_on_close`, but the Cleanup outcome still reports the delete of a handle opened with that flag before the agent started (plan 1b-3c); its actor is then the process whose Cleanup removed the name, the handle's owner.
+**Bounds.** The map is bounded (cap + counter; past the cap, the least recently used eighth goes at once, plan 1b-3a). Since plan 1b-4a-p3 it holds open handles, cleaned ones for one outcome window, provisional entries (during the start-up pass, and always with on-miss seeding on) and failed Creates whose failure came after the confirm window, so on the host the cap is a safety net. The cleaned record is bounded on its own (about 4 MB at its cap). Seeded entries do not know `delete_on_close`, but the Cleanup outcome still reports the delete of a handle opened with that flag before the agent started (plan 1b-3c); its actor is then the process whose Cleanup removed the name, the handle's owner.
 
 ### 7.2 Watchlist
 
@@ -509,7 +512,7 @@ Emitted every 60 s (and immediately on a non-zero change in a loss counter):
 - **Snapshots and differences:** the health thread [7] reads every cumulative counter into a snapshot: the pipeline's (through the driver's control channel, waiting at most 2 s), the intake's and the services' atomics, the callbacks' panics and delivered events (counters the agent owns and hands to each consumer it starts, so a restarted session counts on into them; plan 1b-4a-p1, review M1), both sessions' losses (`ControlTraceW` query), the sink's drops, and the process's CPU time and working set. A report holds the differences between two snapshots. The sessions' counters are 32-bit, so their differences wrap. A source that could not be read is absent from the report, and the next report covers its interval too.
 - **When:** every 60 s; at once when a loss counter rose (checked every 5 s); and a final report at the clean stop, taken after the sessions stopped, so everything counted during the run is reported. The intervals chain: each starts where the last ended, the first at the anchor taken at start. A report's time is the system clock at generation (`GetSystemTimePreciseAsFileTime`, the clock the anchor reads, §3.3); its id is a UUIDv7. If the pipeline thread died, the final report is still sent, without the pipeline's counters (they died with it).
 - **The buffer's figures:** [6] fills in the `buffer` group, `loss.buffer_backlog_drops`, `housekeeping.retention_evictions` and the gap as it appends the report, each since the report before; so the drops of the events queued ahead of a report, the final drain's included, are in it (review finding 3). The health thread's own reading of the drops only decides an early report; the first report of a run carries what recovery found at open. A report the writer's backlog cannot take is held (up to 16) and written before anything else once there is room.
-- **The diagnostic log** gets one line per report with its main figures, including the two parts of `quality.file_cleanup_unpaired` (another operation came first on the `Irp`; the outcome came after the window), which the schema sums, and (plan 1b-4a-p1) the CPU each agent thread used since the last listing: each thread's difference (a thread is its id and creation time), summed by thread name, most first, with the listings' own interval. Threads are listed with a snapshot of every thread in the system (about 45 ms on the host), so at most once a report period, for a report of either kind; a report without them leaves them to the next listing. The final report's listing is taken just before the sensor stops, while its threads still run. Plan 1b-4a-p2 (R6) adds each bounded structure's entries and capacity, `sizes="name=entries/capacity …"`: the ordering stage, the completion stage's line and its pending events, the process cache, the file and key maps, the two `Irp` tables, the watchlist opens, the seeding address history, the negative cache, the TCP and UDP tables, the early key map, the hash and expansion caches, and the kernel, user and sink queues.
+- **The diagnostic log** gets one line per report with its main figures, including the two parts of `quality.file_cleanup_unpaired` (another operation came first on the `Irp`; the outcome came after the window), which the schema sums, and (plan 1b-4a-p1) the CPU each agent thread used since the last listing: each thread's difference (a thread is its id and creation time), summed by thread name, most first, with the listings' own interval. Threads are listed with a snapshot of every thread in the system (about 45 ms on the host), so at most once a report period, for a report of either kind; a report without them leaves them to the next listing. The final report's listing is taken just before the sensor stops, while its threads still run. Plan 1b-4a-p2 (R6) adds each bounded structure's entries and capacity, `sizes="name=entries/capacity …"`: the ordering stage, the completion stage's line and its pending events, the process cache, the file and key maps, the two `Irp` tables, the watchlist opens, the cleaned FileObjects and the entries past their Cleanup (`cleaned_fos`, `retiring_fos`, plan 1b-4a-p3), the seeding address history, the negative cache, the TCP and UDP tables, the early key map, the hash and expansion caches, and the kernel, user and sink queues.
 - **Not measured yet** (absent): `quality.buffer_invalid_records` and `buffer.corrupt_segments` (the agent never reads its own buffer back; `dump` counts both), and `housekeeping.seeder_negative_cache_size` (the negative cache's size is in the diagnostic log's `sizes=` since plan 1b-4a-p2, not in the schema).
 
 ## 10. Schema Additions (`atlas.events.v1`, additive)
@@ -660,6 +663,8 @@ Late arrivals, enrichment misses and DNS rate-limit drops are reported, not gate
 
 **Plan 1b-4a-p2** (D6, 2026-10-15 to 2026-10-27): on-miss seeding off (§13's first fallback), the pipeline's measured fixes, invalidations that wake no thread, Launch images first, memory that follows contents and no power throttling (§15.3). The old and the new build, back to back on the host under the same trace script, both on AC power, the Silent power plan and Windows' high quality of service: under the synthetic load, 11.4% → 10.5% of one core (without the hash workers 5.3% → 3.4%; the seeder 1.0% → 0), with 0 and 1 enrichment misses; the working set 109–130 MB → 40–66 MB, under budget. Five minutes of normal use took 2.2% of one core, most of it first-time hashes of DLLs the agent had not seen since it started; quiet minutes 1.4%. The hash workers vary from run to run (6.1% and 6.1% for the old build, 5.3% and 7.2% for the new under the same load). The hour run judges the workday figures; anything still over budget comes back as a decision. On the merged code (Task 14, F11), with the two runs' quiet and synthetic phases at the same event rates: quiet 3.1% → 1.1% of one core; the synthetic load 13.6% → 10.1% (without the hash workers 6.1% → 3.0%), with 34 and 3 enrichment misses and 3 and 0 Launches without a hash; the working set 105–132 MB → 43–58 MB.
 
+**The hour run** (2026-10-31, 74.7 minutes of normal use on the merged plan 1b-4a-p2 code, AC power, the Silent plan; §15.3): CPU averaged **1.59% of one core**, within the workday budget (per-minute p10 / p50 / p90 / max 0.67 / 0.98 / 3.35 / 7.87%); nothing lost. The working set reached **281 MB**, over budget: the file map kept FileObjects past their Cleanup whose Close Windows sent late or not at all (a `du` walk running in the background), up to its cap. **Plan 1b-4a-p3** (H1, H2, H3): entries leave the map after their Cleanup's outcome window, and no provisional entry is made that nothing can name. On the host, with Git's `du` walking the same folder: 30 minutes on a probe build of the old code left 231k entries and 312 MB (F3); 10 minutes on plan 1b-4a-p3's code, about 4.5k–4.9k entries, flat, and at most 65 MB (F9). The hour run again judges the working set.
+
 **Fallbacks if over budget** (applied in order, each one a decision-log entry): on-miss seeding off (start-up seeding stays; applied by plan 1b-4a-p2); then `file.op_end` off (Create, Delete and Rename are emitted without failure confirmation, counted); then UDP off; then File Update and SetAttributes off (drops the `FILEIO` and `WRITE` keywords, the largest kernel-side cost; Create/Delete/Rename/Open remain); then registry value reads off (`data_unavailable` on every Value Set); then per-class disable in config as a last resort.
 
 ## 14. Definition of Done
@@ -806,6 +811,17 @@ Plan 1a ([2026-10-02-etw-sensor-spikes-plan](../plans/2026-10-02-etw-sensor-spik
 - **F11, the merged code** (Task 14's script on `main`, 2026-10-27; both at high QoS, on AC with Silent; the quiet phases at 2,073 and 2,071 events/s, the synthetic at 17,509 and 17,295): quiet 3.07% → 1.06% of one core (the seeder 1.6% → 0, the pipeline 0.71% → 0.25%); the synthetic load 13.6% → 10.1% (hash workers 7.5% and 7.1%, pipeline 3.2% → 2.1%, seeder 1.1% → 0), with 34 and 3 enrichment misses and 3 and 0 Launches without a hash; the working set 105–132 MB → 43–58 MB, the `sizes=` line's queues empty between reports. The new build's normal phase was much lighter (108 module loads against 962), so it does not compare.
 - **F10, SetAttributes kept** (R5): Windows' SMB server (`srv2.sys`) works in the System process, so a remote client's changes on this machine's shares also show as PID 4; a filter on PID 4 would hide their timestomping.
 
+**Plan 1b-4a-p3 (2026-10-31 to 2026-11-02): the file map's memory** (the host, build 26200; the p3 notes have the details):
+- **F1, the hour run:** 74.7 minutes of normal use: CPU 1.59% of one core, nothing lost, both unpaired-Cleanup parts 0, 109 enrichment misses in one busy minute; the working set 51 → 281 MB.
+- **F2, where the memory went:** in the `sizes=` lines only the file map grew, 3,000–5,000 entries a minute, quiet minutes included, to its 262,144 cap; each jump in the working set was one doubling of its table (229k → 458k slots: +67 MB).
+- **F3, what it held** (a probe build, never merged; 30 minutes with `du` running throughout): 204,526 of 231,340 entries were files Git Bash's `du` had looked at, 99.99% past their Cleanup with no Close; the working set reached 312 MB. Windows sends such a Close late or not for a long time (12,000 came at once in one minute); the agent's own hash reads linger the same way. Close (14) reaches the pipeline: nothing is lost.
+- **F4, what reaches a FileObject past its Cleanup** (the synthetic load and five minutes of normal use): no SetAttributes, Rename or SetDelete (41,102 SetAttributes, the System process's bursts included, hit open handles); the Cleanup's outcomes, all within 100 ms; 2,246 late writes, 99.6% within 60 s; Closes, 98% within 100 ms.
+- **F5, provisional entries:** a write on a FileObject never seen opened made one, about 400–600 a minute (17.5k in 30 minutes), none ever cleaned up or closed; with on-miss seeding off nothing names them after the start-up pass.
+- **F6, H1 on the host** (`run-p3-host.ps1`: the agent, then `du -s` over the same folder for 10 minutes, then 3 quiet minutes): the file map 4.6k → 9.6k entries and the working set at most 70 MB; the remaining growth was F5's.
+- **F7, H1 and H2 on the host** (the same script): the file map 4,485 → 4,511 entries, at most 71 MB, `cleaned_fos` 2.3k–4.9k during the walk; nothing lost.
+- **F8, tooling:** Windows PowerShell 5.1's `Start-Process` does not quote arguments (the first host try walked nothing); the agent's log rolls over at UTC midnight; a revert whose test filter matches no test now fails the check.
+- **F9, the reviewed code on the host** (the same script, 2026-11-02, AC power, the Turbo plan): the file map 4,512–4,873 entries through the 13 minutes, `cleaned_fos` 4.0k–8.4k during the walk and under 50 after it, `retiring_fos` at most 1,177, the working set 43 → 65 MB at most; nothing lost, both unpaired-Cleanup parts 0, no late outcome.
+
 ## 16. Known Limitations
 
 - **No boot-time coverage:** events before the agent starts (early boot, or while the service is stopped) are not captured. An AutoLogger session is sub-project 8.
@@ -980,3 +996,14 @@ Plan 1a ([2026-10-02-etw-sensor-spikes-plan](../plans/2026-10-02-etw-sensor-spik
 3. the set of watchlist Opens, keyed by path, keeps SipHash;
 4. the `sizes=` line gives the kernel, user and sink queues too;
 5. a test that could not catch its bug (the expander list's overflow) now can.
+
+**Plan 1b-4a-p3 clarifications (2026-10-31).** The hour run's working set (decision log, 2026-10-31). Applied to §7.1, §9.3, §13 and §15.3:
+1. a file map entry leaves one outcome window after its Cleanup (at the Cleanup without OperationEnds; later while an event waits on it for the seeder), not at its Close; a 60 s record of cleaned FileObjects absorbs late writes, and a Create on a recorded FileObject counts `file_object_replaced` (H1);
+2. after the start-up pass, with on-miss seeding off, a write on an unknown handle makes no provisional entry, and `unknown_file_object` does not count its Update (H2);
+3. `sizes=` gives the cleaned FileObjects and the entries past their Cleanup.
+
+**Plan 1b-4a-p3 review fixes (2026-10-31).** An independent review of the plan and its code:
+1. **one outcome window** (H3): the callback pairs a Cleanup with its outcome within 1 s (§5.1), but the pipeline kept the Cleanup's record for one 250 ms confirm window, so a Delete whose outcome came 250 ms–1 s after its Cleanup was lost and not counted (on `main` before this plan). One constant, `cleanup::OUTCOME_WINDOW` (1 s), now sets the callback's pairing, the pipeline's record and the retire delay;
+2. an entry made for a Cleanup's outcome on a handle the agent never saw opened leaves like any other;
+3. an outcome exactly one window after its Cleanup still finds its entry;
+4. a late 8.3 expansion fills a map entry only if the entry still has the NT path it asked about: after a Close and a Create at the same address, the next file's Update carried the first file's path (on `main` since plan 1b-3a).
```

- [ ] **Step 2: Commit**

```powershell
git add docs
git commit -m "docs(1): plan 1b-4a-p3 clarifications and findings"
```

### Task 6: PR and merge

- [ ] **Step 1:** Push `feat/1b-4a-p3` and open the PR. The body summarises H1, H2 and H3 with F7 and F9, Task 3, Task 4's figures, the test counts, and ends with the attribution line.
- [ ] **Step 2:** CI runs `rust-linux`, `rust-windows`, `etw-live`, `agent-live`, `proto`, `powershell` and `audit`. If a test fails only on the runner, fix it with a test, never by skipping.
- [ ] **Step 3:** When every check on the PR's current head is green, merge. Then update the decision log's build row if anything changed on the way, and delete the remote `scratch/p3` and `scratch/p3-review` branches. The 1b-4b brainstorm follows.

## Review Log

### Independent review (2026-10-31, of the draft on `scratch/p3` at `168885a`)

An independent agent read the draft against the spec, the p3 notes and the code. It built the code in its own target directory and ran the unelevated tests (`atlas-agent` 261 passed, the replay tests passed, fmt clean), and probed suspected bugs in throwaway worktrees with tests of its own. It found the new retirement code sound: no event that H1 or H2 loses, reports twice, or lets grow without bound inside the window they rely on. It found one major issue, next to H1 and already on `main`, four minor ones, some nits, and one bug outside the plan. Each fix was made on `scratch/p3-review` in the task that owns the code, with a test, and each test was checked by reverting its fix (the verification's reverts). CI then ran the elevated jobs on the fixed code, and the user ran `run-p3-host.ps1` on it (F9).

**Major:**
1. **M1: a Delete whose outcome came 250 ms–1 s after its Cleanup was lost, and nothing counted it.** The callback pairs a Cleanup with its outcome within 1 s (§5.1) and passes it on, so `cleanup_outcome_late` does not count it; the pipeline's Cleanup record expired after the 250 ms confirm window, so the outcome went to `early_outcomes` and was never paired. Reproduced on `main` and the draft (a delete-on-close handle, outcome at 500 ms: no event, no count). H1.1 had copied the 250 ms as the retire delay, and the draft's §7.1 text contradicted §5.1. F4's outcomes all came within 100 ms, but on local NTFS; a filter that scans at Cleanup, or SMB, can be slower, and a delete-on-close of a freshly written file is the dropper pattern. **Fixed (H3, the user's decision A; Task 1):** one constant, `cleanup::OUTCOME_WINDOW` (1 s), sets the callback's pairing, the pipeline's record and the retire delay; `the_cleanup_outcome_still_finds_the_entry` now has its outcome 894 ms after the Cleanup. Two reverts (the record at the confirm window; the retire delay at the confirm window). F9 measured the cost: `retiring_fos` at most 1,177 during the `du` walk.

**Minor:**
1. **m1: `file_object_replaced` changed meaning.** After H1 a Create on an address whose Close never came found no entry, so it was not counted (a Create, a Cleanup, no Close, a Create 1 s later: 1 on `main`, 0 on the draft), while §7.1 says the counter measures address reuse after a lost Close. The hour run had 29,979. **Fixed** (Task 1): a Create on a FileObject in the cleaned record counts it; one revert.
2. **m2: the cleaned record was never tested after the start-up pass.** Every H1 test ran during the pass, so moving H2's rule above the record's check passed all 261 tests; in production that would stop counting late writes. **Fixed** (Task 2): `after_the_start_up_pass_a_late_write_is_still_absorbed`; one revert.
3. **m3: an outcome on a FileObject with no entry made an entry that never left.** `on_cleanup_outcome` → `file_action_on` makes an entry to wait for the seeder; it was never marked cleaned or queued, so it stayed until a Close or an eviction (reproduced during the start-up pass with on-miss off, and after it with on-miss on). Already on `main`, low volume, but against the new §7.1. **Fixed** (Task 1): such an entry is past its Cleanup and is queued like any other; one revert.
4. **m4: the text.** §13 compared unlike runs (the 231k entries and 312 MB were F3's 30-minute probe run, not `run-p3-host.ps1` on the old code); the Global Constraints' "no behaviour change but H2" left out late writes past 60 s, `file_object_replaced` and events past the window; §7.1's Bounds said "open handles, and cleaned ones for one window", leaving out provisional entries and failed Creates whose failure came late; F7 was numbered twice (the notes' and the spec's). **Fixed:** §13 names each run; the Global Constraints list each change; Bounds lists what the map holds; the spec's F numbers now follow the notes' (F1–F9, Task 4's run F10).

**Nits, fixed:** the window edge (an outcome exactly one window after its Cleanup was paired but found no entry: retirement is now strictly before the stream, `an_outcome_exactly_one_window_late_still_finds_its_entry`, one revert); "then leaves a window later" is now "at the next pass after the event is answered or leaves the completion stage"; the record is about 4 MB at its cap, its order queue included, not 2 MB; `retiring_fos` joins `cleaned_fos` in `sizes=` (one revert); the record is looked at only on a miss (`on_unknown_write`), not on every write; F7's `cleaned_fos` range is 2.3k–4.9k.

**Nits, not changed:** the seeder's `None` branch does not look at the cleaned record. The reviewer found it safe: `taken` is the verifying second table read, and snapshots apply at a stream time at or after it, before anything cleaned after it can retire. A guard would only remove that dependency. H2 depends on the start-up pass replying: if its handle-table read fails, `startup_pending` stays true and H2 never applies. That is a Sensor Health question; it goes to plan 1b-4b (Interfaces).

**Outside the plan, already on `main` (since plan 1b-3a): a late 8.3 expansion named the next file at its address.** `on_expanded` wrote the long path into whatever entry held the FileObject when the answer came: A creates `PROGRA~1\a.txt`, writes, cleans up and closes; B is created at the same address as `b.txt`; A's answer lands on B's entry, and B's Update reports `C:\Program Files\a.txt`. Reproduced. **Fixed** (Task 3, the user's choice to fix it in this plan): each expansion keeps the NT path it asked about and fills an entry only if the entry still has it; one revert.

**What held up:** `retire`'s check (`cleaned == Some(at)`) keeps a reused address's new entry; the hold-and-requeue for waiters works, and the waiting list always empties (the seeder's answer, unnamable, drain, cancel and flush all clear it), so requeues end; the once-per-pass bound is safe at `i64::MAX` and with a zero window. Cleanups processed in order give increasing due times; a late arrival waits at most one window behind; requeued entries go to the back with the largest due time. Within the window: the delete-on-close requester is stored at the Cleanup; a Delete on another handle's Cleanup uses that handle's entry; an early outcome is handled before queuing; the POSIX check uses the cleaning handle's own entry; renames confirm before the entry retires. With `file.op_end` off the entry retires after the Update and Delete emitted at the Cleanup. H2 is consistent with `file_action_on`'s wait rule. Keeping entries for the outcome only when delete-on-close fails 11 older tests, so the outcome's use of the entry is covered beyond its own test. The figures (F1, F4, F6, F7) match the logs.
