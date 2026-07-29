# Supported public surface

The supported operational API has three entrypoint families. Each family below
has a runnable happy-path example. The examples are part of the workspace
doctest gate.

<!-- public-surface:envelope-hash -->
## Envelope construction and deterministic identity

Use [`envelope`] to construct typed packets, normalize file and symbol
references, and derive deterministic content identity. Runtime duration,
generation time, and estimated serialized size do not participate in the
canonical hash.

```rust
use serde_json::json;
use suite_packet_core::envelope::{EnvelopeV1, FileRef};

let packet = EnvelopeV1 {
    tool: "example".to_string(),
    kind: "example.result".to_string(),
    summary: "one result".to_string(),
    files: vec![FileRef {
        path: "src/lib.rs".to_string(),
        ..FileRef::default()
    }],
    payload: json!({"count": 1}),
    ..EnvelopeV1::default()
}
.with_canonical_hash_and_real_budget();

assert_eq!(packet.version, "1");
assert_eq!(packet.hash.len(), 64);
assert_eq!(packet.budget_cost.est_bytes, serde_json::to_vec(&packet)?.len());
# Ok::<(), serde_json::Error>(())
```

<!-- public-surface:machine-artifact -->
## Machine wrappers and artifact I/O

Use [`machine`] to wrap an envelope for machine output or to persist and read a
full JSON artifact. Artifact handles identify the exact serialized wrapper
written to disk.

```rust
use serde_json::json;
use suite_packet_core::envelope::EnvelopeV1;
use suite_packet_core::machine::{
    read_packet_artifact, write_packet_artifact, PacketWrapperV1,
};

let nonce = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)?
    .as_nanos();
let root = std::env::temp_dir().join(format!(
    "suite-packet-core-doctest-{}-{nonce}",
    std::process::id()
));
let packet = EnvelopeV1 {
    tool: "example".to_string(),
    kind: "example.result".to_string(),
    summary: "persisted result".to_string(),
    payload: json!({"ok": true}),
    ..EnvelopeV1::default()
}
.with_canonical_hash();

let handle = write_packet_artifact(&root, "suite.example.result.v1", &packet)?;
let value = read_packet_artifact(&root, &handle.handle_id)?;
let wrapper: PacketWrapperV1<EnvelopeV1<serde_json::Value>> =
    serde_json::from_value(value)?;

assert_eq!(wrapper.packet_type, "suite.example.result.v1");
assert_eq!(wrapper.packet.hash, packet.hash);
std::fs::remove_dir_all(root)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

<!-- public-surface:schema-registry -->
## Packet registry and schema snapshots

Use [`registry`] to enumerate stable packet families and obtain their reviewed
payload-aware schema snapshots. Unknown packet identifiers return `None`.

```rust
use suite_packet_core::registry::{
    packet_contract, packet_type_schema_snapshot, PACKET_TYPE_MAP_QUERY,
};

let contract = packet_contract(PACKET_TYPE_MAP_QUERY).expect("registered family");
let schema =
    packet_type_schema_snapshot(contract.packet_type).expect("registered schema");

assert_eq!(contract.required_payload_fields, &["query", "matches", "truncation"]);
assert_eq!(
    schema["properties"]["packet_type"]["const"],
    PACKET_TYPE_MAP_QUERY
);
```

## Errors and compatibility

- Envelope sizing and hashing are intentionally infallible compatibility APIs:
  size estimation returns zero when serialization fails, while canonical
  hashing normalizes an unrepresentable value to JSON `null`.
- Artifact serialization and reads/writes return [`CovyError::Parse`] or
  [`CovyError::Io`] with their original path or format context.
- Parsing an unsupported [`JsonProfile`] returns [`CovyError::Config`].
- Registry lookup uses `Option`; an unknown packet family is not silently
  assigned a schema.
- The crate-root re-exports are a compatibility-only allowlist for `0.2`
  callers. New subsystem wire contracts belong in their named module.

## Reviewed exclusions

The `error` module is the stable error taxonomy exercised by the fallible
operational APIs. The following public modules are stable data contracts, not
operational entrypoint families: `agent`, `context`, `coverage`, `diagnostics`,
`gate`, `governance`, `instruction`, `kernel`, `memory`, `merge`, `search`,
`shard`, and `testmap`. Their happy paths are serialization, schema, and
wire-compatibility fixtures, so they are intentionally excluded from the
one-doctest-per-operational-family rule.

The `diff` module is a compatibility-only namespace. The private binary-codec
alias and private canonicalization, property-hint, and artifact helper
functions are internal implementation details and are intentionally excluded
from the supported public inventory.
