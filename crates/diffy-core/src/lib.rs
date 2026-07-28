//! Diff-aware coverage and diagnostics analysis.
//!
//! The crate owns git diff parsing, coverage/diagnostics pipeline orchestration,
//! quality-gate evaluation, and report rendering. Fallible diff and pipeline
//! APIs return [`DiffyError`] so callers can inspect stable error variants while
//! binaries remain free to add `anyhow` context at their presentation boundary.

pub mod error;
pub use error::DiffyError;

pub mod diagnostics {
    pub use suite_packet_core::diagnostics::*;
}

pub mod model {
    pub use suite_packet_core::coverage::*;
    pub use suite_packet_core::gate::*;
    pub use suite_packet_core::merge::*;
    pub use suite_packet_core::shard::*;
}

pub mod config {
    pub use suite_foundation_core::config::*;
}

pub mod diff;
pub mod gate;
pub mod pipeline;
pub mod report;
