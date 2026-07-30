//! Authenticated endpoint discovery and connection support for Packet28 clients.
//!
//! This crate sits between the implementation-free daemon wire contract and
//! concrete clients. It authenticates runtime discovery metadata, verifies
//! Unix server credentials, and performs the mandatory loopback-TCP capability
//! prelude without depending on daemon storage or runtime implementations.
//!
//! <!-- public-surface:daemon-client-discovery -->
//! # Examples
//!
//! Authenticated discovery distinguishes an absent daemon publication from an
//! invalid or unauthentic one:
//!
//! ```
//! use packet28_daemon_client::runtime_discovery::read_runtime_info_if_present;
//!
//! let workspace = tempfile::tempdir()?;
//! let runtime = read_runtime_info_if_present(workspace.path())?;
//! assert!(runtime.is_none());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Migration and errors
//!
//! New clients should use [`runtime_discovery`] and, on Unix, [`transport`]
//! instead of reading `.packet28/daemon/runtime.json` or connecting to a
//! conventional socket path themselves. Discovery failures preserve typed
//! filesystem and JSON sources. A published loopback-TCP endpoint without its
//! per-instance capability is rejected as a legacy insecure daemon and must
//! be restarted; the client never sends an ordinary request first.

pub mod runtime_discovery;

#[cfg(unix)]
pub mod transport;
