//! #501 client-contract guard: the client's exported `CAPABILITIES`
//! constant is a PUBLISHED surface and must advertise exactly the daemon's
//! read-only set. This compares EXPORTED VALUES (not file text), so
//! reintroducing one removed mutation/terminal capability — or omitting a
//! supported signed read — fails RED here.

use corrald_client::model::CAPABILITIES;

/// The read-only advertisement since the #354 cut, spelled out locally so a
/// drift that moves BOTH constants together still fails.
const READ_ONLY_SET: [&str; 2] = ["read_tail", "read_diff"];

#[test]
fn client_capabilities_equal_daemon_read_only_set() {
    assert_eq!(
        CAPABILITIES.as_slice(),
        READ_ONLY_SET.as_slice(),
        "client CAPABILITIES must advertise exactly read_tail + read_diff; \
         removed mutation/terminal capabilities must not reappear (#501)"
    );
    // Cross-crate parity: `core::model::CAPABILITIES` is the contract this
    // mirror is pinned to, so an intentional daemon-side change cannot leave
    // the client stale.
    assert_eq!(
        CAPABILITIES.as_slice(),
        corrald::core::model::CAPABILITIES.as_slice(),
        "client CAPABILITIES must equal the daemon's advertised set"
    );
}
