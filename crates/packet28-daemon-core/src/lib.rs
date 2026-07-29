//! Persistence and local runtime support for the Packet28 daemon.
//!
//! Wire messages, framing, and endpoint paths live in
//! `packet28-daemon-protocol`. The pre-split root imports remain available
//! unconditionally for source compatibility.
//!
//! Fallible library operations return [`DaemonCoreError`] and retain typed
//! filesystem, JSON, and framing causes. Executables may add presentation
//! context at their process boundary without losing the source chain.

mod compat_v0;
mod error;
pub mod integrity;
pub mod storage;
pub mod task_store_lease;
pub mod trust;

pub use compat_v0::*;
pub use error::{DaemonCoreError, Result};
