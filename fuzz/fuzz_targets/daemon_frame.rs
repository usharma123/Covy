#![no_main]

use std::borrow::Cow;
use std::io::Cursor;

use libfuzzer_sys::fuzz_target;
use packet28_daemon_protocol::frame::{read_frame, MAX_SOCKET_MESSAGE_BYTES};
use packet28_daemon_protocol::DaemonRequest;

const MAX_FUZZ_INPUT_BYTES: usize = 256 * 1024;
const MAX_FUZZ_BODY_BYTES: u64 = MAX_FUZZ_INPUT_BYTES as u64;

fuzz_target!(|input: &[u8]| {
    if input.len() > MAX_FUZZ_INPUT_BYTES {
        return;
    }

    // A leading '=' lets text corpus files seed semantically valid frames.
    // Every other input is interpreted as raw, attacker-controlled wire bytes.
    let wire = if let Some(body) = input.strip_prefix(b"=") {
        let mut framed = Vec::with_capacity(8 + body.len());
        framed.extend_from_slice(&(body.len() as u64).to_be_bytes());
        framed.extend_from_slice(body);
        Cow::Owned(framed)
    } else {
        Cow::Borrowed(input)
    };

    if wire.len() >= 8 {
        let mut prefix = [0_u8; 8];
        prefix.copy_from_slice(&wire[..8]);
        let declared = u64::from_be_bytes(prefix);
        if declared > MAX_FUZZ_BODY_BYTES && declared <= MAX_SOCKET_MESSAGE_BYTES as u64 {
            return;
        }
    }

    let _ = read_frame::<_, DaemonRequest>(&mut Cursor::new(wire.as_ref()));
});
