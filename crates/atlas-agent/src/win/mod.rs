//! The Windows services behind [`crate::services`] (sensor spec §5–§7; plan 1b-3b).
//! All of the agent's Windows `unsafe` code lives in this module, behind a safe API.
//!
//! - [`identity`]: device and boot identity, the anchor, and `Setup`'s facts.
//! - [`lookups::WinLookups`]: the [`crate::services::Lookups`] the pipeline thread calls.
//! - [`Services`]: the hash workers, the reader and expander lanes and the seeder, answering
//!   [`crate::services::Request`]s.

// The structure offsets used here (handle table, telemetry, process list,
// directory entries) are the x64 layouts.
#[cfg(not(target_pointer_width = "64"))]
compile_error!("atlas-agent's Windows services support 64-bit Windows only");

mod expand;
mod handles;
mod hash;
pub mod identity;
pub mod lookups;
pub mod privilege;
mod seeder;
mod services;
mod telemetry;
mod util;
mod value;

pub use services::Services;
