//! Persistence and local runtime support for the Packet28 daemon.
//!
//! Wire messages, framing, and endpoint paths live in
//! `packet28-daemon-protocol`. The pre-split root imports remain available
//! unconditionally for source compatibility.

mod compat_v0;
pub mod integrity;
pub mod storage;
pub mod trust;

pub use compat_v0::*;
