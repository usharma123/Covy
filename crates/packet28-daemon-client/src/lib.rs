//! Authenticated endpoint discovery and connection support for Packet28 clients.
//!
//! This crate sits between the implementation-free daemon wire contract and
//! concrete clients. It authenticates runtime discovery metadata, verifies
//! Unix server credentials, and performs the mandatory loopback-TCP capability
//! prelude without depending on daemon storage or runtime implementations.

pub mod runtime_discovery;

#[cfg(unix)]
pub mod transport;
