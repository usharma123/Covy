# Packet28 daemon client

`packet28-daemon-client` provides the shared authenticated connection boundary
for `Packet28`, `p28`, and the Linux and macOS instruction shims.

It authenticates the workspace runtime-discovery namespace and metadata before
using the published endpoint. Unix connections verify the server's effective
user before any frame is sent. Loopback TCP connections send the published
per-instance capability as their first frame and wait for the authentication
acknowledgement before returning the stream.

The crate depends only on `packet28-daemon-protocol` and low-level filesystem
support. Daemon runtime, persistence, kernel, memory, and search implementations
remain above this boundary.
