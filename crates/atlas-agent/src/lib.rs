//! The Atlas agent (sensor spec §3).

pub mod cleanup;
pub mod completion;
pub mod config;
pub mod counters;
pub mod evict;
pub mod fakes;
pub mod input;
pub mod intake;
pub mod keymap;
pub mod ordering;
pub mod paths;
pub mod pipeline;
pub mod process;
pub mod recent;
pub mod services;
pub mod time;
pub mod watchlist;
#[cfg(windows)]
pub mod win;
