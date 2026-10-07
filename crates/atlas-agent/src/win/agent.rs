//! Starting and stopping the sensor (sensor spec §3.2, §11.4; plan 1b-3c,
//! decisions D1 and D4): identity, the two sessions and their consumers, the
//! callbacks' intakes, the Windows services, and the pipeline thread running
//! [`crate::driver::Driver`] into a sink.
//!
//! Plan 1b-4 adds the buffer writer as the sink, the watchdog, Sensor Health,
//! the service and the CLI around this.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel, sync_channel};
use std::thread::JoinHandle;
use std::time::Duration;

use atlas_etw::providers;
use atlas_etw::session::{self, Consumer, EtwError, EventRecord, Session, VersionGate};
use atlas_schema::{Event, EventId};

use super::identity::{self, IdentityError, Started};
use super::lookups::WinLookups;
use super::services::Services;
use super::util::qpc_now;
use crate::config::{Config, ServiceConfig};
use crate::counters::{Counters, IntakeCounters, ServiceCounters};
use crate::driver::{Control, Driver, DriverConfig};
use crate::input::{Header, Session as Which};
use crate::intake::{Intake, Queues};
use crate::pipeline::{Pipeline, Setup};
use crate::watchlist::BadPattern;

/// What to start.
#[derive(Debug, Clone)]
pub struct AgentOptions {
    /// Session A (the manifest providers) and Session B (the system logger's
    /// process events). Tests use `Atlas-Test-*` names (§12.3).
    pub sensor_session: String,
    pub process_session: String,
    pub config: Config,
    pub services: ServiceConfig,
    pub driver: DriverConfig,
    /// `device.json` (§6.1).
    pub device_file: PathBuf,
}

#[derive(Debug)]
pub enum AgentError {
    Identity(IdentityError),
    Etw(EtwError),
    Watchlist(BadPattern),
    /// A configuration `check()` refused.
    Config(String),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentError::Identity(e) => write!(f, "{e}"),
            AgentError::Etw(e) => write!(f, "{e}"),
            AgentError::Watchlist(e) => write!(f, "watchlist: {e:?}"),
            AgentError::Config(e) => write!(f, "configuration: {e}"),
        }
    }
}

impl std::error::Error for AgentError {}

/// The running sensor. [`Running::stop`] stops it cleanly; dropping it stops
/// it without waiting for the hold.
pub struct Running {
    sensor: Option<Session>,
    process: Option<Session>,
    consumers: Vec<Consumer>,
    /// Senders kept so the queues stay open while a session is restarted
    /// (plan 1b-4's watchdog); dropped at stop, which ends the pipeline loop.
    spare: Option<Queues>,
    pipeline: Option<JoinHandle<Counters>>,
    control: Sender<Control<WinLookups>>,
    intake: Arc<IntakeCounters>,
    services: Arc<ServiceCounters>,
    seeding: bool,
    started: Started,
    /// The pipeline's settings as started (seeding turned off without
    /// `SeDebugPrivilege`): what a replacement `Intake` needs (plan 1b-4).
    config: Config,
}

/// What a stopped sensor counted.
#[derive(Debug)]
pub struct Stopped {
    pub pipeline: Counters,
    pub intake: Arc<IntakeCounters>,
    pub services: Arc<ServiceCounters>,
    /// Callbacks that panicked, both sessions (`loss.callback_panics`).
    pub callback_panics: u64,
}

/// Starts the sensor (§11.4): identity, the services, both sessions and their
/// consumers, then the pipeline thread. The pipeline's start-up seeding pass
/// is asked for on its first pass, after the sessions started (§3.2).
pub fn start(opts: AgentOptions, sink: impl FnMut(Event) + Send + 'static) -> Result<Running, AgentError> {
    opts.services.check().map_err(|e| AgentError::Config(e.to_string()))?;
    let started = identity::start(&opts.device_file).map_err(AgentError::Identity)?;
    let services = Services::start(&opts.services);
    let mut config = opts.config.clone();
    if !services.seeding() {
        config.seed_on_start = false;
        config.seed_on_miss = false;
    }
    let setup = Setup {
        config: config.clone(),
        ticks: started.ticks,
        anchor: started.anchor,
        identity: started.identity,
        current_control_set: started.current_control_set,
        self_keys: vec![started.self_key],
        started: started.started,
    };
    let pipeline =
        Pipeline::new(setup, WinLookups::new(), Box::new(|_| EventId::new_v7())).map_err(AgentError::Watchlist)?;

    let (ktx, krx) = sync_channel(opts.driver.kernel_queue_cap);
    let (utx, urx) = sync_channel(opts.driver.user_queue_cap);
    let queues = || Queues { kernel: ktx.clone(), user: utx.clone() };
    let intake_counters = Arc::new(IntakeCounters::default());
    let self_keys = [started.self_key];
    let fast_read = config.registry_value_reads.then(|| services.fast_read());
    let sensor_intake =
        Intake::new(Which::Sensor, &config, started.ticks, queues(), intake_counters.clone(), fast_read, &self_keys);
    let process_intake =
        Intake::new(Which::Process, &config, started.ticks, queues(), intake_counters.clone(), None, &self_keys);

    let sensor = Session::start(&session::Config::session_a(&opts.sensor_session)).map_err(AgentError::Etw)?;
    for e in providers::session_a(config.network_udp) {
        sensor.enable(&e).map_err(AgentError::Etw)?;
    }
    let process = Session::start(&session::Config::session_b(&opts.process_session)).map_err(AgentError::Etw)?;
    let consumers = vec![
        session::consume(&opts.sensor_session, callback(Which::Sensor, sensor_intake)).map_err(AgentError::Etw)?,
        session::consume(&opts.process_session, callback(Which::Process, process_intake)).map_err(AgentError::Etw)?,
    ];

    let service_counters = services.counters();
    let seeding = services.seeding();
    let (control, control_rx) = channel();
    let driver =
        Driver::new(pipeline, services, krx, urx, Box::new(qpc_now), Box::new(identity::anchor), opts.driver.clone())
            .with_control(control_rx);
    let pipeline = std::thread::Builder::new()
        .name("atlas-pipeline".into())
        .spawn(move || driver.run(sink))
        .expect("spawn the pipeline thread");
    Ok(Running {
        sensor: Some(sensor),
        process: Some(process),
        consumers,
        spare: Some(queues()),
        pipeline: Some(pipeline),
        control,
        intake: intake_counters,
        services: service_counters,
        seeding,
        started,
        config,
    })
}

/// The ETW callback (§3.2 [1]): parse with the version check, then the intake.
fn callback(which: Which, mut intake: Intake) -> impl FnMut(&EventRecord) + Send + 'static {
    let mut gate = VersionGate::new();
    move |rec| {
        let header =
            Header { session: which, pid: rec.pid(), tid: rec.tid(), ts: rec.timestamp(), start_key: rec.start_key() };
        intake.on_event(header, gate.parse(rec));
    }
}

impl Running {
    /// Whether the seeder runs (`housekeeping.seeding_enabled`).
    pub fn seeding(&self) -> bool {
        self.seeding
    }

    pub fn started(&self) -> &Started {
        &self.started
    }

    pub fn intake_counters(&self) -> &Arc<IntakeCounters> {
        &self.intake
    }

    pub fn service_counters(&self) -> &Arc<ServiceCounters> {
        &self.services
    }

    /// The pipeline's settings as started.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Work to run on the pipeline thread between two passes (`driver::Control`):
    /// the canary's self key, Sensor Health reads (plan 1b-4).
    pub fn control(&self) -> Sender<Control<WinLookups>> {
        self.control.clone()
    }

    /// Whether the pipeline thread still runs: if it panicked, the callbacks'
    /// sends fail and nothing is emitted (the watchdog's check, plan 1b-4).
    pub fn pipeline_running(&self) -> bool {
        self.pipeline.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// A clean stop (§11.4): flush both sessions, wait out the hold, stop the
    /// sessions so the consumers deliver the rest and finish, then let the
    /// pipeline drain and stop. No event logged before this call is lost.
    pub fn stop(mut self) -> Stopped {
        for s in [&self.sensor, &self.process].into_iter().flatten() {
            let _ = s.flush();
        }
        std::thread::sleep(self.config.hold);
        self.finish()
    }

    fn finish(&mut self) -> Stopped {
        for s in [self.sensor.take(), self.process.take()].into_iter().flatten() {
            let _ = s.stop();
        }
        // A stopped session's consumer delivers what is left and finishes; its
        // panic count is read after that (review R-m7).
        let t0 = std::time::Instant::now();
        while self.consumers.iter().any(|c| !c.is_finished()) && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let callback_panics = self.consumers.iter().map(Consumer::panics).sum();
        // Closing a consumer drops its callback, and with it the intake's senders
        // and Session A's `FastRead`: before the services go (R-m11).
        for c in self.consumers.drain(..) {
            c.close();
        }
        self.spare = None;
        let pipeline = self.pipeline.take().map(|t| t.join().expect("the pipeline thread")).unwrap_or_default();
        Stopped { pipeline, intake: self.intake.clone(), services: self.services.clone(), callback_panics }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if self.pipeline.is_some() {
            self.finish();
        }
    }
}
