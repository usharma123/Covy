//! Source-compatible v0 daemon-core root facade.
//!
//! New code should import wire contracts and endpoint helpers from their
//! explicit `packet28-daemon-protocol` modules. This bridge remains enabled by
//! default for the documented compatibility window.

use std::io::{Read, Write};

use anyhow::Result;
use serde::de::DeserializeOwned;
use serde::Serialize;

pub use crate::storage::*;
pub use packet28_daemon_protocol::broker::*;
pub use packet28_daemon_protocol::commands::*;
pub use packet28_daemon_protocol::context_store::*;
pub use packet28_daemon_protocol::frame::MAX_SOCKET_MESSAGE_BYTES;
pub use packet28_daemon_protocol::hooks::*;
pub use packet28_daemon_protocol::index::*;
pub use packet28_daemon_protocol::message::*;
pub use packet28_daemon_protocol::paths::*;
pub use packet28_daemon_protocol::task::*;

/// Writes a legacy daemon-core socket message using the shared protocol codec.
pub fn write_socket_message<W: Write, T: Serialize>(writer: &mut W, value: &T) -> Result<()> {
    packet28_daemon_protocol::frame::write_frame(writer, value).map_err(Into::into)
}

/// Reads a legacy daemon-core socket message using the shared protocol codec.
pub fn read_socket_message<R: Read, T: DeserializeOwned>(reader: &mut R) -> Result<T> {
    packet28_daemon_protocol::frame::read_frame(reader).map_err(Into::into)
}
