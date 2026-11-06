//! Scratch probe for plan 1b-4b-1's brainstorm, never for `main`: what a session
//! restart really costs. Elevated; CI's `b4-probe` workflow runs it.
#![cfg(windows)]

use atlas_etw::providers::session_a;
use atlas_etw::session::{self, Config, Consumer, EventRecord, Session};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};

const SA: &str = "Atlas-Probe-Sensor";
const SB: &str = "Atlas-Probe-Process";

#[derive(Default, Clone)]
struct Seen {
    count: Arc<AtomicU64>,
    /// Events other than rundown: Session B's Start and End, all of Session A's.
    live: Arc<AtomicU64>,
    first_live: Arc<AtomicI64>,
    last_live: Arc<AtomicI64>,
    rundown: Arc<AtomicU64>,
    first_rundown_ms: Arc<AtomicU64>,
    last_rundown_ms: Arc<AtomicU64>,
}

fn watch(seen: Seen, is_b: bool, born: Instant) -> impl FnMut(&EventRecord) + Send + 'static {
    move |rec| {
        seen.count.fetch_add(1, Ordering::Relaxed);
        let op = rec.opcode();
        if is_b && op != 1 && op != 2 {
            if op == 3 {
                let ms = born.elapsed().as_millis() as u64;
                if seen.rundown.fetch_add(1, Ordering::Relaxed) == 0 {
                    seen.first_rundown_ms.store(ms, Ordering::Relaxed);
                }
                seen.last_rundown_ms.store(ms, Ordering::Relaxed);
            }
            return;
        }
        let ts = rec.timestamp();
        if seen.live.fetch_add(1, Ordering::Relaxed) == 0 {
            seen.first_live.store(ts, Ordering::Relaxed);
        }
        seen.last_live.store(ts, Ordering::Relaxed);
    }
}

fn start(is_b: bool) -> Session {
    if is_b {
        Session::start(&Config::session_b(SB)).expect("start B")
    } else {
        let s = Session::start(&Config::session_a(SA)).expect("start A");
        for e in session_a(true, true) {
            s.enable(&e).expect("enable");
        }
        s
    }
}

fn wait(mut done: impl FnMut() -> bool, limit: Duration) -> Option<Duration> {
    let t = Instant::now();
    while t.elapsed() < limit {
        if done() {
            return Some(t.elapsed());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    None
}

fn ms(d: Option<Duration>) -> String {
    d.map_or("null".into(), |d| format!("{:.1}", d.as_secs_f64() * 1000.0))
}

fn consume(is_b: bool, seen: &Seen, born: Instant) -> Consumer {
    let name = if is_b { SB } else { SA };
    session::consume(name, watch(seen.clone(), is_b, born)).expect("consume")
}

fn round(is_b: bool, busy: bool, n: u32) {
    let name = if is_b { SB } else { SA };
    let old = start(is_b);
    let id_old = old.logger_id();
    let seen = Seen::default();
    let consumer = consume(is_b, &seen, Instant::now());
    let freq = consumer.qpc_frequency();
    std::thread::sleep(Duration::from_millis(1500));

    // Half the rounds stop it as an attacker would, through `logman`.
    let external = n % 2 == 1;
    let before = seen.count.load(Ordering::Relaxed);
    let t0 = Instant::now();
    if external {
        let out = std::process::Command::new("logman").args(["stop", name, "-ets"]).output().expect("logman");
        assert!(out.status.success(), "logman: {out:?}");
    } else {
        session::stop_by_name(name).expect("stop");
    }
    let t_call = t0.elapsed();
    let noticed = wait(|| consumer.is_finished(), Duration::from_secs(10)).map(|d| d + t_call);
    let after = seen.count.load(Ordering::Relaxed);
    let last = seen.last_live.load(Ordering::Relaxed);
    old.abandon();

    // Half the rounds start the new session before closing the old consumer.
    let close_first = n % 4 < 2;
    let mut status = None;
    let mut waiting = Some(consumer);
    if close_first {
        status = Some(waiting.take().unwrap().close());
    }
    let t1 = Instant::now();
    let new = start(is_b);
    let t_started = t1.elapsed();
    let seen2 = Seen::default();
    let consumer2 = consume(is_b, &seen2, t1);
    let t_consuming = t1.elapsed();
    let first = wait(|| seen2.live.load(Ordering::Relaxed) > 0, Duration::from_secs(5)).map(|d| d + t_consuming);
    if let Some(c) = waiting.take() {
        status = Some(c.close());
    }
    std::thread::sleep(Duration::from_millis(1200));
    let hole = (seen2.first_live.load(Ordering::Relaxed) - last) as f64 * 1000.0 / freq as f64;
    println!(
        "PROBE {{\"session\":\"{}\",\"busy\":{busy},\"round\":{n},\"external\":{external},\"close_first\":{close_first},\
         \"id_old\":{id_old},\"id_new\":{},\"stop_call_ms\":{:.1},\"noticed_ms\":{},\"events_after_call\":{},\
         \"restart_session_ms\":{:.1},\"consumer_ms\":{:.1},\"first_event_ms\":{},\"hole_ms\":{hole:.1},\
         \"old_status\":{},\"rundown\":{},\"rundown_first_ms\":{},\"rundown_last_ms\":{},\"events_new\":{}}}",
        if is_b { "B" } else { "A" },
        new.logger_id(),
        t_call.as_secs_f64() * 1000.0,
        ms(noticed),
        after.saturating_sub(before),
        t_started.as_secs_f64() * 1000.0,
        t_consuming.as_secs_f64() * 1000.0,
        ms(first),
        status.unwrap_or(u32::MAX),
        seen2.rundown.load(Ordering::Relaxed),
        seen2.first_rundown_ms.load(Ordering::Relaxed),
        seen2.last_rundown_ms.load(Ordering::Relaxed),
        seen2.count.load(Ordering::Relaxed),
    );
    new.stop().expect("stop the new session");
    consumer2.close();
}

#[test]
#[ignore = "needs administrator rights"]
fn restart_costs() {
    let stop = Arc::new(AtomicBool::new(false));
    // Activity for both sessions: files, and a process every 100 ms.
    let churn = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let p = std::env::temp_dir().join("atlas-b4-probe.txt");
            let mut i = 0u32;
            while !stop.load(Ordering::Relaxed) {
                let _ = std::fs::write(&p, b"x");
                let _ = std::fs::remove_file(&p);
                i += 1;
                if i % 20 == 0 {
                    let _ = std::process::Command::new("cmd.exe").args(["/c", "exit"]).status();
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        })
    };
    for busy in [false, true] {
        let spin = Arc::new(AtomicBool::new(busy));
        let spinners: Vec<_> = (0..if busy { std::thread::available_parallelism().unwrap().get() } else { 0 })
            .map(|_| {
                let spin = spin.clone();
                std::thread::spawn(move || {
                    let mut x = 1u64;
                    while spin.load(Ordering::Relaxed) {
                        x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
                    }
                    std::hint::black_box(x);
                })
            })
            .collect();
        for n in 0..8 {
            round(false, busy, n);
            round(true, busy, n);
        }
        spin.store(false, Ordering::Relaxed);
        for t in spinners {
            t.join().unwrap();
        }
    }
    stop.store(true, Ordering::Relaxed);
    churn.join().unwrap();
}
