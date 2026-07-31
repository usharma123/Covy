use std::collections::BTreeSet;
use std::path::Path;

use packet28_daemon_client::runtime_discovery::{
    read_runtime_info, read_runtime_info_if_present, RuntimeDiscoveryError,
};
use packet28_daemon_protocol::message::DaemonRuntimeInfo;

const REVIEWED_MODULES: &[(&str, &str)] = &[
    (
        "runtime_discovery",
        "authenticated discovery has one runnable happy-path example",
    ),
    (
        "transport",
        "connection authentication requires Unix/process fixtures",
    ),
];

fn assert_public_error<T: std::error::Error + Send + Sync + 'static>() {}

#[test]
fn every_public_module_has_a_reviewed_classification() {
    let source = include_str!("../src/lib.rs");
    let actual = source
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("pub mod ")
                .and_then(|tail| tail.strip_suffix(';'))
                .map(ToOwned::to_owned)
        })
        .collect::<BTreeSet<_>>();
    let expected = REVIEWED_MODULES
        .iter()
        .map(|(module, reason)| {
            assert!(
                !reason.trim().is_empty(),
                "reviewed module '{module}' needs a coverage reason"
            );
            (*module).to_owned()
        })
        .collect::<BTreeSet<_>>();

    assert_eq!(
        actual, expected,
        "client root changed without updating its reviewed documentation inventory"
    );
}

#[test]
fn discovery_family_has_one_runnable_doctest_marker() {
    let source = include_str!("../src/lib.rs");
    let marker = "<!-- public-surface:daemon-client-discovery -->";

    assert_eq!(
        source.match_indices(marker).count(),
        1,
        "authenticated discovery needs exactly one reviewed runnable example"
    );
}

#[test]
fn selected_discovery_api_remains_nameable() {
    assert_public_error::<RuntimeDiscoveryError>();

    let _optional: fn(&Path) -> Result<Option<DaemonRuntimeInfo>, RuntimeDiscoveryError> =
        read_runtime_info_if_present;
    let _required: fn(&Path) -> Result<DaemonRuntimeInfo, RuntimeDiscoveryError> =
        read_runtime_info;
}

#[cfg(unix)]
#[test]
fn selected_transport_api_remains_nameable() {
    use std::time::Duration;

    use packet28_daemon_client::transport::{
        connect, connect_endpoint, discover_endpoint, endpoint_may_have_stale_socket,
        DaemonClientError, DaemonEndpoint, DaemonStream,
    };

    assert_public_error::<DaemonClientError>();
    let _discover: fn(&Path) -> Result<DaemonEndpoint, DaemonClientError> = discover_endpoint;
    let _connect: fn(&Path, Duration) -> Result<DaemonStream, DaemonClientError> = connect;
    let _connect_endpoint: fn(
        &DaemonEndpoint,
        Duration,
    ) -> Result<DaemonStream, DaemonClientError> = connect_endpoint;
    let _stale_socket: fn(&DaemonEndpoint) -> bool = endpoint_may_have_stale_socket;
}
