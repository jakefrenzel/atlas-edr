# Sub-project 1b-4a-p3 — The File Map's Memory Implementation Plan

> **Status:** Draft (2026-10-31), for independent review, then the user's approval. **For agentic workers:** steps use checkbox (`- [ ] `) syntax for tracking. Task 3 is a run on the host by the user, from a script. Nothing in this plan needs the VM or a kernel driver.

**Goal:** Bring the agent's working set within §13's 150 MB over a workday. The hour run (decision log, 2026-10-31) met the CPU budget (1.59% of one core) but reached 281 MB: the file map kept FileObjects whose Close Windows sends late or not at all. Two fixes, each decided from measured facts (the p3 notes, H1 and H2):
- **entries leave the file map after their Cleanup** (H1), not at their Close: past its Cleanup an entry serves only the Cleanup's outcome, which follows within 100 ms; a 60 s record of cleaned FileObjects absorbs late writes;
- **no provisional entry that nothing can name** (H2): after the start-up pass, with on-miss seeding off, a write on a handle the agent never saw opened makes no entry.

**Architecture:** all in `pipeline::file`, the FileObject map's stage, plus one name in `sizes=`.
- **`FileEntry`:** `cleaned: bool` becomes `cleaned: Option<i64>`, the QPC of its Cleanup.
- **`Files`:** `retiring`, a queue of entries past their Cleanup, in Cleanup order, each with the stream time it is due (one confirm window after its Cleanup); `cleaned`, a `Recent<u64>` of FileObjects whose entry left (60 s, at most 65,536); `retire(fo, at)` takes an entry out if it is still the one cleaned at `at` and no event waits on it.
- **`on_file_cleanup`** queues the entry, or retires it at once with `file.op_end` off; **`file_confirm`** (every stream step) retires what is due, each queued entry at most once a pass, and requeues an entry an event still waits on; it expires the record.
- **`on_file_write`:** a FileObject in the record counts `writes_after_cleanup`; after the start-up pass, with on-miss seeding off, an unknown one makes no entry.
- **`on_file_create`** and **`on_file_close`** take the FileObject out of the record.
- **`Pipeline::sizes`:** `cleaned_fos`.

**Tech stack:** Rust 1.97 (edition 2024). No new dependency. Measurement tools (git-ignored, Task 3): plan 1b-4a's hour script and summary, the log copier, and `run-p3-host.ps1` (the agent while Git's `du` walks a folder).

**Spec:** `docs/specs/2026-10-01-etw-sensor-design.md`, revision 3, with the clarifications of plans 1b-2 to 1b-4a-p2. Section numbers (§) refer to it. The p3 notes (`docs/specs/2026-10-31-etw-sensor-1b-4a-p3-brainstorm-notes.md`) have the facts behind each decision (F1–F8).

**Verification note (2026-10-31, host, Windows 11 build 26200, and the CI runner, build 26100):** every code block below was compiled and run in a scratch worktree of `main` (`e3c7211`), `scratch/p3`, one commit per task (Task 1 `eef2877`, Task 2 `d2e0274`, Task 4's spec text `168885a`).
- **Lint:** `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` are clean at the end of every task.
- **Unelevated:** `cargo test --workspace` passes 449 tests and ignores 19 after Task 1 and 450 tests and ignores 19 after Task 2 (plan 1b-4a-p2 left 441 and 19). `atlas-agent` has 260 and 261 unit tests (252 before). Counts were measured at each task's commit in the round trip.
- **On the CI runner** (the scratch branch pushed, no PR): run 41250556287 (Task 1) and run 41253970989 (Task 2) passed every job, the binary test and the agent live test among them.
- **On the host:** the probe runs behind F3 and F4 (a probe build on `scratch/fm-probe`, never for `main`); `run-p3-host.ps1` on Task 1's build (F6) and on Task 2's (F7).
- **Reverts:** every rule in the Review Focus was checked by reverting it in the source and running its test, which must then fail (`spikes/p3-plan-src/mutate.py`, plan 1b-4a-p1's with one change: a revert whose test filter matches no test now fails the check). 15 reverts, all caught on the final code:
  - entries never leave in the confirm pass: `a_walk_without_closes_leaves_only_open_handles_in_the_map` (caught)
  - the cleaned record never expires: `a_walk_without_closes_leaves_only_open_handles_in_the_map` (caught)
  - a late write after the entry left makes a provisional entry: `a_late_write_after_the_entry_left_is_absorbed` (caught)
  - with OperationEnds the entry leaves at its Cleanup, before the outcome: `the_cleanup_outcome_still_finds_the_entry` (caught)
  - without OperationEnds the entry waits a window anyway: `without_operation_ends_an_entry_leaves_at_its_cleanup` (caught)
  - an entry leaves while an event waits on it for the seeder: `an_entry_waited_on_stays_until_answered` (caught)
  - a waited-on entry is not looked at again: `an_entry_waited_on_stays_until_answered` (caught)
  - retirement takes whatever entry holds the address: `a_reused_address_inside_the_window_keeps_its_new_entry` (caught)
  - retirement takes any cleaned entry, not the one cleaned then: `a_second_cleanup_on_a_reused_address_waits_for_its_own_outcome` (caught)
  - a Create leaves the address in the cleaned record: `a_reused_or_closed_address_leaves_the_cleaned_record` (caught)
  - a Close leaves the address in the cleaned record: `a_reused_or_closed_address_leaves_the_cleaned_record` (caught)
  - cleaned_fos missing from sizes=: `the_sizes_name_each_bounded_structure` (caught)
  - a write on an unknown handle after the pass makes an entry: `without_on_miss_seeding_only_the_start_up_pass_is_waited_for` (caught)
  - no entry during the start-up pass either: `without_on_miss_seeding_only_the_start_up_pass_is_waited_for` (caught)
  - no entry with on-miss seeding on: `with_on_miss_seeding_an_unknown_write_still_asks` (caught)
- **Round trip:** the code was rebuilt from this document alone in a fresh LF worktree of `main` (`spikes/p3-plan-src/roundtrip.py`): every diff applied, and every task's tree is identical to its verified commit; the test counts above were measured there.
- **Not run yet:** Task 3 (the hour run) on the build branch; the Linux build of the tests is run by CI only.

## Global Constraints

- **Branches.** This plan is reviewed on `docs/1b-4a-p3-plan`. The build runs on `feat/1b-4a-p3`, created from `main` after this plan merges. Never commit to `main`. (Claude may merge a PR itself once every check on its current head is green; the user granted that on 2026-10-05.)
- **Portable code:** `pipeline` stays portable; Linux CI builds and tests it.
- **The ETW callback is untouched** (§3.2).
- **No behaviour change but the decided ones:** the same events, fields and counters as before for the same input, except that a write on an unknown handle after the start-up pass no longer leads to an `unknown_file_object` count at its Cleanup (H2); the replay tests still turn the CI recording into the same snapshot.
- **Elevation:** Claude never runs elevated on the host (plan 1a, D2). Task 3's run is the user's.
- **The host's data stays on the host:** logs and dumps stay in the git-ignored `spikes\results\`; only summary figures enter the docs.
- **No schema change.**
- **Formatting and lint** at the end of every task, as in the verification note.
- **Shell:** PowerShell 7 (`cargo` commands are the same in bash). Edits that contain backslashes are made with an editor, never a heredoc.

## Decisions

The p3 notes have the facts; the decision log has each entry.

### H1, file map entries whose Close never comes (chosen 2026-10-31: A)

- **(A) Chosen: an entry leaves the map once its Cleanup's outcome is handled; a small bounded record of recently cleaned FileObjects absorbs late writes.** The map then holds open handles only, and memory no longer follows how late Windows closes files (F2, F3).
- (B) Rejected: smaller entries (the opener by reference, shared paths): a walk like `du`'s still fills the map to its cap.
- (C) Rejected: a lower cap only: evicts long-open handles' names and still lets memory reach the cap.

### H1.1–H1.5, the details (chosen 2026-10-31, accepted together after F4)

1. **When:** one confirm window (250 ms) after the Cleanup, the window the Cleanup's outcome record already lives for; at the Cleanup itself with `file.op_end` off; an entry that an event waits on for the seeder stays until the event is answered, then leaves a window later.
2. **The record:** FileObject values only, kept 60 s, at most 65,536, oldest out first (about 2 MB). F4 found no SetAttributes, Rename or SetDelete on a FileObject past its Cleanup, so no path is kept; 99.6% of late writes came within 60 s. A write that finds its FileObject there counts `writes_after_cleanup`; past it a late write is an unknown handle's, today's rule.
3. **Reuse:** a Create or a Close on a recorded FileObject takes it out.
4. **Observability:** `sizes=` gains `cleaned_fos=`; the map's 262,144 cap stays and now bounds open handles only; no new counter.
5. **Tests:** each rule a unit test checked by reverting it; a walk test leaves the map at its open-handle count; then the hour run again.

### H2, provisional entries nothing can name (chosen 2026-10-31: A)

- **(A) Chosen: after the start-up pass, with on-miss seeding off, a write on a FileObject the agent never saw opened makes no entry** (F5). Its Update would be dropped as `unknown_file_object` anyway, and the entry would hold memory until a Close that may not come. During the start-up pass, and with on-miss seeding on, writes make provisional entries as before. `unknown_file_object` no longer counts the Update a later Cleanup of such a handle would have dropped (in F5's run none of them got a Cleanup).
- (B) Rejected: expire provisional entries through the same queue: more code for the same effect; it only keeps the count.
- (C) Rejected: leave it for 1b-4c: bounded by the cap, but a 24-hour run would sit near it, about 100 MB of map.

## Findings

The p3 notes' F1–F8, in short:
- **F1, the hour run:** 74.7 minutes of normal use: CPU 1.59% of one core, nothing lost; the working set 51 → 281 MB.
- **F2:** only the file map grew (`sizes=`), 3,000–5,000 entries a minute, to its cap; each working-set jump was one doubling of its table.
- **F3:** 204,526 of 231,340 entries were a `du` walk's files past their Cleanup, with no Close; Close reaches the pipeline, so nothing is lost; Windows sends it late or not at all.
- **F4:** past its Cleanup a FileObject sees only the Cleanup's outcome (within 100 ms), late writes (99.6% within 60 s) and its Close; no SetAttributes, Rename or SetDelete.
- **F5:** provisional entries grew 400–600 a minute, never cleaned up or closed.
- **F6, F7:** the same 10-minute `du` walk with Task 1's build: the file map 4.6k → 9.6k, at most 70 MB; with Task 2's too: 4,485 → 4,511, at most 71 MB, nothing lost (the probe build: 231k entries, 312 MB).
- **F8, tooling:** `Start-Process` in Windows PowerShell 5.1 does not quote arguments; the agent's log rolls over at UTC midnight; a revert whose filter matches no test passed as a miss.

## Deliberate clarifications of the spec (applied to the spec text in Task 4)

1. **§7.1:** an entry leaves after its Cleanup, not at its Close; the cleaned record; late writes (H1).
2. **§7.1:** no provisional entry after the start-up pass with on-miss seeding off (H2).
3. **§9.3:** `cleaned_fos` in `sizes=`.
4. **§13, §15.3:** the hour run's figures and F1–F6.
5. **§17:** the clarifications.

## Review Focus

1. **An entry past its Cleanup leaves one window later, and the map then holds open handles only** (`a_walk_without_closes_leaves_only_open_handles_in_the_map`: 1,000 files created and cleaned up with no Close, one open handle; after the window the map holds 1 and the record 1,000; after 60 s the record holds none).
2. **The Cleanup's outcome still finds its entry** (`the_cleanup_outcome_still_finds_the_entry`: a delete-on-close handle's outcome 194 ms after its Cleanup gives a Delete with the path and the opener).
3. **Without OperationEnds the entry leaves at its Cleanup** (`without_operation_ends_an_entry_leaves_at_its_cleanup`).
4. **A late write is absorbed while the record holds its FileObject, and is an unknown handle's after** (`a_late_write_after_the_entry_left_is_absorbed`).
5. **An entry an event waits on stays until the event is answered, then leaves** (`an_entry_waited_on_stays_until_answered`: the window passes with the Update waiting on the start-up seeder; the answer still names it; a window later the map is empty).
6. **A retirement takes only the entry it queued** (`a_reused_address_inside_the_window_keeps_its_new_entry`: a Close and a new Create inside the window keep the new open handle; `a_second_cleanup_on_a_reused_address_waits_for_its_own_outcome`: a second Cleanup on the address waits for its own outcome).
7. **A Create or a Close takes an address out of the record** (`a_reused_or_closed_address_leaves_the_cleaned_record`).
8. **`sizes=` names `cleaned_fos`** (`the_sizes_name_each_bounded_structure`).
9. **After the start-up pass, with on-miss seeding off, an unknown write makes no entry; during the pass, and with on-miss seeding on, it still does** (`without_on_miss_seeding_only_the_start_up_pass_is_waited_for`, changed: its second half now expects no entry and no count; `with_on_miss_seeding_an_unknown_write_still_asks`).
10. **The pipeline's bookkeeping still empties** (`bookkeeping_is_freed_when_no_reply_comes`, changed: it runs one more window, since an entry past its Cleanup leaves a window after the events waiting on it; the test-only list gains the queue and the record).
11. **Nothing else changes:** the CI recording still gives the scenario and the snapshot, the driver loop the synchronous replay's events, and every other earlier test passes unchanged.

Every rule in 1–9 was checked by reverting it: its test fails (verification note). One rule has no revert check of its own: the retirement pass looks at each queued entry at most once (a final flush runs at the end of time, where a requeued entry is due again at once). Without it `a_clean_stop_drops_what_a_deadline_would` hung in the draft; a revert would hang the suite, so it is not in the revert list.

## File Structure

```
crates/atlas-agent/src/pipeline/file.rs   FileEntry.cleaned as a time; Files' retiring queue and cleaned record; the rules
crates/atlas-agent/src/pipeline/mod.rs    sizes: cleaned_fos
crates/atlas-agent/src/pipeline/tests.rs  the tests of the Review Focus
spikes/                                   git-ignored; Task 3: run-1b4a-hour.ps1, summarize-1b4a.ps1, copy-1b4a-hour-log.ps1, run-p3-host.ps1
docs/specs/2026-10-01-etw-sensor-design.md   Task 4
```

## Interfaces for later plans

- **The hour run** (Task 3) ends the profiling step: normal use for an hour, judged against §13 from the Sensor Health reports and the `sizes=` lines. If the working set is still over budget, what holds it comes back as a decision.
- **Plan 1b-4b (blinding detection):** unchanged from plan 1b-4a-p2's interfaces.
- **Plan 1b-4c (the service):** the 24-hour run reads `file_map=` and `cleaned_fos=` in `sizes=`; with H1 and H2 the map should stay near the machine's open-handle count. A machine with on-miss seeding turned on still makes provisional entries (H2 leaves it), bounded by the cap.

---

### Task 1: File map entries leave after their Cleanup (H1)

**Files:**
- Modify: `crates/atlas-agent/src/pipeline/file.rs`, `crates/atlas-agent/src/pipeline/mod.rs`, `crates/atlas-agent/src/pipeline/tests.rs`

- [ ] **Step 1: The file stage** (H1, H1.1–H1.4). `FileEntry.cleaned` becomes the QPC of the Cleanup. `on_file_cleanup` queues the entry for one window, or retires it at once without OperationEnds (an entry an event waits on is queued then too). `file_confirm` retires what is due, each queued entry at most once a pass (a final flush runs at `i64::MAX`, where a requeued entry is due again at once), and expires the record. `retire` checks the entry is still the one cleaned then, so an address closed and reused inside the window keeps its new entry. A late write that finds its FileObject in the record counts `writes_after_cleanup` and makes no provisional entry; a Create or a Close takes the FileObject out.

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
@@ -120,6 +127,13 @@ pub struct Files {
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
@@ -152,21 +166,41 @@ impl Files {
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
+    /// opens (R6) and the cleaned FileObjects (H1).
+    pub(super) fn recent_sizes(&self) -> [(usize, usize); 4] {
         [
             (self.confirmed.len(), self.confirmed.capacity()),
             (self.failed.len(), self.failed.capacity()),
             (self.opened.len(), self.opened.capacity()),
+            (self.cleaned.len(), self.cleaned.capacity()),
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
@@ -175,6 +209,8 @@ impl Files {
             ("confirmed irps", self.confirmed.len()),
             ("watchlist opens", self.opened.len()),
             ("cleanups awaiting an outcome", self.cleanups.len() + self.early_outcomes.len() + self.request_irps.len()),
+            ("entries past their cleanup", self.retiring.len()),
+            ("cleaned file objects", self.cleaned.len()),
         ]
     }
 
@@ -217,7 +253,7 @@ fn entry(nt: Option<String>, opener: Option<ProcessRef>, since: i64) -> FileEntr
         opener,
         written: false,
         delete_on_close: false,
-        cleaned: false,
+        cleaned: None,
         since,
         touched: 0,
         waiting: Vec::new(),
@@ -298,6 +334,7 @@ impl<L: Lookups> Pipeline<L> {
             self.counters.file_object_replaced += 1;
         }
         self.seeding.changed(HandleKind::File, c.file_object, h.ts);
+        self.files.cleaned.remove(&c.file_object); // the address is reused
         let mut e = entry(Some(nt.clone()), opener.clone(), h.ts);
         e.delete_on_close = c.delete_on_close();
         self.files.insert(c.file_object, e);
@@ -350,13 +387,16 @@ impl<L: Lookups> Pipeline<L> {
 
     /// A Write, or a truncation (the overwrite case, §5.1).
     pub(super) fn on_file_write(&mut self, h: &Header, fo: u64) {
+        // A lazy-writer flush or a mapped page after the entry left (H1).
+        let after_cleanup = self.files.cleaned.get(&fo).is_some();
         match self.files.touch(fo) {
             Some(e) => {
-                if e.cleaned {
+                if e.cleaned.is_some() {
                     self.counters.writes_after_cleanup += 1;
                 }
                 e.written = true;
             }
+            None if after_cleanup => self.counters.writes_after_cleanup += 1,
             None => {
                 let mut e = entry(None, None, h.ts);
                 e.written = true;
@@ -599,17 +639,26 @@ impl<L: Lookups> Pipeline<L> {
         if let Some((oh, info)) = early {
             self.on_cleanup_outcome(&oh, x.file_object, x.file_key, info);
         }
+        // Past its Cleanup an entry serves only the outcome, which follows at
+        // once: it leaves after the outcome's window, or now without
+        // OperationEnds, whenever its Close comes (H1).
+        let fo = x.file_object;
+        if self.files.map.get(&fo).is_some_and(|e| e.cleaned == Some(h.ts)) && (op_end || !self.files.retire(fo, h.ts))
+        {
+            let due = h.ts.saturating_add(self.files.window);
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
@@ -637,6 +686,7 @@ impl<L: Lookups> Pipeline<L> {
 
     pub(super) fn on_file_close(&mut self, h: &Header, x: FileHandle) {
         self.files.map.remove(&x.file_object);
+        self.files.cleaned.remove(&x.file_object);
         self.seeding.changed(HandleKind::File, x.file_object, h.ts);
     }
 
@@ -678,6 +728,20 @@ impl<L: Lookups> Pipeline<L> {
                 self.files.confirmed.insert(irp, ts);
             }
         }
+        // Entries past their Cleanup leave once the outcome's window passed (H1);
+        // one an event still waits on is looked at again a window later (each
+        // at most once a pass: a final flush runs at the end of time).
+        for _ in 0..self.files.retiring.len() {
+            let Some(&(due, at, fo)) = self.files.retiring.front() else { break };
+            if due > stream {
+                break;
+            }
+            self.files.retiring.pop_front();
+            if !self.files.retire(fo, at) {
+                self.files.retiring.push_back((stream.saturating_add(window), at, fo));
+            }
+        }
+        self.files.cleaned.expire(stream.saturating_sub(self.ticks.of(CLEANED_KEEP)));
         self.files.failed.expire(stream.saturating_sub(window));
         // A Cleanup's OperationEnd follows it at once: a window is plenty.
         self.files.cleanups.expire(stream.saturating_sub(window));
```

- [ ] **Step 2: `cleaned_fos` in `sizes=`.**

```diff
--- a/crates/atlas-agent/src/pipeline/mod.rs
+++ b/crates/atlas-agent/src/pipeline/mod.rs
@@ -293,7 +293,7 @@ impl<L: Lookups> Pipeline<L> {
     pub fn sizes(&self) -> crate::health::Sizes {
         let (line, line_cap, pending, pending_cap) = self.completion.sizes();
         let (tcp, tcp_cap, flows, flows_cap) = self.net.sizes();
-        let [confirmed, failed, opened] = self.files.recent_sizes();
+        let [confirmed, failed, opened, cleaned] = self.files.recent_sizes();
         let n = |len: usize, cap: usize| (len as u64, cap as u64);
         let entries = [
             ("ordering", n(self.ordering.len(), self.ordering.capacity())),
@@ -305,6 +305,7 @@ impl<L: Lookups> Pipeline<L> {
             ("confirmed_irps", n(confirmed.0, confirmed.1)),
             ("failed_irps", n(failed.0, failed.1)),
             ("watch_opens", n(opened.0, opened.1)),
+            ("cleaned_fos", n(cleaned.0, cleaned.1)),
             ("address_history", n(self.seeding.history_len(), self.seeding.history_capacity())),
             ("negative_cache", n(self.seeding.negative_cache_len(), self.seeding.negative_cache_capacity())),
             ("tcp", n(tcp, tcp_cap)),
```

- [ ] **Step 3: The tests** (Review Focus 1–8, 10). The name list of `sizes=`; one more window for the bookkeeping test; eight new tests at the end of the file.

```diff
--- a/crates/atlas-agent/src/pipeline/tests.rs
+++ b/crates/atlas-agent/src/pipeline/tests.rs
@@ -625,6 +625,7 @@ fn the_sizes_name_each_bounded_structure() {
             "confirmed_irps",
             "failed_irps",
             "watch_opens",
+            "cleaned_fos",
             "address_history",
             "negative_cache",
             "tcp",
@@ -1469,6 +1470,10 @@ fn bookkeeping_is_freed_when_no_reply_comes() {
     }
     let out = t.settle();
     assert!(!out.is_empty());
+    // An entry past its Cleanup leaves a window after the events waiting on
+    // it for the seeder (plan 1b-4a-p3, H1).
+    let later = t.now / (FREQ / 1000) + 1_000;
+    t.at(later);
     no_bookkeeping_left(&t);
 }
 
@@ -1895,3 +1900,152 @@ fn a_request_that_succeeded_stands_against_a_later_failure_on_its_irp() {
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
+    t.ev(200, 4, None, outcome(2, crate::cleanup::FILE_DELETED)); // late, inside the window
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
+    // After the first Cleanup's window (6 + 250 ms), inside the second's.
+    t.ev(400, 4, None, outcome(3, crate::cleanup::FILE_DELETED));
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
```

- [ ] **Step 4: Check and commit**

```powershell
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass. Workspace: 449 tests and ignores 19 (`atlas-agent` unit tests: 260).
```powershell
git add crates/atlas-agent
git commit -m "feat(agent): file map entries leave after their Cleanup (H1)"
```

### Task 2: No provisional entry that nothing can name (H2)

**Files:**
- Modify: `crates/atlas-agent/src/pipeline/file.rs`, `crates/atlas-agent/src/pipeline/tests.rs`

- [ ] **Step 1: `on_file_write`** (H2, F5). After the start-up pass, with on-miss seeding off, a write on an unknown FileObject makes no entry.

```diff
--- a/crates/atlas-agent/src/pipeline/file.rs
+++ b/crates/atlas-agent/src/pipeline/file.rs
@@ -397,6 +397,11 @@ impl<L: Lookups> Pipeline<L> {
                 e.written = true;
             }
             None if after_cleanup => self.counters.writes_after_cleanup += 1,
+            // Once the start-up pass is done, and with on-miss seeding off,
+            // nothing can name a handle the agent never saw opened: its Update
+            // would be dropped, and an entry would hold memory until a Close
+            // that may never come (H2).
+            None if !self.cfg.seed_on_miss && !self.seeding.startup_pending(HandleKind::File) => {}
             None => {
                 let mut e = entry(None, None, h.ts);
                 e.written = true;
```

- [ ] **Step 2: The tests** (Review Focus 9). The start-up pass test's second half now expects no entry and no count; a new test with on-miss seeding on.

```diff
--- a/crates/atlas-agent/src/pipeline/tests.rs
+++ b/crates/atlas-agent/src/pipeline/tests.rs
@@ -1606,11 +1606,14 @@ fn without_on_miss_seeding_only_the_start_up_pass_is_waited_for() {
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
@@ -2049,3 +2052,24 @@ fn an_entry_waited_on_stays_until_answered() {
     t.at(4_000);
     assert_eq!(size_of(&t, "file_map"), 0, "gone once answered");
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
```

- [ ] **Step 3: Check and commit**

```powershell
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass. Workspace: 450 tests and ignores 19 (`atlas-agent` unit tests: 261).
```powershell
git add crates/atlas-agent
git commit -m "feat(agent): no provisional file entry once nothing can name it (H2)"
```

### Task 3: The hour run (the user)

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
- [ ] **Step 3:** Claude reads `spikes\results\1b4a-hour\`: the summary (CPU, working set, losses, both unpaired-Cleanup parts) and the `sizes=` lines per minute (`file_map`, `cleaned_fos`, and any structure that grows). Expected: nothing lost; the working set within 150 MB; the file map near its open-handle count. The figures go into Task 4.

### Task 4: Documentation

- [ ] **Step 1: The spec.** In `docs/specs/2026-10-01-etw-sensor-design.md`: the status line (its date is the build's merge day: change `2026-10-31` if it differs); §7.1 (H1, H2); §9.3 (`cleaned_fos`); §13 (the hour run's figures; Task 3's figures added); §15.3 (a "Plan 1b-4a-p3" addendum, F1–F6, and Task 3's run as F7); §17 ("Plan 1b-4a-p3 clarifications", and, after the review, its fixes). `docs/architecture-overview.md` (roadmap row 1 and a decision-log row for the build) is a second commit, `docs(1): plan 1b-4a-p3 built: roadmap and decision log`, made once Task 3's figures are in.

```diff
--- a/docs/specs/2026-10-01-etw-sensor-design.md
+++ b/docs/specs/2026-10-01-etw-sensor-design.md
@@ -1,6 +1,6 @@
 # Sub-project 1 — ETW Sensor Design (Agent Core)
 
-**Status:** Approved (2026-10-02), revision 3. Revision 2 was approved on 2026-10-01; revision 3 folds the spike results (§15.3) and the decisions made during the spikes into the design sections (§17). Revision 1 had an independent review; its findings are folded in. Implementation: **plan 1a** (spikes S1–S10, §15.2) is done; **plan 1b** (the build) comes in six parts, each reviewed and approved before it runs (decision log, 2026-10-02; plan 1b-3 was split in two on 2026-10-05, and a plan 1b-3c added on 2026-10-06); plan 1b-1 (schema additions + `atlas-buffer`) is done (2026-10-04); plan 1b-2 (`atlas-etw`) is done (2026-10-05); plan 1b-3a (the `atlas-agent` pipeline core) is done (2026-10-05); plan 1b-3b (the Windows services behind it) is done (2026-10-06); plan 1b-3c (the driver, the agent-level live test, and deletes from the Cleanup outcome) is done (2026-10-06); plan 1b-4 is split in three (decision log, 2026-10-12): plan 1b-4a (a runnable agent: `run`, `dump`, `agent.toml`, the buffer as the sink, Sensor Health reports) is done (2026-10-13); the profiling step that comes before 1b-4b (decision log, 2026-10-12, D6) produced plan 1b-4a-p1 (the hash cache no longer forgets a file on a SetAttributes; the events each session delivers and each thread's CPU are measured), done (2026-10-15), and plan 1b-4a-p2 (on-miss seeding off; the pipeline's measured fixes; cache invalidations that wake no thread; Launch images hashed first; each bounded structure's size in the log; memory that follows contents; no power throttling), done (2026-10-27); the hour run that ends the profiling step is next, then plan 1b-4b (blinding detection), then 1b-4c (the service and acceptance). Brainstorm handoff: [etw-sensor-brainstorm-notes](2026-10-01-etw-sensor-brainstorm-notes.md).
+**Status:** Approved (2026-10-02), revision 3. Revision 2 was approved on 2026-10-01; revision 3 folds the spike results (§15.3) and the decisions made during the spikes into the design sections (§17). Revision 1 had an independent review; its findings are folded in. Implementation: **plan 1a** (spikes S1–S10, §15.2) is done; **plan 1b** (the build) comes in six parts, each reviewed and approved before it runs (decision log, 2026-10-02; plan 1b-3 was split in two on 2026-10-05, and a plan 1b-3c added on 2026-10-06); plan 1b-1 (schema additions + `atlas-buffer`) is done (2026-10-04); plan 1b-2 (`atlas-etw`) is done (2026-10-05); plan 1b-3a (the `atlas-agent` pipeline core) is done (2026-10-05); plan 1b-3b (the Windows services behind it) is done (2026-10-06); plan 1b-3c (the driver, the agent-level live test, and deletes from the Cleanup outcome) is done (2026-10-06); plan 1b-4 is split in three (decision log, 2026-10-12): plan 1b-4a (a runnable agent: `run`, `dump`, `agent.toml`, the buffer as the sink, Sensor Health reports) is done (2026-10-13); the profiling step that comes before 1b-4b (decision log, 2026-10-12, D6) produced plan 1b-4a-p1 (the hash cache no longer forgets a file on a SetAttributes; the events each session delivers and each thread's CPU are measured), done (2026-10-15), and plan 1b-4a-p2 (on-miss seeding off; the pipeline's measured fixes; cache invalidations that wake no thread; Launch images hashed first; each bounded structure's size in the log; memory that follows contents; no power throttling), done (2026-10-27); the hour run (2026-10-31) met the CPU budget but not the working set, and produced plan 1b-4a-p3 (file map entries leave after their Cleanup; no provisional entry that nothing can name), done (2026-10-31); the hour run again ends the profiling step, then plan 1b-4b (blinding detection), then 1b-4c (the service and acceptance). Brainstorm handoff: [etw-sensor-brainstorm-notes](2026-10-01-etw-sensor-brainstorm-notes.md).
 **Depends on:** 0a (event schema: domain types, `process_uid`, validating `TryFrom`), 0b (CI, nightly fuzz workflow).
 **Depended on by:** sub-project 2 (adds enrollment + gRPC on top of the buffer's read API), sub-project 3 (fills the agent-side detection hook), sub-project 6 (the driver becomes a second sensor feeding the same pipeline).
 
@@ -331,17 +331,19 @@ Querying `ProcessTelemetryIdInformation` for the new PID at Launch is **not** a
 - The actor of an Update is the entry's opener, never the Cleanup's header. The cache manager, or the agent's own duplicate during seeding (§7.4), can be the closing context.
 - A File Update or Delete from a provisional entry waits in [5] with a "drop at deadline" seeder reason (§3.2): if the entry is still unresolved at the deadline, the event is dropped and `unknown_file_object` counts it. This is a deliberate carve-out from E11: a file event without a path, and usually without a trustworthy actor, has no detection value.
 
-**Close and after.**
-- Close (14) removes the entry.
-- Writes that arrive after Cleanup (lazy-writer flushes, memory-mapped paging writes) are absorbed into the Update already emitted and counted as `writes_after_cleanup`.
+**After the Cleanup** (plan 1b-4a-p3, H1).
+- The entry leaves once the Cleanup's outcome window (the confirm window, §5.5) has passed, or at the Cleanup itself with `file.op_end` off. Past its Cleanup it serves only that outcome, which follows at once. Windows may send the Close (14) long after the Cleanup, or not for hours, for a file it keeps cached or mapped: a `du` walk once left 204,526 entries past their Cleanup, and the working set at 312 MB (§15.3). An entry that an event waits on for the seeder stays until the event is answered, then leaves a window later.
+- A FileObject whose entry left is kept in a **cleaned record**, FileObject values only, for 60 s (at most 65,536, oldest out first). A Create or a Close on it takes it out: the address belongs to another file object then.
+- Close (14) removes the entry if it is still there.
+- Writes that arrive after Cleanup (lazy-writer flushes, memory-mapped paging writes) are absorbed into the Update already emitted and counted as `writes_after_cleanup`, whether the entry is still there or its FileObject is in the cleaned record. Past the record's 60 s such a write is an unknown handle's (on the host, 99.6% of late writes came within 60 s).
 - A confirmed rename (§5.5) changes the entry's path to the new name, so a later Update carries the name the file has now.
 
 **Unknown handles, refined** (plan 1b-3a):
 - A SetAttributes on a handle that is never named is dropped and counted (`unknown_file_object`), like Update and Delete.
 - A RenamePath on a handle whose name is unknown is emitted at once, with an empty source. It does not wait for the seeder, because a snapshot read after the rename could only give the new name. The handle takes the new name once the rename stands.
-- With on-miss seeding off (§11.2), such events wait only for the start-up pass, while it is outstanding.
+- With on-miss seeding off (§11.2), such events wait only for the start-up pass, while it is outstanding. After the pass a write or truncation on an unknown handle makes no provisional entry: nothing can name it, so its Update would be dropped, and on the host such entries got no Cleanup or Close (plan 1b-4a-p3, H2). `unknown_file_object` then does not count that Update.
 
-**Bounds.** The map is bounded (cap + counter; past the cap, the least recently used eighth goes at once, plan 1b-3a). Seeded entries do not know `delete_on_close`, but the Cleanup outcome still reports the delete of a handle opened with that flag before the agent started (plan 1b-3c); its actor is then the process whose Cleanup removed the name, the handle's owner.
+**Bounds.** The map is bounded (cap + counter; past the cap, the least recently used eighth goes at once, plan 1b-3a). Since plan 1b-4a-p3 it holds open handles, and cleaned ones for one window, so the cap is a safety net only. Seeded entries do not know `delete_on_close`, but the Cleanup outcome still reports the delete of a handle opened with that flag before the agent started (plan 1b-3c); its actor is then the process whose Cleanup removed the name, the handle's owner.
 
 ### 7.2 Watchlist
 
@@ -509,7 +511,7 @@ Emitted every 60 s (and immediately on a non-zero change in a loss counter):
 - **Snapshots and differences:** the health thread [7] reads every cumulative counter into a snapshot: the pipeline's (through the driver's control channel, waiting at most 2 s), the intake's and the services' atomics, the callbacks' panics and delivered events (counters the agent owns and hands to each consumer it starts, so a restarted session counts on into them; plan 1b-4a-p1, review M1), both sessions' losses (`ControlTraceW` query), the sink's drops, and the process's CPU time and working set. A report holds the differences between two snapshots. The sessions' counters are 32-bit, so their differences wrap. A source that could not be read is absent from the report, and the next report covers its interval too.
 - **When:** every 60 s; at once when a loss counter rose (checked every 5 s); and a final report at the clean stop, taken after the sessions stopped, so everything counted during the run is reported. The intervals chain: each starts where the last ended, the first at the anchor taken at start. A report's time is the system clock at generation (`GetSystemTimePreciseAsFileTime`, the clock the anchor reads, §3.3); its id is a UUIDv7. If the pipeline thread died, the final report is still sent, without the pipeline's counters (they died with it).
 - **The buffer's figures:** [6] fills in the `buffer` group, `loss.buffer_backlog_drops`, `housekeeping.retention_evictions` and the gap as it appends the report, each since the report before; so the drops of the events queued ahead of a report, the final drain's included, are in it (review finding 3). The health thread's own reading of the drops only decides an early report; the first report of a run carries what recovery found at open. A report the writer's backlog cannot take is held (up to 16) and written before anything else once there is room.
-- **The diagnostic log** gets one line per report with its main figures, including the two parts of `quality.file_cleanup_unpaired` (another operation came first on the `Irp`; the outcome came after the window), which the schema sums, and (plan 1b-4a-p1) the CPU each agent thread used since the last listing: each thread's difference (a thread is its id and creation time), summed by thread name, most first, with the listings' own interval. Threads are listed with a snapshot of every thread in the system (about 45 ms on the host), so at most once a report period, for a report of either kind; a report without them leaves them to the next listing. The final report's listing is taken just before the sensor stops, while its threads still run. Plan 1b-4a-p2 (R6) adds each bounded structure's entries and capacity, `sizes="name=entries/capacity …"`: the ordering stage, the completion stage's line and its pending events, the process cache, the file and key maps, the two `Irp` tables, the watchlist opens, the seeding address history, the negative cache, the TCP and UDP tables, the early key map, the hash and expansion caches, and the kernel, user and sink queues.
+- **The diagnostic log** gets one line per report with its main figures, including the two parts of `quality.file_cleanup_unpaired` (another operation came first on the `Irp`; the outcome came after the window), which the schema sums, and (plan 1b-4a-p1) the CPU each agent thread used since the last listing: each thread's difference (a thread is its id and creation time), summed by thread name, most first, with the listings' own interval. Threads are listed with a snapshot of every thread in the system (about 45 ms on the host), so at most once a report period, for a report of either kind; a report without them leaves them to the next listing. The final report's listing is taken just before the sensor stops, while its threads still run. Plan 1b-4a-p2 (R6) adds each bounded structure's entries and capacity, `sizes="name=entries/capacity …"`: the ordering stage, the completion stage's line and its pending events, the process cache, the file and key maps, the two `Irp` tables, the watchlist opens, the cleaned FileObjects (`cleaned_fos`, plan 1b-4a-p3), the seeding address history, the negative cache, the TCP and UDP tables, the early key map, the hash and expansion caches, and the kernel, user and sink queues.
 - **Not measured yet** (absent): `quality.buffer_invalid_records` and `buffer.corrupt_segments` (the agent never reads its own buffer back; `dump` counts both), and `housekeeping.seeder_negative_cache_size` (the negative cache's size is in the diagnostic log's `sizes=` since plan 1b-4a-p2, not in the schema).
 
 ## 10. Schema Additions (`atlas.events.v1`, additive)
@@ -660,6 +662,8 @@ Late arrivals, enrichment misses and DNS rate-limit drops are reported, not gate
 
 **Plan 1b-4a-p2** (D6, 2026-10-15 to 2026-10-27): on-miss seeding off (§13's first fallback), the pipeline's measured fixes, invalidations that wake no thread, Launch images first, memory that follows contents and no power throttling (§15.3). The old and the new build, back to back on the host under the same trace script, both on AC power, the Silent power plan and Windows' high quality of service: under the synthetic load, 11.4% → 10.5% of one core (without the hash workers 5.3% → 3.4%; the seeder 1.0% → 0), with 0 and 1 enrichment misses; the working set 109–130 MB → 40–66 MB, under budget. Five minutes of normal use took 2.2% of one core, most of it first-time hashes of DLLs the agent had not seen since it started; quiet minutes 1.4%. The hash workers vary from run to run (6.1% and 6.1% for the old build, 5.3% and 7.2% for the new under the same load). The hour run judges the workday figures; anything still over budget comes back as a decision. On the merged code (Task 14, F11), with the two runs' quiet and synthetic phases at the same event rates: quiet 3.1% → 1.1% of one core; the synthetic load 13.6% → 10.1% (without the hash workers 6.1% → 3.0%), with 34 and 3 enrichment misses and 3 and 0 Launches without a hash; the working set 105–132 MB → 43–58 MB.
 
+**The hour run** (2026-10-31, 74.7 minutes of normal use on the merged plan 1b-4a-p2 code, AC power, the Silent plan; §15.3): CPU averaged **1.59% of one core**, within the workday budget (per-minute p10 / p50 / p90 / max 0.67 / 0.98 / 3.35 / 7.87%); nothing lost. The working set reached **281 MB**, over budget: the file map kept FileObjects past their Cleanup whose Close Windows sent late or not at all (a `du` walk running in the background), up to its cap. **Plan 1b-4a-p3** (H1, H2): entries leave the map after their Cleanup, and no provisional entry is made that nothing can name. The same 10-minute `du` walk on the host: the file map 231k entries → 4.5k, flat; the working set at most 312 MB → 71 MB. The hour run again judges the working set.
+
 **Fallbacks if over budget** (applied in order, each one a decision-log entry): on-miss seeding off (start-up seeding stays; applied by plan 1b-4a-p2); then `file.op_end` off (Create, Delete and Rename are emitted without failure confirmation, counted); then UDP off; then File Update and SetAttributes off (drops the `FILEIO` and `WRITE` keywords, the largest kernel-side cost; Create/Delete/Rename/Open remain); then registry value reads off (`data_unavailable` on every Value Set); then per-class disable in config as a last resort.
 
 ## 14. Definition of Done
@@ -806,6 +810,14 @@ Plan 1a ([2026-10-02-etw-sensor-spikes-plan](../plans/2026-10-02-etw-sensor-spik
 - **F11, the merged code** (Task 14's script on `main`, 2026-10-27; both at high QoS, on AC with Silent; the quiet phases at 2,073 and 2,071 events/s, the synthetic at 17,509 and 17,295): quiet 3.07% → 1.06% of one core (the seeder 1.6% → 0, the pipeline 0.71% → 0.25%); the synthetic load 13.6% → 10.1% (hash workers 7.5% and 7.1%, pipeline 3.2% → 2.1%, seeder 1.1% → 0), with 34 and 3 enrichment misses and 3 and 0 Launches without a hash; the working set 105–132 MB → 43–58 MB, the `sizes=` line's queues empty between reports. The new build's normal phase was much lighter (108 module loads against 962), so it does not compare.
 - **F10, SetAttributes kept** (R5): Windows' SMB server (`srv2.sys`) works in the System process, so a remote client's changes on this machine's shares also show as PID 4; a filter on PID 4 would hide their timestomping.
 
+**Plan 1b-4a-p3 (2026-10-31): the file map's memory** (the host, build 26200; the p3 notes have the details):
+- **F1, the hour run:** 74.7 minutes of normal use: CPU 1.59% of one core, nothing lost, both unpaired-Cleanup parts 0, 109 enrichment misses in one busy minute; the working set 51 → 281 MB.
+- **F2, where the memory went:** in the `sizes=` lines only the file map grew, 3,000–5,000 entries a minute, quiet minutes included, to its 262,144 cap; each jump in the working set was one doubling of its table (229k → 458k slots: +67 MB).
+- **F3, what it held** (a probe build, never merged): 204,526 of 231,340 entries were files Git Bash's `du` had looked at, 99.99% past their Cleanup with no Close. Windows sends such a Close late or not for a long time (12,000 came at once in one minute); the agent's own hash reads linger the same way. Close (14) reaches the pipeline: nothing is lost.
+- **F4, what reaches a FileObject past its Cleanup** (the synthetic load and five minutes of normal use): no SetAttributes, Rename or SetDelete (41,102 SetAttributes, the System process's bursts included, hit open handles); the Cleanup's outcomes, all within 100 ms; 2,246 late writes, 99.6% within 60 s; Closes, 98% within 100 ms.
+- **F5, provisional entries:** a write on a FileObject never seen opened made one, about 400–600 a minute (17.5k in 30 minutes), none ever cleaned up or closed; with on-miss seeding off nothing names them after the start-up pass.
+- **F6, the fix on the host** (the agent, then `du -s` over the same folder for 10 minutes, then 3 quiet minutes): with H1 the file map 4.6k → 9.6k entries and the working set at most 70 MB (F3's run: 231k and 312 MB); with H2 too, 4,485 → 4,511 entries, at most 71 MB, nothing lost.
+
 ## 16. Known Limitations
 
 - **No boot-time coverage:** events before the agent starts (early boot, or while the service is stopped) are not captured. An AutoLogger session is sub-project 8.
@@ -980,3 +992,8 @@ Plan 1a ([2026-10-02-etw-sensor-spikes-plan](../plans/2026-10-02-etw-sensor-spik
 3. the set of watchlist Opens, keyed by path, keeps SipHash;
 4. the `sizes=` line gives the kernel, user and sink queues too;
 5. a test that could not catch its bug (the expander list's overflow) now can.
+
+**Plan 1b-4a-p3 clarifications (2026-10-31).** The hour run's working set (decision log, 2026-10-31). Applied to §7.1, §9.3, §13 and §15.3:
+1. a file map entry leaves one confirm window after its Cleanup (at the Cleanup without OperationEnds; later while an event waits on it for the seeder), not at its Close; a 60 s record of cleaned FileObjects absorbs late writes;
+2. after the start-up pass, with on-miss seeding off, a write on an unknown handle makes no provisional entry, and `unknown_file_object` does not count its Update;
+3. `sizes=` gives the cleaned FileObjects.
```

- [ ] **Step 2: Commit**

```powershell
git add docs
git commit -m "docs(1): plan 1b-4a-p3 clarifications and findings"
```

### Task 5: PR and merge

- [ ] **Step 1:** Push `feat/1b-4a-p3` and open the PR. The body summarises H1 and H2 with F6 and F7, Task 3's figures, the test counts, and ends with the attribution line.
- [ ] **Step 2:** CI runs `rust-linux`, `rust-windows`, `etw-live`, `agent-live`, `proto`, `powershell` and `audit`. If a test fails only on the runner, fix it with a test, never by skipping.
- [ ] **Step 3:** When every check on the PR's current head is green, merge. Then update the decision log's build row if anything changed on the way, and delete the remote `scratch/p3` branch. The 1b-4b brainstorm follows.

## Review Log

Pending: the independent review.
