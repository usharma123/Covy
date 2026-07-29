//! Shared argument, adapter, and rendering helpers for the test CLIs.

pub mod adapters;
pub mod error;
pub mod impact;
pub mod shard;
pub mod support;
pub mod testmap;

pub use error::{Result, TestyCliError};

#[cfg(test)]
mod tests;
