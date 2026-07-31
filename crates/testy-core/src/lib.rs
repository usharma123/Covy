//! Test selection, test-map construction, timing ingestion, and shard planning.
//!
//! Reusable fallible APIs return [`error::TestyError`]. Coverage and diff
//! integrations cross the typed [`error::AdapterError`] boundary so callers
//! can inspect original sources while CLI crates remain free to add presentation
//! context.

pub mod error;

pub mod diagnostics {
    pub use suite_packet_core::diagnostics::*;
}

pub mod model {
    pub use suite_packet_core::coverage::*;
    pub use suite_packet_core::gate::*;
    pub use suite_packet_core::merge::*;
    pub use suite_packet_core::shard::*;
}

pub mod testmap {
    pub use suite_packet_core::testmap::*;
}

pub mod cache {
    pub use suite_foundation_core::cache::*;
}

pub mod command_impact;
pub mod command_shard;
pub mod command_testmap;
pub mod impact;
pub mod merge;
pub mod pipeline;
pub mod pipeline_shard;
pub mod pipeline_testmap;
pub mod shard;
pub mod shard_timing;
