//! Length-prefixed JSON framing shared by daemon transports.
//!
//! Frames use an eight-byte big-endian payload length followed by one JSON
//! value. Both readers and writers enforce [`MAX_SOCKET_MESSAGE_BYTES`].
//!
//! # Examples
//!
//! ```
//! use std::io::Cursor;
//!
//! use packet28_daemon_protocol::frame::{read_frame, write_frame};
//! use packet28_daemon_protocol::{DaemonRequest, DaemonResponse};
//!
//! let mut wire = Vec::new();
//! write_frame(&mut wire, &DaemonRequest::Status)?;
//! let request: DaemonRequest = read_frame(&mut Cursor::new(wire))?;
//! assert!(matches!(request, DaemonRequest::Status));
//!
//! let mut response_wire = Vec::new();
//! write_frame(
//!     &mut response_wire,
//!     &DaemonResponse::Ack {
//!         message: "ready".to_owned(),
//!     },
//! )?;
//! let response: DaemonResponse = read_frame(&mut Cursor::new(response_wire))?;
//! assert!(matches!(response, DaemonResponse::Ack { .. }));
//! # Ok::<(), packet28_daemon_protocol::frame::FrameError>(())
//! ```

use std::io::{Read, Write};

use serde::de::DeserializeOwned;
use serde::Serialize;
use thiserror::Error;

/// Maximum serialized JSON body accepted by the daemon protocol.
pub const MAX_SOCKET_MESSAGE_BYTES: usize = 8 * 1024 * 1024;

/// Failure while encoding, decoding, or transferring a daemon frame.
#[derive(Debug, Error)]
pub enum FrameError {
    /// The underlying stream could not be read or written.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The frame body was not valid JSON for the requested type.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// A peer sent a frame with no body.
    #[error("socket frame length must be greater than zero")]
    Empty,
    /// A frame exceeded [`MAX_SOCKET_MESSAGE_BYTES`].
    #[error("socket frame too large: {actual} bytes exceeds limit of {limit}")]
    TooLarge {
        /// Encoded or declared byte count.
        actual: u64,
        /// Maximum accepted byte count.
        limit: usize,
    },
    /// A peer declared a length that cannot be represented on this platform.
    #[error("socket frame length does not fit in usize")]
    LengthOverflow,
}

/// Writes one big-endian length-prefixed JSON frame and flushes the stream.
pub fn write_frame<W: Write, T: Serialize>(writer: &mut W, value: &T) -> Result<(), FrameError> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_SOCKET_MESSAGE_BYTES {
        return Err(FrameError::TooLarge {
            actual: bytes.len() as u64,
            limit: MAX_SOCKET_MESSAGE_BYTES,
        });
    }
    let len = u64::try_from(bytes.len()).map_err(|_| FrameError::LengthOverflow)?;
    writer.write_all(&len.to_be_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}

/// Reads one big-endian length-prefixed JSON frame.
pub fn read_frame<R: Read, T: DeserializeOwned>(reader: &mut R) -> Result<T, FrameError> {
    let mut len_bytes = [0_u8; 8];
    reader.read_exact(&mut len_bytes)?;
    let declared = u64::from_be_bytes(len_bytes);
    let len = usize::try_from(declared).map_err(|_| FrameError::LengthOverflow)?;
    if len == 0 {
        return Err(FrameError::Empty);
    }
    if len > MAX_SOCKET_MESSAGE_BYTES {
        return Err(FrameError::TooLarge {
            actual: declared,
            limit: MAX_SOCKET_MESSAGE_BYTES,
        });
    }
    let mut body = vec![0_u8; len];
    reader.read_exact(&mut body)?;
    Ok(serde_json::from_slice(&body)?)
}

#[cfg(test)]
mod tests {
    use std::io::{self, Cursor};

    use serde_json::{json, Value};

    use super::*;

    #[test]
    fn frame_bytes_preserve_the_legacy_wire_format() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &json!({"type":"status"})).unwrap();

        assert_eq!(
            bytes,
            [
                0, 0, 0, 0, 0, 0, 0, 17, b'{', b'"', b't', b'y', b'p', b'e', b'"', b':', b'"',
                b's', b't', b'a', b't', b'u', b's', b'"', b'}'
            ]
        );
    }

    #[test]
    fn variable_payloads_roundtrip() {
        for size in [0, 1, 7, 255, 4096, 65_535] {
            let value = json!({"payload": "x".repeat(size)});
            let mut bytes = Vec::new();
            write_frame(&mut bytes, &value).unwrap();
            let decoded: Value = read_frame(&mut Cursor::new(bytes)).unwrap();
            assert_eq!(decoded, value);
        }
    }

    #[test]
    fn zero_length_is_rejected() {
        let error = read_frame::<_, Value>(&mut Cursor::new(0_u64.to_be_bytes())).unwrap_err();
        assert!(matches!(error, FrameError::Empty));
    }

    #[test]
    fn declared_oversize_is_rejected_before_body_allocation() {
        let declared = (MAX_SOCKET_MESSAGE_BYTES as u64) + 1;
        let error = read_frame::<_, Value>(&mut Cursor::new(declared.to_be_bytes())).unwrap_err();
        assert!(matches!(
            error,
            FrameError::TooLarge {
                actual,
                limit: MAX_SOCKET_MESSAGE_BYTES
            } if actual == declared
        ));
    }

    #[test]
    fn encoded_oversize_is_rejected() {
        let value = "x".repeat(MAX_SOCKET_MESSAGE_BYTES);
        let error = write_frame(&mut Vec::new(), &value).unwrap_err();
        assert!(matches!(error, FrameError::TooLarge { .. }));
    }

    #[test]
    fn truncated_prefix_and_body_are_io_errors() {
        let prefix_error = read_frame::<_, Value>(&mut Cursor::new([0_u8; 7])).unwrap_err();
        assert!(matches!(
            prefix_error,
            FrameError::Io(error) if error.kind() == io::ErrorKind::UnexpectedEof
        ));

        let mut body = 4_u64.to_be_bytes().to_vec();
        body.extend_from_slice(b"{}");
        let body_error = read_frame::<_, Value>(&mut Cursor::new(body)).unwrap_err();
        assert!(matches!(
            body_error,
            FrameError::Io(error) if error.kind() == io::ErrorKind::UnexpectedEof
        ));
    }

    #[test]
    fn invalid_json_is_reported() {
        let mut bytes = 1_u64.to_be_bytes().to_vec();
        bytes.push(b'{');
        let error = read_frame::<_, Value>(&mut Cursor::new(bytes)).unwrap_err();
        assert!(matches!(error, FrameError::Json(_)));
    }

    #[test]
    fn chunked_reader_and_writer_preserve_frames() {
        struct OneByteWriter(Vec<u8>);

        impl Write for OneByteWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                let Some(byte) = bytes.first() else {
                    return Ok(0);
                };
                self.0.push(*byte);
                Ok(1)
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        struct OneByteReader(Cursor<Vec<u8>>);

        impl Read for OneByteReader {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                let limit = bytes.len().min(1);
                self.0.read(&mut bytes[..limit])
            }
        }

        let value = json!({"type":"status"});
        let mut writer = OneByteWriter(Vec::new());
        write_frame(&mut writer, &value).unwrap();
        let decoded: Value = read_frame(&mut OneByteReader(Cursor::new(writer.0))).unwrap();
        assert_eq!(decoded, value);
    }
}
