//! AutoLog's domain: reading worklog exports, matching them to a tracker's
//! own names, and posting them.
//!
//! Nothing here knows about a window. The GUI drives this crate over channels,
//! which keeps the rules testable and leaves the door open to a CLI.

pub mod config;
pub mod consumer;
pub mod error;
pub mod http;
pub mod logging;
pub mod matching;
pub mod model;
pub mod pipeline;
pub mod secrets;
pub mod source;
pub mod time;

pub use error::{UserError, UserFacing};
pub use matching::MatchStrategy;
pub use model::{EntryStatus, LoggedEntry, WorklogEntry};
