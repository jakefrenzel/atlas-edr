//! The Atlas agent (sensor spec §3).

pub mod cleanup;
pub mod completion;
pub mod config;
pub mod counters;
pub mod driver;
pub mod dump;
pub mod evict;
pub mod fakes;
pub mod fast;
pub mod health;
pub mod input;
pub mod intake;
pub mod keymap;
pub mod ordering;
pub mod paths;
pub mod pipeline;
pub mod process;
pub mod recent;
pub mod services;
pub mod settings;
pub mod store;
pub mod time;
pub mod watchlist;
#[cfg(windows)]
pub mod win;

/// The message of a panic that ended a thread, from its join.
pub(crate) fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic without a message".into())
}
