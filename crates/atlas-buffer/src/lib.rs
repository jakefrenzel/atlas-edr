//! The agent's on-disk event buffer (sensor spec §8).

pub mod record;

mod cursor;
mod fsx;

pub use cursor::Cursor;
pub use fsx::{SEGMENT_HEADER, SEGMENT_MAGIC};
