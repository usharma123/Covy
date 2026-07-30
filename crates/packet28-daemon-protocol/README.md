# Packet28 daemon protocol

`packet28-daemon-protocol` is the implementation-free contract shared by
Packet28 daemon clients and the daemon runtime.

Use explicit modules for new code:

```rust
use packet28_daemon_protocol::frame::{read_frame, write_frame};
use packet28_daemon_protocol::message::{DaemonRequest, DaemonResponse};
use packet28_daemon_protocol::paths::socket_path;
```

The crate contains serializable messages, length-prefixed JSON framing, and
deterministic endpoint paths. It does not contain daemon persistence, kernel,
memory, reducer, search, or transport-loop implementations.

Loopback TCP uses `message::DaemonTransportAuth` as a mandatory first frame.
The daemon publishes that per-instance 256-bit capability only inside its
owner-authenticated `runtime.json`; Unix sockets authenticate the peer through
operating-system credentials in both directions instead. The preferred Unix
socket lives in an effective-user-specific, owner-only temporary directory;
the daemon authenticates that directory's owner, mode, ACL, and safe ancestry
before binding. Clients must authenticate the connected server UID before
sending a frame and must never fall back to sending ordinary requests on a TCP
connection when the capability is absent.

## Migrating from `packet28-daemon-core`

Existing root imports continue to compile through daemon-core's unconditional
compatibility facade through the `0.2.x` release line, including for consumers
that disable default features:

```rust
use packet28_daemon_core::{read_socket_message, DaemonRequest};
```

Migrate clients to the protocol crate and its explicit modules. Runtime code
that needs registry persistence should instead use
`packet28_daemon_core::storage`.

The daemon-core root facade is frozen: new protocol types are not added to it
automatically. It may be removed in `0.3.0`, while the named protocol modules
remain the supported interface.
