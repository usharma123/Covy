//! Source-compatible v0 daemon-core root facade.
//!
//! New code should import wire contracts and endpoint helpers from their
//! explicit `packet28-daemon-protocol` modules. This private adapter remains
//! enabled through `0.2.x`; the explicit root allowlist may be removed in
//! `0.3.0`.

use std::io::{Read, Write};

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::{DaemonCoreError, Result};

/// Writes a legacy daemon-core socket message using the shared protocol codec.
///
/// # Errors
///
/// Returns [`DaemonCoreError::Frame`] if the value cannot be serialized or the
/// destination stream cannot accept or flush the bounded frame.
pub fn write_socket_message<W: Write, T: Serialize>(writer: &mut W, value: &T) -> Result<()> {
    packet28_daemon_protocol::frame::write_frame(writer, value)
        .map_err(|source| DaemonCoreError::frame("failed to write", source))
}

/// Reads a legacy daemon-core socket message using the shared protocol codec.
///
/// # Errors
///
/// Returns [`DaemonCoreError::Frame`] if the stream is truncated, the declared
/// frame exceeds the protocol limit, or the payload is not valid JSON for `T`.
pub fn read_socket_message<R: Read, T: DeserializeOwned>(reader: &mut R) -> Result<T> {
    packet28_daemon_protocol::frame::read_frame(reader)
        .map_err(|source| DaemonCoreError::frame("failed to read", source))
}
