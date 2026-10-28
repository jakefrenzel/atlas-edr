//! `atlas-agent run` (sensor spec §11.3, §11.4; plan 1b-4a): the sensor in a
//! console, until Ctrl+C, Ctrl+Break or the console closes, then a clean stop.
//!
//! Start: an elevated token → the single-instance lock → the data directory
//! (created or verified, §11.1) → `agent.toml` → the diagnostic log → the
//! buffer → power throttling turned off ([`power`], plan 1b-4a-p2, R8) → the
//! sensor ([`agent::start`]) with the buffer writer as its sink → the health
//! thread.
//!
//! Clean stop: the health thread → the sensor ([`agent::Running::stop`]) →
//! the final Sensor Health report → the buffer, flushed.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::sync::{Mutex, Once, PoisonError};
use std::time::{Duration, Instant};

use atlas_schema::{Device, Event};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, SetConsoleCtrlHandler};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::BOOL;

use super::agent::{self, AgentOptions};
use super::datadir;
use super::health::{self, HealthConfig, HealthThread, Interval, Reporting};
use super::instance;
use super::power;
use super::util::Owned;
use crate::config::Config;
use crate::driver::DriverConfig;
use crate::health::Snapshot;
use crate::settings::{LogLevel, Settings};
use crate::store::{Sink, Store, StoreConfig};

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub data_dir: PathBuf,
    pub sensor_session: String,
    pub process_session: String,
    pub health: HealthConfig,
    /// Tests: the buffer writer thread panics after this many appends.
    #[cfg(test)]
    pub store_panics_after: Option<u64>,
}

impl RunOptions {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        RunOptions {
            data_dir: data_dir.into(),
            sensor_session: "Atlas-Sensor".into(),
            process_session: "Atlas-Process".into(),
            health: HealthConfig::default(),
            #[cfg(test)]
            store_panics_after: None,
        }
    }
}

/// Exit codes.
pub const EXIT_OK: i32 = 0;
/// Start-up was refused (not elevated, another instance, the directory, the
/// settings, the buffer, the sessions).
pub const EXIT_START: i32 = 1;
/// The pipeline or the buffer writer thread ended while running.
pub const EXIT_PIPELINE: i32 = 2;

/// Runs until stopped; returns the process exit code.
pub fn run(opts: &RunOptions) -> i32 {
    if !elevated() {
        eprintln!("atlas-agent run needs an elevated (administrator) prompt");
        return EXIT_START;
    }
    let _instance = match instance::acquire() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("atlas-agent: {e}");
            return EXIT_START;
        }
    };
    if let Err(e) = datadir::prepare(&opts.data_dir) {
        eprintln!("atlas-agent: {e}");
        return EXIT_START;
    }
    let settings = Settings::load(&opts.data_dir.join("agent.toml"));
    let level = settings.as_ref().map_or(LogLevel::Info, |s| s.log_level);
    let _log = init_logging(&opts.data_dir.join("logs"), level);
    let settings = match settings {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("agent.toml refused: {e}");
            return EXIT_START;
        }
    };
    tracing::info!(version = env!("CARGO_PKG_VERSION"), data_dir = %opts.data_dir.display(), "atlas-agent starting");
    let code = run_sensor(opts, &settings);
    tracing::info!(code, "atlas-agent stopped");
    // The log file is written last; only then may a console close end the
    // process (it waits for `STOPPED`).
    drop(_log);
    STOPPED.store(true, Ordering::SeqCst);
    code
}

fn run_sensor(opts: &RunOptions, settings: &Settings) -> i32 {
    run_with_sink(opts, settings, |sink| move |e| sink.event(e))
}

/// [`run_sensor`] with the pipeline's sink made from the buffer's by `sink`
/// (tests make it fail).
fn run_with_sink<S>(opts: &RunOptions, settings: &Settings, sink: impl FnOnce(Sink) -> S) -> i32
where
    S: FnMut(Event) + Send + 'static,
{
    let buffer_dir = opts.data_dir.join("buffer");
    let buffer = atlas_buffer::Config {
        cap_bytes: settings.buffer.cap_bytes,
        overflow: settings.buffer.overflow,
        ..atlas_buffer::Config::new(&buffer_dir)
    };
    #[allow(unused_mut)]
    let mut store_cfg = StoreConfig::new(buffer);
    #[cfg(test)]
    {
        store_cfg.panic_after = opts.store_panics_after;
    }
    let (store, recovery) = match Store::start(store_cfg) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("the buffer could not be opened: {e}");
            return EXIT_START;
        }
    };
    if recovery != atlas_buffer::Recovery::default() {
        tracing::warn!(?recovery, "the buffer was repaired at open");
    }

    let sink = sink(store.sink());
    let agent_opts = AgentOptions {
        sensor_session: opts.sensor_session.clone(),
        process_session: opts.process_session.clone(),
        config: settings.config.clone(),
        services: settings.services.clone(),
        driver: DriverConfig::default(),
        device_file: opts.data_dir.join("device.json"),
    };
    // Before the sessions start: Ctrl+C during start-up must not end the
    // process with them running. A stop asked for meanwhile waits in `stop`.
    let stop = install_ctrl_handler();
    // SAFETY: GetCurrentProcess returns a pseudo-handle; nothing to close.
    if let Err(e) = power::run_at_full_speed(unsafe { GetCurrentProcess() }) {
        tracing::warn!("power throttling could not be turned off: {e}");
    }
    tracing::info!("starting the sensor");
    let running = match agent::start(agent_opts, sink) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("the sensor could not start: {e}");
            let _ = store.close();
            return EXIT_START;
        }
    };
    let started = running.started().clone();
    if let Some(kusd) = started.boot_id_disagreement {
        tracing::warn!(kusd, "the boot id in KUSER_SHARED_DATA disagreed with the kernel's; the kernel's is used");
    }
    log_settings(&running.config().clone(), running.seeding());

    let seeding = running.seeding();
    let reporting = Reporting {
        to: store.health(),
        device: Device { uid: started.identity.device, boot_id: started.identity.boot },
        buffer_drops: store.drops(),
    };
    let first = Interval { from: Snapshot::at_start(seeding, started.anchor.unix_ns), start: started.anchor.unix_ns };
    let mut sources = running.health_sources();
    sources.queues.push(("sink", store.health().gauge()));
    let health = HealthThread::start(sources, reporting.clone(), first, opts.health);

    tracing::info!("running; Ctrl+C stops");
    let mut code = EXIT_OK;
    loop {
        match stop.recv_timeout(Duration::from_secs(1)) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) if !running.pipeline_running() => {
                tracing::error!("the pipeline thread ended; stopping");
                code = EXIT_PIPELINE;
                break;
            }
            Err(RecvTimeoutError::Timeout) if !store.is_running() => {
                tracing::error!("the buffer writer thread ended; stopping");
                code = EXIT_PIPELINE;
                break;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    }

    tracing::info!("stopping");
    let t0 = Instant::now();
    let at = health.stop();
    let drops = store.drops();
    // Listed before the sensor stops, while its threads still run (review m2).
    let listing = health::threads();
    let stopped = running.stop();
    if let Err(panic) = &stopped.pipeline {
        // The default hook printed it to stderr only; a service has none.
        tracing::error!("the pipeline thread panicked: {panic}");
    }
    let last = health::final_snapshot(&stopped, &drops, seeding, listing);
    health::send(&reporting, at, &last);
    drop(reporting);
    match store.close() {
        Ok(stats) => tracing::info!(
            records = stats.records_written,
            bytes = stats.bytes_written,
            stop_ms = t0.elapsed().as_millis() as u64,
            "buffer closed"
        ),
        Err(e) => tracing::error!("the buffer could not be flushed at stop: {e}"),
    }
    code
}

fn log_settings(c: &Config, seeding: bool) {
    tracing::info!(
        hold_ms = c.hold.as_millis() as u64,
        udp = c.network_udp,
        op_end = c.file_op_end,
        value_reads = c.registry_value_reads,
        seeding,
        "sensor started"
    );
}

/// The diagnostic log (§11.4): the console, and `logs\` rolled daily, seven
/// files kept. The guard flushes the file at the end of `run`.
fn init_logging(dir: &Path, level: LogLevel) -> Option<WorkerGuard> {
    let level = match level {
        LogLevel::Error => LevelFilter::ERROR,
        LogLevel::Warn => LevelFilter::WARN,
        LogLevel::Info => LevelFilter::INFO,
        LogLevel::Debug => LevelFilter::DEBUG,
        LogLevel::Trace => LevelFilter::TRACE,
    };
    let console = fmt::layer().with_writer(std::io::stderr).with_target(false);
    let file = Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix("atlas-agent")
        .filename_suffix("log")
        .max_log_files(7)
        .build(dir);
    let (file_layer, guard) = match file {
        Ok(appender) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            (Some(fmt::layer().with_writer(writer).with_ansi(false).with_target(false)), Some(guard))
        }
        Err(e) => {
            eprintln!("atlas-agent: the log file in {} cannot be opened: {e}", dir.display());
            (None, None)
        }
    };
    let _ = tracing_subscriber::registry().with(level).with(console).with(file_layer).try_init();
    guard
}

fn elevated() -> bool {
    let mut token = HANDLE::default();
    // SAFETY: opens our own token for query; closed by `Owned`.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }
    let token = Owned(token);
    let mut e = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    // SAFETY: `e` is the structure TokenElevation returns, of the size given.
    let ok = unsafe {
        GetTokenInformation(
            token.raw(),
            TokenElevation,
            Some((&raw mut e).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    };
    ok.is_ok() && e.TokenIsElevated != 0
}

/// Where the console handler sends a stop: the latest `run`'s channel.
static STOP: Mutex<Option<Sender<()>>> = Mutex::new(None);
static HANDLER: Once = Once::new();
/// Set once the clean stop is done and the log file written: a console
/// close waits for it.
static STOPPED: AtomicBool = AtomicBool::new(false);
/// Windows ends the process about 5 s after a close event.
const CLOSE_WAIT: Duration = Duration::from_millis(4_500);

/// Ctrl+C, Ctrl+Break and closing the console all ask for the clean stop.
fn install_ctrl_handler() -> std::sync::mpsc::Receiver<()> {
    let (tx, rx) = channel();
    // A later `run` in one process (tests) replaces the channel.
    *STOP.lock().unwrap_or_else(PoisonError::into_inner) = Some(tx);
    STOPPED.store(false, Ordering::SeqCst);
    HANDLER.call_once(|| {
        // SAFETY: `on_ctrl` is a valid handler for the life of the process.
        if let Err(e) = unsafe { SetConsoleCtrlHandler(Some(on_ctrl), true) } {
            tracing::warn!("the console handler could not be installed: {e}");
        }
    });
    rx
}

unsafe extern "system" fn on_ctrl(kind: u32) -> BOOL {
    match kind {
        CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT => {
            if let Some(tx) = STOP.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                let _ = tx.send(());
            }
            if kind == CTRL_CLOSE_EVENT {
                // Returning ends the process: wait for the clean stop.
                let t0 = Instant::now();
                while !STOPPED.load(Ordering::SeqCst) && t0.elapsed() < CLOSE_WAIT {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            true.into()
        }
        _ => false.into(),
    }
}

#[cfg(test)]
mod tests {
    use atlas_buffer::{Cursor, Reader};
    use atlas_schema::EventKind;
    use atlas_schema::classes::sensor_health::SensorHealthAction;

    use super::*;

    /// Removes the test's data directory, also on failure.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    #[ignore = "needs administrator rights; CI's agent-live job runs it"]
    fn a_pipeline_that_dies_still_gets_its_final_report_and_a_flushed_buffer() {
        const WRITTEN: u64 = 20;
        let pid = std::process::id();
        let dir = Scratch(std::env::temp_dir().join(format!("atlas-run-dies-{pid}")));
        let _ = std::fs::remove_dir_all(&dir.0);
        std::fs::create_dir_all(&dir.0).unwrap();
        let mut opts = RunOptions::new(&dir.0);
        opts.sensor_session = format!("Atlas-Test-Run-A-{pid}");
        opts.process_session = format!("Atlas-Test-Run-B-{pid}");

        // The start-up seeding alone makes far more than WRITTEN events.
        let code = run_with_sink(&opts, &Settings::default(), |sink| {
            let mut n = 0;
            move |e| {
                n += 1;
                assert!(n <= WRITTEN, "test: the sink fails on event {n}");
                sink.event(e);
            }
        });
        assert_eq!(code, EXIT_PIPELINE);

        let mut reader = Reader::open(&dir.0.join("buffer"), Cursor::default());
        let mut events = 0;
        let mut last = None;
        while let Some(rec) = reader.next_record().unwrap() {
            let e = atlas_schema::decode_event(&rec.payload).expect("a valid record");
            if !matches!(e.kind, EventKind::SensorHealth(_)) {
                events += 1;
            }
            last = Some(e);
        }
        assert_eq!(events, WRITTEN, "every event the sink took was flushed");
        let Some(EventKind::SensorHealth(health)) = last.map(|e| e.kind) else {
            panic!("the last record is not the final Sensor Health report");
        };
        let SensorHealthAction::Report(report) = health.action;
        // The pipeline's counters died with it: absent, not made up.
        assert_eq!(report.quality.late_arrivals, None);
    }

    #[test]
    #[ignore = "needs administrator rights; CI's agent-live job runs it"]
    fn a_buffer_writer_that_dies_stops_the_agent() {
        let pid = std::process::id();
        let dir = Scratch(std::env::temp_dir().join(format!("atlas-run-writer-dies-{pid}")));
        let _ = std::fs::remove_dir_all(&dir.0);
        std::fs::create_dir_all(&dir.0).unwrap();
        let mut opts = RunOptions::new(&dir.0);
        opts.sensor_session = format!("Atlas-Test-Run-C-{pid}");
        opts.process_session = format!("Atlas-Test-Run-D-{pid}");
        opts.store_panics_after = Some(20);

        let (tx, rx) = channel();
        let runner = std::thread::spawn(move || {
            let _ = tx.send(run_with_sink(&opts, &Settings::default(), |sink| move |e| sink.event(e)));
        });
        let code = rx.recv_timeout(Duration::from_secs(60)).ok();
        if code.is_none() {
            // Never noticed: stop it, so the test ends.
            if let Some(tx) = STOP.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                let _ = tx.send(());
            }
        }
        runner.join().unwrap();
        assert_eq!(code, Some(EXIT_PIPELINE), "the writer's end was noticed within 60 s");
    }
}
