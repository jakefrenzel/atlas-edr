//! Process Launch (the join of Kernel-Process 1 and Session B's classic Start,
//! §5.2), Terminate, Module Load, the rundown that seeds the cache (§6.2), and
//! enrichment results (§6.3).

use std::collections::HashMap;

use atlas_etw::parse::{ClassicKind, ClassicProcess, ImageLoad, ProcessStart, ProcessStop};
use atlas_schema::classes::module::{ModuleAction, ModuleActivity};
use atlas_schema::classes::process::ProcessActivity;
use atlas_schema::limits::{CMD_LINE_MAX, USER_NAME_MAX, USER_UID_MAX, truncate_utf8};
use atlas_schema::{Event, EventKind, File, Hashes, Process, ProcessRef, Signature, User};

use super::expand::{Expansion, Slot};
use super::{Pending, Pipeline, alive};
use crate::completion::{PendingId, Reason};
use crate::counters::{Class, Counters};
use crate::input::Header;
use crate::paths::{fit, has_short_name};
use crate::process::{Identity, ProcInfo, ProcessCache, file_name, integrity, start_key};
use crate::services::{EnrichTarget, Lookups, Request};
use crate::time::filetime_to_unix_ns;

/// Launch halves waiting for their partner (§5.2).
#[derive(Default)]
pub struct Launches {
    /// pid → (QPC, the pending Launch, start key): waiting for the classic half.
    kernel: HashMap<u32, (i64, PendingId, u64)>,
    /// pid → the classic half and its header: waiting for the Kernel-Process half.
    classic: HashMap<u32, (i64, ClassicProcess, Header)>,
    enrich: Pending<EnrichTarget>,
}

impl Launches {
    /// The event left the completion stage.
    pub(super) fn forget(&mut self, id: PendingId) {
        self.enrich.take(id);
    }

    #[cfg(test)]
    pub(super) fn sizes(&self) -> [(&'static str, usize); 3] {
        [
            ("enrich", self.enrich.by_id.len()),
            ("kernel halves", self.kernel.len()),
            ("classic halves", self.classic.len()),
        ]
    }
}

impl<L: Lookups> Pipeline<L> {
    pub(super) fn on_process_start(&mut self, h: &Header, s: ProcessStart) {
        let key = start_key(self.id.kernel_boot_id, s.sequence_number);
        let nt = s.image_name.to_string_lossy();
        let parent_key = start_key(self.id.kernel_boot_id, s.parent_sequence_number);
        let parent = match self.procs.get(parent_key) {
            Some(p) => p.to_ref(&self.id),
            None => self.id.bare_ref(parent_key, s.parent_pid),
        };
        let mut info = ProcInfo {
            path: self.dos(&nt),
            created_time: Some(filetime_to_unix_ns(s.create_time)),
            integrity: if s.mandatory_label.is_mandatory_label() {
                s.mandatory_label.rid().and_then(integrity)
            } else {
                None
            },
            parent: Some(parent),
            ..ProcInfo::new(key, s.pid)
        };
        let window = self.ticks.of(self.cfg.join_window);
        let joined = match self.launches.classic.remove(&s.pid) {
            Some((ts, c, _)) if ts.abs_diff(h.ts) <= window.unsigned_abs() => {
                self.apply_classic(&mut info, &c);
                true
            }
            Some(other) => {
                self.launches.classic.insert(s.pid, other);
                false
            }
            None => false,
        };
        self.procs.insert(info.clone(), h.ts);
        let Some(actor) = self.actor_sync(h, Class::Process) else { return };
        self.push_launch(h.ts, actor, &info, &nt, joined, Some((s.pid, key)));
    }

    fn push_launch(
        &mut self,
        ts: i64,
        actor: ProcessRef,
        info: &ProcInfo,
        nt_path: &str,
        joined: bool,
        kernel_half: Option<(u32, u64)>,
    ) {
        let process = launch_process(info, &self.id);
        let ev = self.event(ts, EventKind::Process(ProcessActivity::Launch { actor, process }));
        let mut waits = Vec::new();
        let enrich = !nt_path.is_empty();
        if enrich {
            waits.push(self.wait(Reason::Enrich, self.cfg.enrich_deadline));
        }
        if !joined {
            waits.push(self.wait(Reason::Join, self.cfg.join_deadline));
        }
        let expands = image_expansion(nt_path, Some(info.start_key));
        let Some(id) = self.push_with(ev, waits, expands) else { return };
        if enrich {
            self.launches.enrich.insert(id, EnrichTarget::LaunchImage);
            self.requests.push(Request::Enrich { id, target: EnrichTarget::LaunchImage, nt_path: nt_path.into() });
        }
        if let (false, Some((pid, key))) = (joined, kernel_half) {
            self.launches.kernel.insert(pid, (ts, id, key));
        }
    }

    /// Command line and user from the classic half (§5.2).
    fn apply_classic(&mut self, info: &mut ProcInfo, c: &ClassicProcess) {
        info.cmd_line = Some(c.command_line.to_string_lossy());
        info.user = c.user_sid.as_ref().map(|sid| {
            let uid = sid.to_string();
            let name = self.lookups.account_name(&uid).unwrap_or_default();
            User { uid: fit(uid, USER_UID_MAX), name: fit(name, USER_NAME_MAX) }
        });
    }

    pub(super) fn on_classic(&mut self, h: &Header, c: ClassicProcess) {
        match c.kind {
            ClassicKind::Start => {
                let window = self.ticks.of(self.cfg.join_window);
                match self.launches.kernel.remove(&c.pid) {
                    Some((ts, id, key)) if ts.abs_diff(h.ts) <= window.unsigned_abs() => {
                        let mut info = self.procs.get(key).cloned().unwrap_or_else(|| ProcInfo::new(key, c.pid));
                        self.apply_classic(&mut info, &c);
                        let (cmd, user) = (info.cmd_line.clone(), info.user.clone());
                        self.procs.insert(info, ts);
                        self.completion.update(id, |e| set_launch_details(e, cmd, user));
                        self.completion.resolve(id, Reason::Join);
                    }
                    other => {
                        if let Some(k) = other {
                            self.launches.kernel.insert(c.pid, k);
                        }
                        self.launches.classic.insert(c.pid, (h.ts, c, *h));
                    }
                }
            }
            ClassicKind::DcStart => self.seed_rundown(&c),
            ClassicKind::End | ClassicKind::DcEnd => {}
        }
    }

    /// A process running when Session B started (§6.2): cached, never emitted.
    fn seed_rundown(&mut self, c: &ClassicProcess) {
        if c.pid == 0 {
            return; // Idle is built in
        }
        let Some(live) = self.lookups.live_process(c.pid) else { return };
        let mut info = ProcInfo { path: self.dos(&live.image_path), ..ProcInfo::new(live.start_key, c.pid) };
        self.apply_classic(&mut info, c);
        if info.cmd_line.as_deref().is_none_or(str::is_empty) {
            info.cmd_line = live.command_line;
        }
        self.procs.insert(info, i64::MIN);
    }

    /// Launch halves whose partner did not come within the join window.
    pub(super) fn launch_expiry(&mut self, stream: i64) {
        let window = self.ticks.of(self.cfg.join_window);
        // A Kernel-Process half stays pending in the completion stage until its
        // Join deadline; it just stops accepting a partner.
        self.launches.kernel.retain(|_, (ts, _, _)| ts.saturating_add(window) >= stream);
        let expired: Vec<u32> = self
            .launches
            .classic
            .iter()
            .filter(|(_, (ts, _, _))| ts.saturating_add(window) < stream)
            .map(|(pid, _)| *pid)
            .collect();
        for pid in expired {
            let (ts, c, h) = self.launches.classic.remove(&pid).expect("listed");
            // Only the classic half: the start key must come from the live process.
            let Some(live) = self.lookups.live_process(pid) else {
                self.counters.launch_join_miss += 1;
                Counters::add_class(&mut self.counters.actor_dropped, Class::Process);
                continue;
            };
            if let Some(known) = self.procs.get_mut(live.start_key) {
                // Its Kernel-Process half came, more than the join window away: that
                // Launch stands (its own join miss is counted at its deadline). The
                // cache keeps its integrity and creation time, and gains the details.
                if known.cmd_line.is_none() {
                    known.cmd_line = Some(c.command_line.to_string_lossy());
                }
                if known.user.is_none()
                    && let Some(sid) = &c.user_sid
                {
                    let uid = sid.to_string();
                    let name = self.lookups.account_name(&uid).unwrap_or_default();
                    known.user = Some(User { uid: fit(uid, USER_UID_MAX), name: fit(name, USER_NAME_MAX) });
                }
                continue;
            }
            self.counters.launch_join_miss += 1;
            let mut info = ProcInfo { path: self.dos(&live.image_path), ..ProcInfo::new(live.start_key, pid) };
            self.apply_classic(&mut info, &c);
            info.parent = self.procs.lookup_at(c.parent_pid, ts).map(|p| p.to_ref(&self.id));
            self.procs.insert(info.clone(), ts);
            let Some(actor) = self.actor_sync(&h, Class::Process) else { continue };
            self.push_launch(ts, actor, &info, &live.image_path, true, None);
        }
    }

    pub(super) fn on_process_stop(&mut self, h: &Header, s: ProcessStop) {
        let key = start_key(self.id.kernel_boot_id, s.sequence_number);
        let process = match self.procs.get(key) {
            Some(p) => p.to_ref(&self.id),
            None => {
                let mut r = self.id.bare_ref(key, s.pid);
                r.file.name = String::from_utf8_lossy(&s.image_name).into_owned();
                r
            }
        };
        let ev = self.event(
            h.ts,
            EventKind::Process(ProcessActivity::Terminate { process, exit_code: Some(s.exit_code as i32) }),
        );
        self.completion.push(ev);
        self.procs.end(key, h.ts);
        self.seeding.owner_exited(s.pid);
    }

    pub(super) fn on_image_load(&mut self, h: &Header, i: ImageLoad) {
        // The payload PID is authoritative (§5.3); the header's start key names
        // it when the header is the same process.
        let actor = if h.pid == i.pid {
            self.actor_sync(h, Class::Module)
        } else {
            self.actor_payload(i.pid, h.ts, Class::Module)
        };
        let Some(actor) = actor else { return };
        let nt = i.image_name.to_string_lossy();
        let path = self.dos(&nt);
        let file = File { name: file_name(&path).to_string(), path, hashes: None, signature: None };
        let ev = self.event(
            h.ts,
            EventKind::Module(ModuleActivity {
                actor,
                action: ModuleAction::Load { file, base_address: i.image_base },
            }),
        );
        if nt.is_empty() {
            self.completion.push(ev);
            return;
        }
        let w = self.wait(Reason::Enrich, self.cfg.enrich_deadline);
        let Some(id) = self.push_with(ev, vec![w], image_expansion(&nt, None)) else { return };
        self.launches.enrich.insert(id, EnrichTarget::Module);
        self.requests.push(Request::Enrich { id, target: EnrichTarget::Module, nt_path: nt });
    }

    pub(super) fn on_enriched(&mut self, id: PendingId, hashes: Option<Hashes>, sig: Option<Signature>, error: bool) {
        let Some(target) = self.launches.enrich.take(id) else { return };
        if error {
            self.counters.enrichment_errors += 1;
        }
        // A late result is cached by the workers for the next event (§6.3).
        if !alive(&self.completion, id, Reason::Enrich) {
            return;
        }
        self.completion.update(id, |e| {
            let file = match (&mut e.kind, target) {
                (EventKind::Process(ProcessActivity::Launch { process, .. }), EnrichTarget::LaunchImage) => {
                    &mut process.file
                }
                (EventKind::Module(m), EnrichTarget::Module) => match &mut m.action {
                    ModuleAction::Load { file, .. } => file,
                },
                _ => return,
            };
            file.hashes = hashes;
            file.signature = sig;
        });
        self.completion.resolve(id, Reason::Enrich);
    }
}

/// An image path with 8.3 components is expanded (plan 1b-3a decision D4); for a
/// Launch, the cached process gets the long path too.
fn image_expansion(nt: &str, start_key: Option<u64>) -> Vec<(String, Expansion)> {
    if !has_short_name(nt) {
        return Vec::new();
    }
    vec![(nt.to_string(), Expansion { slot: Slot::Image, image_key: start_key, ..Expansion::file(None) })]
}

fn launch_process(info: &ProcInfo, id: &Identity) -> Process {
    let r = info.to_ref(id);
    let (cmd_line, cmd_line_truncated) = truncate_cmd(info.cmd_line.as_deref().unwrap_or_default());
    Process {
        uid: r.uid,
        pid: r.pid,
        file: r.file,
        user: r.user,
        cmd_line,
        cmd_line_truncated,
        created_time: info.created_time.unwrap_or(0),
        integrity: info.integrity,
        parent_process: info.parent.clone(),
    }
}

fn truncate_cmd(s: &str) -> (String, bool) {
    let (t, cut) = truncate_utf8(s, CMD_LINE_MAX);
    (t.to_string(), cut)
}

fn set_launch_details(e: &mut Event, cmd: Option<String>, user: Option<User>) {
    if let EventKind::Process(ProcessActivity::Launch { process, .. }) = &mut e.kind {
        let (c, cut) = truncate_cmd(cmd.as_deref().unwrap_or_default());
        process.cmd_line = c;
        process.cmd_line_truncated = cut;
        process.user = user;
    }
}

/// A Launch whose classic half never came: the command line from the live
/// process, if it is still running and is the same process (§5.2).
pub(super) fn join_fallback<L: Lookups>(e: &mut Event, lookups: &mut L, procs: &mut ProcessCache, id: &Identity) {
    let EventKind::Process(ProcessActivity::Launch { process, .. }) = &mut e.kind else { return };
    let Some(live) = lookups.live_process(process.pid) else { return };
    if id.uid(live.start_key) != process.uid {
        return;
    }
    if let Some(cmd) = live.command_line {
        let (c, cut) = truncate_cmd(&cmd);
        process.cmd_line = c;
        process.cmd_line_truncated = cut;
        if let Some(info) = procs.get_mut(live.start_key) {
            info.cmd_line = Some(cmd);
        }
    }
}
