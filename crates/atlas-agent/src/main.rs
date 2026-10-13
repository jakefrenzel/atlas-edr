//! The `atlas-agent` command line (sensor spec §11.3). Plan 1b-4a has `run`
//! and `dump`; 1b-4b adds `canary`, 1b-4c `service`.

use std::path::PathBuf;
use std::process::ExitCode;

use atlas_agent::dump::{self, ClassName, DumpOptions};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "atlas-agent", version, about = "The Atlas endpoint agent")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// `C:\ProgramData\Atlas` (§11.1).
const DATA_DIR: &str = r"C:\ProgramData\Atlas";

#[derive(Subcommand)]
enum Command {
    /// Runs the sensor in this console until Ctrl+C (administrator required).
    Run {
        /// The data directory: created protected, or verified (§11.1).
        #[arg(long, default_value = DATA_DIR)]
        data_dir: PathBuf,
    },
    /// Prints the buffer as JSON lines, one event per line. Never acks.
    Dump {
        #[arg(long, default_value = DATA_DIR)]
        data_dir: PathBuf,
        /// Keeps printing new events until Ctrl+C.
        #[arg(long)]
        follow: bool,
        /// Starts after this position, as `<segment:offset>`.
        #[arg(long, value_parser = dump::parse_cursor)]
        from: Option<atlas_buffer::Cursor>,
        /// Only this class: process, module, network, file, registry_key,
        /// registry_value, dns, event_log, sensor_health.
        #[arg(long)]
        class: Option<ClassName>,
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Run { data_dir } => run(data_dir),
        Command::Dump { data_dir, follow, from, class } => {
            let opts = DumpOptions { from: from.unwrap_or_default(), class, follow };
            let out = std::io::stdout();
            match dump::dump(&data_dir.join("buffer"), &opts, &mut out.lock(), || false) {
                Ok(s) => {
                    eprintln!(
                        "{} events; {} invalid records skipped; {} corrupt segments; next: {}:{}",
                        s.printed, s.invalid, s.corrupt_segments, s.next.segment, s.next.offset
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("atlas-agent dump: {}: {e}", data_dir.display());
                    if e.kind() == std::io::ErrorKind::PermissionDenied {
                        eprintln!("the data directory is readable by administrators only");
                    }
                    ExitCode::FAILURE
                }
            }
        }
    }
}

#[cfg(windows)]
fn run(data_dir: PathBuf) -> ExitCode {
    let code = atlas_agent::win::run::run(&atlas_agent::win::run::RunOptions::new(data_dir));
    ExitCode::from(code as u8)
}

#[cfg(not(windows))]
fn run(_: PathBuf) -> ExitCode {
    eprintln!("atlas-agent run needs Windows");
    ExitCode::FAILURE
}
