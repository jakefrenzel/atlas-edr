//! The event classes: the seven 0a classes (spec section 5) and the two added
//! by sub-project 1 (sensor spec §10). Each module holds the domain types for
//! one class plus their wire conversions.

pub mod dns;
pub mod event_log;
pub mod file;
pub mod module;
pub mod network;
pub mod process;
pub mod registry;
pub mod sensor_health;
