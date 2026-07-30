# Packet28 daemon protocol

`packet28-daemon-protocol` is the implementation-free contract shared by
Packet28 daemon clients and the daemon runtime.

Use explicit modules for new code:

```rust
use packet28_daemon_protocol::frame::{read_frame, write_frame};
use packet28_daemon_protocol::message::{DaemonRequest, DaemonResponse};
use packet28_daemon_protocol::paths::socket_path;
use packet28_daemon_protocol::registry::{
    DaemonRegistryRequestV1, DaemonRegistryResponseV1,
};
```

The crate contains serializable messages, length-prefixed JSON framing, and
deterministic endpoint paths. It does not contain daemon persistence, kernel,
memory, reducer, search, or transport-loop implementations.

The legacy `message::DaemonStatus`, `DaemonRequest`, and `DaemonResponse`
remain frozen for source and wire compatibility in the `0.2.x` line. Legacy
`Status` therefore remains an exhaustive registry dump when it fits its
bounded response; when it does not, the daemon returns an explicit error
instead of a misleading prefix.

Bounded liveness and large-registry traversal use the additive `registry`
module. `DaemonRegistryRequestV1::Status` reports authoritative task/watch
counts and a `registry_revision`; versioned task/watch page requests carry
that value as `snapshot_revision` on every subsequent request. The revision
contains both an opaque daemon-instance ID and a monotonic mutation counter,
preventing restart ABA as well as in-process page mixing. A mutation or restart
between pages is rejected so records from different snapshots cannot be mixed.
Page limits, individual-record limits, and the page-response byte bound fail
explicitly instead of closing a connection with an oversized frame.
Cursors name the last returned ID and are exclusive; a missing cursor is also
rejected so clients can restart their traversal. New clients normalize a
legacy exhaustive `Status` by deriving counts from its vectors and leave the
revision unset.

Loopback TCP uses `message::DaemonTransportAuth` as a mandatory first frame.
The daemon publishes that per-instance 256-bit capability only inside its
owner-authenticated `runtime.json`; Unix sockets authenticate the peer through
operating-system credentials in both directions instead. The preferred Unix
socket lives in an effective-user-specific, owner-only temporary directory;
the daemon authenticates that directory's owner, mode, ACL, and safe ancestry
before binding. Clients must authenticate the connected server UID before
sending a frame and must never fall back to sending ordinary requests on a TCP
connection when the capability is absent.

Use `packet28-daemon-client` when a process needs authenticated runtime
discovery or a ready-to-use daemon connection. This protocol crate
intentionally keeps filesystem trust checks and socket operations outside the
wire contract.

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
