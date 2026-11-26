//! Runs test suites against a board: ordering, timeouts and SKIP when a test does not apply.
//!
//! Testable without a board (fake transport plus byte fixtures). It may use `probe`, `chips`
//! and the `report` model, but must not import `ui`, `ratatui` or `crossterm`.
//!
//! # Port suite (H3)
//! Every port of a board is synced with the ROM once, in order (bridge first, ADR-008 §5):
//! [`sync_port`] opens a session and reads the MAC, producing a [`PortSync`]. The decision of
//! PASS/FAIL/SKIP is the pure function [`port_tests`], following `report-json.md` §4 and D7:
//! - `port/uart-bridge` exists only when the board has a bridge port;
//! - `port/usb-serial-jtag` always exists: SKIP when the capability table says the chip has no
//!   USB-Serial/JTAG, SKIP when it has one but no `303A:1001` port is in the group, otherwise
//!   the result of the sync (MAC equal to the board MAC and to the port's USB serial number).
//!
//! The threaded runner with events (ADR-011 §5) comes with #20.

use std::time::Instant;

use crate::chips::ChipCaps;
use crate::probe::{Probe, ProbeError};
use crate::report::{Outcome, SafeText, Suite, TestId, TestResult};
use crate::transport::{Mac, PortClass, PortDesc, classify};

/// `port/uart-bridge`: sync and MAC through the USB-UART bridge.
pub const UART_BRIDGE: TestId = TestId::new(Suite::Port, "uart-bridge");

/// `port/usb-serial-jtag`: sync and MAC through the native USB-Serial/JTAG port.
pub const USB_SERIAL_JTAG: TestId = TestId::new(Suite::Port, "usb-serial-jtag");

/// SKIP reason when the capability table says the chip has no USB-Serial/JTAG (spec §4).
pub const SKIP_NO_USB_SERIAL_JTAG: &str =
    "chip has no USB-Serial/JTAG; JTAG needs an external probe (GPIO12-15)";

/// SKIP reason when the chip has USB-Serial/JTAG but no native port is connected (spec §4).
pub const SKIP_NATIVE_NOT_CONNECTED: &str = "native USB not connected on this board";

/// What syncing through one port of the board gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortSync {
    /// The port.
    pub port: PortDesc,
    /// MAC read from the ROM through this port, or why the session failed.
    pub mac: Result<Mac, ProbeError>,
    /// Time from opening the port to the MAC read (or the failure), in ms.
    pub duration_ms: u64,
}

/// Opens a ROM session on `port` and reads the MAC (H3).
///
/// The session is left open on success so the caller can run the other suites through the
/// primary port; the caller must end it with [`Probe::hard_reset`] in every case.
pub fn sync_port(probe: &mut dyn Probe, port: &PortDesc) -> PortSync {
    let start = Instant::now();
    let mac = probe.connect(port).and_then(|()| probe.mac());
    PortSync {
        port: port.clone(),
        mac,
        duration_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

/// The `port` suite of one board, in catalog order (`uart-bridge`, then `usb-serial-jtag`).
///
/// `caps` is the chip row (from the primary port), `board_mac` the board identity (ADR-008 §1)
/// and `syncs` one entry per port of the group. At most one port of each kind is expected; if
/// there are more, the first one counts.
pub fn port_tests(caps: &ChipCaps, board_mac: Option<Mac>, syncs: &[PortSync]) -> Vec<TestResult> {
    let mut tests = Vec::new();
    let bridge = syncs
        .iter()
        .find(|sync| matches!(classify(&sync.port), PortClass::Bridge { .. }));
    let native = syncs
        .iter()
        .find(|sync| classify(&sync.port) == PortClass::Native);

    if let Some(sync) = bridge {
        tests.push(TestResult {
            id: UART_BRIDGE,
            outcome: synced(board_mac, sync, None),
        });
    }

    let outcome = match (caps.usb_serial_jtag.value, native) {
        (false, None) => Outcome::skip(SKIP_NO_USB_SERIAL_JTAG),
        (true, None) => Outcome::skip(SKIP_NATIVE_NOT_CONNECTED),
        // A native port answered with this board's MAC, but the table says the chip has no
        // USB-Serial/JTAG: the table or the identification is wrong (invariant finding).
        (false, Some(sync)) if sync.mac.is_ok() => Outcome::from_comparison(
            caps.chip,
            "usb_serial_jtag",
            SafeText::from_host("yes"),
            sync.duration_ms,
        ),
        (_, Some(sync)) => synced(board_mac, sync, native_serial_mac(&sync.port)),
    };
    tests.push(TestResult {
        id: USB_SERIAL_JTAG,
        outcome,
    });
    tests
}

/// PASS when the sync worked and the MAC matches the board (and the USB serial number, when
/// the port has one that is a MAC); FAIL otherwise.
fn synced(board_mac: Option<Mac>, sync: &PortSync, serial_mac: Option<Mac>) -> Outcome {
    let expected = board_mac.map(|mac| SafeText::from_host(&mac.to_string()));
    let read = match &sync.mac {
        Ok(mac) => *mac,
        Err(error) => {
            let reason = format!("sync failed: {error}");
            return Outcome::fail(&reason, sync.duration_ms, expected, None);
        }
    };
    let read_text = Some(SafeText::from_host(&read.to_string()));
    if board_mac.is_some_and(|board| board != read) {
        return Outcome::fail(
            "MAC read through this port differs from the board MAC",
            sync.duration_ms,
            expected,
            read_text,
        );
    }
    if serial_mac.is_some_and(|serial| serial != read) {
        return Outcome::fail(
            "MAC read through this port differs from its USB serial number",
            sync.duration_ms,
            expected,
            read_text,
        );
    }
    Outcome::pass(sync.duration_ms, expected, read_text)
}

/// The native port's USB serial number as a MAC (ADR-008 §6). `None` when absent or not a MAC
/// (such a port is grouped as a bridge, so there is nothing to compare).
fn native_serial_mac(port: &PortDesc) -> Option<Mac> {
    port.usb
        .as_ref()
        .and_then(|usb| usb.serial.as_deref())
        .and_then(|serial| Mac::parse(serial).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chips::Chip;
    use crate::probe::fake::{FakeProbe, Step};
    use crate::report::State;
    use crate::transport::UsbIds;

    fn usb_port(name: &str, vid: u16, pid: u16, serial: Option<&str>) -> PortDesc {
        PortDesc {
            name: name.to_owned(),
            usb: Some(UsbIds {
                vid,
                pid,
                serial: serial.map(str::to_owned),
                manufacturer: None,
                product: None,
                interface: None,
            }),
        }
    }

    fn ch343() -> PortDesc {
        usb_port("bridge", 0x1A86, 0x55D3, Some("5D28088406"))
    }

    fn ch9102() -> PortDesc {
        usb_port("bridge", 0x1A86, 0x55D4, Some("5B53026573"))
    }

    fn native(serial: &str) -> PortDesc {
        usb_port("native", 0x303A, 0x1001, Some(serial))
    }

    /// Syncs `port` with a fresh fake of `fixture` and ends the session, like the runner will.
    fn sync_with(fixture: &str, port: &PortDesc, fail: Option<ProbeError>) -> PortSync {
        let mut probe = FakeProbe::from_fixture(fixture).unwrap();
        if let Some(error) = fail {
            probe = probe.fail_at(Step::Connect, error);
        }
        let sync = sync_port(&mut probe, port);
        let _ = probe.hard_reset();
        sync
    }

    fn caps_of(fixture: &str) -> &'static ChipCaps {
        Chip::from_target(fixture).unwrap().caps()
    }

    fn mac_of(sync: &PortSync) -> Option<Mac> {
        sync.mac.as_ref().ok().copied()
    }

    fn ids(tests: &[TestResult]) -> Vec<String> {
        tests.iter().map(|test| test.id.to_string()).collect()
    }

    #[test]
    fn port_suite_skips_usb_serial_jtag_on_esp32() {
        let bridge = sync_with("esp32", &ch9102(), None);
        let tests = port_tests(caps_of("esp32"), mac_of(&bridge), &[bridge]);
        assert_eq!(ids(&tests), ["port/uart-bridge", "port/usb-serial-jtag"]);
        assert_eq!(tests[0].outcome.state(), State::Pass);
        assert_eq!(
            tests[0].outcome.read().map(SafeText::as_str),
            Some("1c:c3:ab:d2:25:b4")
        );
        let skip = &tests[1].outcome;
        assert_eq!(skip.state(), State::Skip);
        assert_eq!(
            skip.reason().map(SafeText::as_str),
            Some("chip has no USB-Serial/JTAG; JTAG needs an external probe (GPIO12-15)")
        );
        assert_eq!(skip.duration_ms(), 0);
    }

    #[test]
    fn port_suite_skips_when_native_not_connected() {
        let bridge = sync_with("esp32s3", &ch343(), None);
        let tests = port_tests(caps_of("esp32s3"), mac_of(&bridge), &[bridge]);
        assert_eq!(ids(&tests), ["port/uart-bridge", "port/usb-serial-jtag"]);
        assert_eq!(tests[0].outcome.state(), State::Pass);
        assert_eq!(tests[1].outcome.state(), State::Skip);
        assert_eq!(
            tests[1].outcome.reason().map(SafeText::as_str),
            Some("native USB not connected on this board")
        );
    }

    #[test]
    fn port_suite_has_no_uart_bridge_test_without_bridge() {
        let port = native("E8:F6:0A:F7:FF:D4");
        let sync = sync_with("esp32c6", &port, None);
        let tests = port_tests(caps_of("esp32c6"), mac_of(&sync), &[sync]);
        assert_eq!(ids(&tests), ["port/usb-serial-jtag"]);
        assert_eq!(tests[0].outcome.state(), State::Pass);
    }

    #[test]
    fn port_suite_passes_synced_ports() {
        // C6 with both cables: bridge first (primary), then the native port.
        let bridge = sync_with("esp32c6", &ch343(), None);
        let native = sync_with("esp32c6", &native("E8:F6:0A:F7:FF:D4"), None);
        let board = mac_of(&bridge);
        let tests = port_tests(caps_of("esp32c6"), board, &[bridge, native]);
        assert_eq!(ids(&tests), ["port/uart-bridge", "port/usb-serial-jtag"]);
        for test in &tests {
            let outcome = &test.outcome;
            assert_eq!(outcome.state(), State::Pass, "{}", test.id);
            assert_eq!(outcome.reason(), None);
            assert_eq!(
                outcome.expected().map(SafeText::as_str),
                Some("e8:f6:0a:f7:ff:d4")
            );
            assert_eq!(
                outcome.read().map(SafeText::as_str),
                Some("e8:f6:0a:f7:ff:d4")
            );
            assert!(outcome.finding().is_none());
        }
    }

    #[test]
    fn port_suite_fails_a_port_that_did_not_sync() {
        let bridge = sync_with("esp32s3", &ch343(), Some(ProbeError::Timeout));
        assert_eq!(bridge.mac, Err(ProbeError::Timeout));
        let tests = port_tests(caps_of("esp32s3"), None, &[bridge]);
        let fail = &tests[0].outcome;
        assert_eq!(fail.state(), State::Fail);
        assert_eq!(
            fail.reason().map(SafeText::as_str),
            Some("sync failed: timeout")
        );
        assert_eq!(fail.read(), None);
    }

    #[test]
    fn port_suite_fails_on_a_mac_mismatch() {
        let bridge = sync_with("esp32c6", &ch343(), None);
        let board = mac_of(&bridge);
        // Native port whose serial number is another board's MAC.
        let other = sync_with("esp32c6", &native("02:00:00:00:00:01"), None);
        let tests = port_tests(caps_of("esp32c6"), board, &[bridge, other]);
        assert_eq!(tests[0].outcome.state(), State::Pass);
        assert_eq!(tests[1].outcome.state(), State::Fail);
        assert_eq!(
            tests[1].outcome.reason().map(SafeText::as_str),
            Some("MAC read through this port differs from its USB serial number")
        );

        // A bridge that answered with a MAC that is not the board's.
        let mut probe = FakeProbe::from_fixture("esp32c6")
            .unwrap()
            .with_mac(Mac([0x02, 0, 0, 0, 0, 2]));
        let wrong = sync_port(&mut probe, &ch343());
        let tests = port_tests(caps_of("esp32c6"), board, &[wrong]);
        assert_eq!(tests[0].outcome.state(), State::Fail);
        assert_eq!(
            tests[0].outcome.reason().map(SafeText::as_str),
            Some("MAC read through this port differs from the board MAC")
        );
    }

    #[test]
    fn native_port_on_a_chip_without_one_is_an_invariant_finding() {
        let bridge = sync_with("esp32", &ch9102(), None);
        let board = mac_of(&bridge);
        let odd = sync_with("esp32", &native("1C:C3:AB:D2:25:B4"), None);
        let tests = port_tests(caps_of("esp32"), board, &[bridge, odd]);
        let outcome = &tests[1].outcome;
        assert_eq!(outcome.state(), State::Fail);
        let finding = outcome.finding().unwrap();
        assert_eq!(finding.field(), "usb_serial_jtag");
        assert_eq!(finding.kind(), crate::chips::FieldKind::Invariant);
    }

    #[test]
    fn sync_port_leaves_the_session_for_the_caller() {
        let mut probe = FakeProbe::from_fixture("esp32c3").unwrap();
        let sync = sync_port(&mut probe, &ch343());
        assert!(sync.mac.is_ok());
        assert_eq!(probe.calls(), [Step::Connect, Step::Mac]);
        assert!(probe.hard_reset().is_ok());
    }
}
