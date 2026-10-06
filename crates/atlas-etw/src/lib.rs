//! ETW for the Atlas sensor (sensor spec §3.1, §4).
//!
//! - [`parse`]: pure parsers, payload bytes → [`parse::RawEvent`]. Portable and
//!   free of `unsafe`; unit-tested and fuzzed on Linux.
//! - [`layout`]: the manifest layout of every event version we parse.
//! - [`providers`]: the providers, and how Session A enables them (§4.2).
//! - `session` (Windows only): start, enable, consume, query and stop the
//!   real-time sessions. All of the sensor's ETW `unsafe` lives there, behind
//!   a safe API.

pub mod layout;
pub mod parse;
pub mod providers;
#[cfg(windows)]
pub mod session;

pub use providers::Provider;
