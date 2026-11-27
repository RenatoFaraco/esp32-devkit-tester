//! The ports of one board, found by MAC and tested in sequence (H2, ADR-008).
//!
//! A dev board may show up as two serial ports: the chip's own USB-Serial/JTAG (`303A:1001`) and
//! a USB-UART bridge wired to UART0. Both reach the same ROM, so they are one board. The board's
//! identity is the factory base MAC (eFuse, ADR-008 §1), never a port name or a bridge serial:
//!
//! 1. [`pre_group`] (pure, nothing opened): a native port whose USB serial number is a MAC is a
//!    group with that MAC. A bridge, or a native port with an unreadable serial, is
//!    `unidentified`. Unknown, non-USB, JTAG-channel and app-firmware Espressif USB (OTG) ports
//!    are `others`: listed, never opened.
//! 2. [`identify`] (only after a user action): each unidentified port is opened once, one at a
//!    time: connect, MAC, chip, hard reset. A failure is a value with a reason, never a panic.
//! 3. [`merge`] (pure): an identified port joins the group with the same MAC, or becomes a board
//!    of its own (original ESP32, board with only a bridge).
//! 4. [`run_board`]: the primary port (the bridge when there is one, ADR-008 §5) runs the port
//!    sync and the `rom`/`efuse`/`flash` suites; then each other port is synced once. Any hard
//!    reset makes the native port vanish and come back (`hardware.md`), so it is looked for again
//!    by USB serial number, never by name, for at most [`REENUMERATE_TIMEOUT`].
//!
//! Sequential and blocking on purpose: it runs on the runner's thread ([`super::worker`]), which
//! forwards the [`Progress`] reported by [`run_board`] to the UI. Every source of I/O is injected
//! (probe factory, enumeration, clock, sleeper), so the whole flow is tested without a board and
//! without sleeping.

use std::time::Duration;

use super::{
    BOARD_TESTS, Clock, PortSync, Progress, Sleeper, SuiteFacts, SyncError, UART_BRIDGE,
    USB_SERIAL_JTAG, catalog, port_tests, run_suites_noting, sync_port,
};
use crate::chips::{Chip, ChipCaps};
use crate::probe::{Probe, ProbeError};
use crate::report::{
    BoardInfo, ChipInfo, HardReset, Outcome, PortInfo, PortKind, Report, Revision, Run, SafeText,
    TestId, TestResult,
};
use crate::transport::{Mac, PortClass, PortDesc, classify, trusted_serial};

/// How long a port may take to come back after a hard reset. The bench saw the native port
/// back in under 2 s (C3 and C6, Windows); 5 s leaves room for a slow hub.
pub const REENUMERATE_TIMEOUT: Duration = Duration::from_secs(5);

/// How often the OS port list is read while waiting for a port.
pub const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// A port that is still listed this long after the reset, without ever having vanished, is taken
/// as it is: the reset did not drop it, or it dropped and came back between two polls.
pub const VANISH_GRACE: Duration = Duration::from_secs(1);

/// How many sessions [`identify`] opens on a bridge whose connect times out.
///
/// Bench, 2026-11-26: an ESP32 DevKit V1 (CH9102) only syncs after several of espflash's reset
/// attempts (~3 s), and the TUI once got "identification failed: timeout" from it. A marginal
/// auto-reset circuit (EN/IO0 RC timing) or a board plugged in a moment ago and still booting
/// can use up every attempt of one connect; a fresh session usually syncs.
pub const IDENTIFY_ATTEMPTS: u32 = 2;

/// What the enumeration alone says, before any port is opened.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grouping {
    /// Native ports grouped by the MAC in their USB serial number, in enumeration order.
    pub groups: Vec<PreGroup>,
    /// Bridges, and native ports whose serial number is not a MAC: need [`identify`].
    pub unidentified: Vec<PortDesc>,
    /// Unknown VID:PID, non-USB, JTAG channel, ambiguous multi-channel and app-firmware
    /// Espressif USB (OTG) ports: never opened.
    pub others: Vec<PortDesc>,
}

/// Ports that share one MAC before identification (normally one native port).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreGroup {
    /// The board identity, from the native port's USB serial number.
    pub mac: Mac,
    /// The native ports with that serial number.
    pub ports: Vec<PortDesc>,
}

/// Splits the enumerated ports without opening any (ADR-008 §2).
pub fn pre_group(ports: &[PortDesc]) -> Grouping {
    let mut grouping = Grouping::default();
    for port in ports {
        let class = classify(port);
        if !class.is_probe_candidate() {
            grouping.others.push(port.clone());
            continue;
        }
        let mac = match class {
            PortClass::Native => serial_mac(port),
            // A bridge's serial number belongs to the bridge, never to the chip.
            _ => None,
        };
        match mac {
            Some(mac) => match grouping.groups.iter_mut().find(|group| group.mac == mac) {
                Some(group) => group.ports.push(port.clone()),
                None => grouping.groups.push(PreGroup {
                    mac,
                    ports: vec![port.clone()],
                }),
            },
            None => grouping.unidentified.push(port.clone()),
        }
    }
    grouping
}

/// The MAC in a port's USB serial number, if it has a trustworthy one that parses as a MAC.
fn serial_mac(port: &PortDesc) -> Option<Mac> {
    trusted_serial(port).and_then(|serial| Mac::parse(serial).ok())
}

/// What opening one unidentified port gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identified {
    /// The ROM answered through the port.
    Found {
        /// The port, as enumerated before it was opened.
        port: PortDesc,
        /// Factory base MAC read from the ROM: the board identity.
        mac: Mac,
        /// Chip family read from the ROM; `None` if that read failed.
        chip: Option<Chip>,
        /// Whether the board went back to its application afterwards (ADR-009 §4).
        reset: HardReset,
    },
    /// The port could not be identified; it stays out of every board.
    Failed {
        /// The port.
        port: PortDesc,
        /// Why, in host-written English (safe to show).
        reason: String,
    },
}

impl Identified {
    /// The port this result is about.
    pub fn port(&self) -> &PortDesc {
        match self {
            Identified::Found { port, .. } | Identified::Failed { port, .. } => port,
        }
    }
}

/// Opens each port of `ports` once, in order, to read the MAC (ADR-008 §4): connect, MAC, chip,
/// then a hard reset back to the application in every case.
///
/// A connect that fails with [`ProbeError::Timeout`] gets one more session (see
/// [`IDENTIFY_ATTEMPTS`]); a busy or vanished port does not (another program holds it, or it is
/// gone: trying again at once would not change that).
///
/// Call it only after the user asked (each port suffers a reset). `probe` makes a fresh
/// [`Probe`] for each port. A port that is not a probe candidate is never opened: it comes back
/// as [`Identified::Failed`].
pub fn identify(
    probe: &mut dyn FnMut(&PortDesc) -> Box<dyn Probe>,
    ports: &[PortDesc],
) -> Vec<Identified> {
    let mut identified = Vec::with_capacity(ports.len());
    for port in ports {
        if !classify(port).is_probe_candidate() {
            identified.push(Identified::Failed {
                port: port.clone(),
                reason: "not an ESP32 serial port; never opened".to_owned(),
            });
            continue;
        }
        let mut attempt = 0;
        let (mac, chip, reset) = loop {
            attempt += 1;
            let mut session = probe(port);
            let connected = session.connect(port);
            let sync_timed_out = connected == Err(ProbeError::Timeout);
            let mac = connected.and_then(|()| session.mac());
            let chip = match mac {
                Ok(_) => session.chip().ok(),
                Err(_) => None,
            };
            let reset = reset_outcome(session.hard_reset());
            drop(session);
            if !sync_timed_out || attempt >= IDENTIFY_ATTEMPTS {
                break (mac, chip, reset);
            }
        };
        identified.push(match mac {
            Ok(mac) => Identified::Found {
                port: port.clone(),
                mac,
                chip,
                reset,
            },
            Err(error) => Identified::Failed {
                port: port.clone(),
                reason: format!("identification failed: {error}"),
            },
        });
    }
    identified
}

/// Joins the identified ports to the groups by MAC (ADR-008 §4). Pure.
///
/// A port found with the MAC of a group joins it; otherwise it starts a board of its own.
/// [`Identified::Failed`] ports and unidentified ports with no result stay out. Boards come in
/// enumeration order of their first port group, then in identification order.
pub fn merge(grouping: &Grouping, identified: &[Identified]) -> Vec<Board> {
    let mut drafts: Vec<(Mac, Option<Chip>, Vec<PortDesc>)> = grouping
        .groups
        .iter()
        .map(|group| (group.mac, None, group.ports.clone()))
        .collect();
    for result in identified {
        let Identified::Found {
            port, mac, chip, ..
        } = result
        else {
            continue;
        };
        match drafts.iter_mut().find(|draft| draft.0 == *mac) {
            Some(draft) => {
                draft.1 = draft.1.or(*chip);
                draft.2.push(port.clone());
            }
            None => drafts.push((*mac, *chip, vec![port.clone()])),
        }
    }
    drafts
        .into_iter()
        .filter_map(|(mac, chip, ports)| Board::new(mac, chip, ports))
        .collect()
}

/// One board: its identity and its ports, with exactly one primary port by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    mac: Mac,
    chip: Option<Chip>,
    primary: PortDesc,
    others: Vec<PortDesc>,
}

impl Board {
    /// A board with identity `mac` and these ports, ordered bridge first (ADR-008 §5). The
    /// primary port is the first bridge, or the native port when there is no bridge.
    ///
    /// `None` when `ports` is empty or holds a port that may not be opened (not a native port
    /// nor an unambiguous bridge).
    pub fn new(mac: Mac, chip: Option<Chip>, ports: Vec<PortDesc>) -> Option<Board> {
        let mut classes = Vec::with_capacity(ports.len());
        for port in ports {
            let class = classify(&port);
            if !class.is_probe_candidate() {
                return None;
            }
            classes.push((class == PortClass::Native, port));
        }
        // Stable: `false` (bridge) sorts before `true` (native); order inside a kind is kept.
        classes.sort_by_key(|(native, _)| *native);
        let mut ports = classes.into_iter().map(|(_, port)| port);
        let primary = ports.next()?;
        Some(Board {
            mac,
            chip,
            primary,
            others: ports.collect(),
        })
    }

    /// The board identity: factory base MAC.
    pub fn mac(&self) -> Mac {
        self.mac
    }

    /// Chip family, if identification read it (a native-only board is not opened to list it).
    pub fn chip(&self) -> Option<Chip> {
        self.chip
    }

    /// The port through which the `rom`, `efuse` and `flash` suites run.
    pub fn primary(&self) -> &PortDesc {
        &self.primary
    }

    /// The other ports, in test order.
    pub fn others(&self) -> &[PortDesc] {
        &self.others
    }

    /// Every port in test order: the primary first.
    pub fn ports(&self) -> impl Iterator<Item = &PortDesc> {
        std::iter::once(&self.primary).chain(&self.others)
    }

    /// True when one of the ports is a USB-UART bridge (the catalog then has `port/uart-bridge`).
    pub fn has_bridge(&self) -> bool {
        self.ports()
            .any(|port| matches!(classify(port), PortClass::Bridge { .. }))
    }

    /// The board's ports for the report model, in test order (the port names are left out,
    /// spec D1).
    pub fn port_infos(&self) -> Vec<PortInfo> {
        self.ports()
            .enumerate()
            .filter_map(|(index, port)| port_info(port, index == 0))
            .collect()
    }
}

/// One port for the report: `usb_serial` is `None` when the VID:PID row says the OS makes the
/// serial number up (CH340, CH340K). `None` for a port that is neither native nor a bridge.
pub fn port_info(port: &PortDesc, primary: bool) -> Option<PortInfo> {
    let usb = port.usb.as_ref()?;
    let class = classify(port);
    let kind = match class {
        PortClass::Native => PortKind::Native,
        PortClass::Bridge { .. } => PortKind::Bridge,
        PortClass::JtagInterface { .. }
        | PortClass::EspressifApp { .. }
        | PortClass::NotUsb
        | PortClass::Unknown { .. } => {
            return None;
        }
    };
    Some(PortInfo {
        kind,
        vid: usb.vid,
        pid: usb.pid,
        bridge_model: class.bridge_model().map(SafeText::from_host),
        usb_serial: trusted_serial(port).map(SafeText::from_board),
        primary,
    })
}

/// Runs the v0.1 catalog on `board`, port by port, and returns the complete report.
///
/// 1. Primary port: [`sync_port`] and the chip read, then [`run_suites`](super::run_suites) on
///    that session (the revision it reads also goes to `board.chip.revision`), then the hard
///    reset. If the sync (or the chip read) fails, the board suites are SKIP
///    `not run: <primary port test> failed`, and the other ports are still tried.
/// 2. Each other port: a native port is first looked for again by USB serial number in
///    `enumerate()` (it re-enumerates after every hard reset); if it is not back within
///    [`REENUMERATE_TIMEOUT`], its port test is FAIL `port did not re-enumerate within 5 s`.
///    Otherwise [`sync_port`] and the hard reset.
/// 3. [`port_tests`] with every port; the report's hard reset is the last one attempted (it
///    decides the state the board is left in).
///
/// `probe` makes a fresh [`Probe`] per session; `enumerate` lists the ports (an OS error is an
/// empty list); `clock` times the suites and the wait; `sleeper` pauses between polls.
///
/// `observer` hears the run as it happens: [`Progress::Started`] when a port test or a suite
/// test begins, [`Progress::Waiting`] before waiting for a port to come back, and
/// [`Progress::Finished`] once per test of the report. A test's outcome is announced as soon as
/// it is final (the primary bridge right after its sync, the suite tests as they end); the rest
/// (the other ports, tests closed by an abort or skipped) right before returning, in report
/// order. The announced outcomes are the report's; the logic does not depend on the observer.
pub fn run_board(
    probe: &mut dyn FnMut(&PortDesc) -> Box<dyn Probe>,
    board: &Board,
    clock: &dyn Clock,
    sleeper: &dyn Sleeper,
    enumerate: &mut dyn FnMut() -> Vec<PortDesc>,
    observer: &mut dyn FnMut(Progress),
) -> Report {
    let mut syncs = Vec::with_capacity(1 + board.others().len());
    let mut announced: Vec<TestId> = Vec::new();
    let mut notify = |progress: Progress| {
        if let Progress::Finished(test) = &progress {
            announced.push(test.id);
        }
        observer(progress);
    };

    let primary_test = port_test_of(board.primary());
    notify(Progress::Started(primary_test));
    let mut session = probe(board.primary());
    let (sync, primary_chip) = sync_session(session.as_mut(), board.primary(), true);
    if primary_test == UART_BRIDGE {
        // Final already: it depends only on this sync and the board MAC (see `port_tests`).
        let caps = primary_chip.or(board.chip()).map(Chip::caps);
        if let Some(test) = port_tests(caps, Some(board.mac()), std::slice::from_ref(&sync))
            .into_iter()
            .find(|test| test.id == UART_BRIDGE)
        {
            notify(Progress::Finished(test));
        }
    }
    let suites =
        primary_chip.map(|chip| board_suites(session.as_mut(), chip.caps(), clock, &mut notify));
    let mut reset = after_session(HardReset::done(), session.hard_reset());
    drop(session);
    syncs.push(sync);
    let mut chip = primary_chip.or(board.chip());

    for port in board.others() {
        let current = match classify(port) {
            PortClass::Native => {
                notify(Progress::Started(USB_SERIAL_JTAG));
                notify(Progress::Waiting { port: port.clone() });
                await_port(port, enumerate, clock, sleeper)
            }
            _ => Ok(port.clone()),
        };
        match current {
            Ok(mut current) => {
                // A port that just came back (or a hub that re-enumerated after another board was
                // unplugged) may be listed before Windows lets it open: retry busy/gone opens,
                // re-finding the port by serial number, within the same re-enumeration window.
                let start = clock.now_ms();
                let max_tries = millis(REENUMERATE_TIMEOUT) / millis(POLL_INTERVAL).max(1) + 1;
                let mut tries = 0;
                let (sync, read) = loop {
                    tries += 1;
                    let mut session = probe(&current);
                    let (sync, read) = sync_session(session.as_mut(), &current, chip.is_none());
                    reset = after_session(reset, session.hard_reset());
                    drop(session);
                    let transient = matches!(
                        &sync.mac,
                        Err(SyncError::Probe(
                            ProbeError::Busy | ProbeError::Disconnected
                        ))
                    );
                    let elapsed = clock.now_ms().saturating_sub(start);
                    if !transient || tries >= max_tries || elapsed >= millis(REENUMERATE_TIMEOUT) {
                        break (sync, read);
                    }
                    sleeper.sleep(POLL_INTERVAL);
                    if let Some(found) = enumerate().into_iter().find(|candidate| {
                        candidate.usb.is_some()
                            && candidate.usb.as_ref().map(|u| (u.vid, u.pid, &u.serial))
                                == current.usb.as_ref().map(|u| (u.vid, u.pid, &u.serial))
                    }) {
                        current = found;
                    }
                };
                chip = chip.or(read);
                syncs.push(sync);
            }
            Err(waited_ms) => syncs.push(PortSync {
                port: port.clone(),
                mac: Err(SyncError::NotReenumerated),
                duration_ms: waited_ms,
            }),
        }
    }

    let caps = chip.map(Chip::caps);
    let (suites, facts) = match suites {
        Some((tests, facts)) => (Some(tests), facts),
        None => (None, SuiteFacts::default()),
    };
    // The probe fact becomes the model's own type here: `report` does not know `probe`.
    let revision = facts.revision.map(|read| Revision {
        major: read.major,
        minor: read.minor,
    });
    let info = BoardInfo {
        chip: chip.map(|chip| ChipInfo {
            chip,
            description: None,
            revision,
        }),
        mac: Some(SafeText::from_host(&board.mac().to_string())),
        ports: board.port_infos(),
    };
    let mut run = Run::new(
        env!("CARGO_PKG_VERSION"),
        info,
        catalog(caps, board.has_bridge()),
    );
    let suite_tests = suites.unwrap_or_else(|| {
        let cause = primary_test;
        let reason = format!("not run: {cause} failed");
        BOARD_TESTS
            .iter()
            .map(|id| TestResult {
                id: *id,
                outcome: Outcome::skip(&reason),
            })
            .collect()
    });
    for test in port_tests(caps, Some(board.mac()), &syncs)
        .into_iter()
        .chain(suite_tests)
    {
        let finished = run.finish(test.id, test.outcome);
        debug_assert!(finished.is_ok(), "runner bug: {finished:?}");
    }
    let report = run.complete(reset);
    for test in report.tests() {
        if !announced.contains(&test.id) {
            observer(Progress::Finished(test.clone()));
        }
    }
    report
}

/// The `port` test a port answers for: `port/usb-serial-jtag` on the native port,
/// `port/uart-bridge` otherwise.
fn port_test_of(port: &PortDesc) -> TestId {
    match classify(port) {
        PortClass::Native => USB_SERIAL_JTAG,
        _ => UART_BRIDGE,
    }
}

/// [`sync_port`], then (when `want_chip`) the chip read on the same session. A failed chip read
/// fails the sync: without the chip no suite can run through this port.
fn sync_session(
    probe: &mut dyn Probe,
    port: &PortDesc,
    want_chip: bool,
) -> (PortSync, Option<Chip>) {
    let mut sync = sync_port(probe, port);
    if !want_chip || sync.mac.is_err() {
        return (sync, None);
    }
    match probe.chip() {
        Ok(chip) => (sync, Some(chip)),
        Err(error) => {
            sync.mac = Err(SyncError::Probe(error));
            (sync, None)
        }
    }
}

/// The board suites on an open session, in a run of their own: an aborted session stops the
/// suites (`not run`) but never the port tests of the other ports. Also returns the facts the
/// suites noted for the board (the revision), kept even when the session was aborted later.
fn board_suites(
    probe: &mut dyn Probe,
    caps: &ChipCaps,
    clock: &dyn Clock,
    observer: &mut dyn FnMut(Progress),
) -> (Vec<TestResult>, SuiteFacts) {
    let board = BoardInfo {
        chip: None,
        mac: None,
        ports: Vec::new(),
    };
    let mut run = Run::new(env!("CARGO_PKG_VERSION"), board, BOARD_TESTS.to_vec());
    // An interruption is already recorded in `run` (abort); the caller does the hard reset.
    let mut facts = SuiteFacts::default();
    let _interrupted = run_suites_noting(probe, caps, clock, &mut run, observer, &mut facts);
    (run.complete(HardReset::done()).tests().to_vec(), facts)
}

/// The board's state after a session's hard reset. A session that never opened (the port was
/// busy or gone, so the reset answers `NotConnected`) never touched the board: the state left by
/// the previous reset stands, instead of claiming the board did not get back to its app.
fn after_session(previous: HardReset, result: Result<(), ProbeError>) -> HardReset {
    match result {
        Err(ProbeError::NotConnected) => previous,
        other => reset_outcome(other),
    }
}

fn reset_outcome(result: Result<(), ProbeError>) -> HardReset {
    match result {
        Ok(()) => HardReset::done(),
        Err(error) => HardReset::not_done(&error.to_string()),
    }
}

/// Waits for `port` to be listed again with the same VID:PID and USB serial number (the name may
/// change). `Err(waited ms)` after [`REENUMERATE_TIMEOUT`]. A port without a serial number
/// cannot be told apart, so it is used as it was enumerated.
fn await_port(
    port: &PortDesc,
    enumerate: &mut dyn FnMut() -> Vec<PortDesc>,
    clock: &dyn Clock,
    sleeper: &dyn Sleeper,
) -> Result<PortDesc, u64> {
    let Some(usb) = port.usb.as_ref().filter(|usb| usb.serial.is_some()) else {
        return Ok(port.clone());
    };
    let is_same = |candidate: &PortDesc| {
        candidate.usb.as_ref().is_some_and(|other| {
            other.vid == usb.vid && other.pid == usb.pid && other.serial == usb.serial
        })
    };
    let timeout = millis(REENUMERATE_TIMEOUT);
    let grace = millis(VANISH_GRACE);
    // Bounded even if the clock stood still: one poll per interval, plus the first and last.
    let max_polls = timeout / millis(POLL_INTERVAL).max(1) + 2;
    let start = clock.now_ms();
    let mut seen_gone = false;
    let mut elapsed = 0;
    for _ in 0..max_polls {
        elapsed = clock.now_ms().saturating_sub(start);
        match enumerate().into_iter().find(|candidate| is_same(candidate)) {
            Some(found) if seen_gone || elapsed >= grace => return Ok(found),
            Some(_) => {}
            None => seen_gone = true,
        }
        if elapsed >= timeout {
            break;
        }
        sleeper.sleep(POLL_INTERVAL);
    }
    Err(elapsed)
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, VecDeque};
    use std::rc::Rc;

    use super::*;
    use crate::chips::ChipIdentity;
    use crate::probe::fake::{FakeProbe, Step};
    use crate::probe::{EmbeddedMem, FlashId, Revision};
    use crate::report::State;
    use crate::transport::UsbIds;

    const C6_MAC: &str = "e8:f6:0a:f7:ff:d4";
    const C3_MAC: &str = "80:53:e0:0a:5d:70";
    const ESP32_MAC: &str = "1c:c3:ab:d2:25:b4";

    fn usb(name: &str, vid: u16, pid: u16, serial: Option<&str>, iface: Option<u8>) -> PortDesc {
        PortDesc {
            name: name.to_owned(),
            usb: Some(UsbIds {
                vid,
                pid,
                serial: serial.map(str::to_owned),
                manufacturer: None,
                product: None,
                interface: iface,
            }),
        }
    }

    fn native(name: &str, serial: &str) -> PortDesc {
        usb(name, 0x303A, 0x1001, Some(serial), Some(0))
    }

    fn ch343(name: &str) -> PortDesc {
        usb(name, 0x1A86, 0x55D3, Some("5D28088406"), None)
    }

    fn ch340k(name: &str) -> PortDesc {
        // Windows makes the serial number up from the hub position (hardware.md).
        usb(name, 0x1A86, 0x7522, Some("6"), None)
    }

    fn ch9102(name: &str) -> PortDesc {
        usb(name, 0x1A86, 0x55D4, Some("5B53026573"), None)
    }

    fn mac(text: &str) -> Mac {
        Mac::parse(text).unwrap()
    }

    fn chip(target: &str) -> Chip {
        Chip::from_target(target).unwrap()
    }

    fn fake(fixture: &str) -> FakeProbe {
        FakeProbe::from_fixture(fixture).unwrap()
    }

    type Log = Rc<RefCell<Vec<(String, Step)>>>;

    /// A [`FakeProbe`] that writes every call, with the name of the port it was made for, to a
    /// log shared with the test (the probe itself is moved into the code under test).
    struct Logged {
        name: String,
        inner: FakeProbe,
        log: Log,
    }

    impl Logged {
        fn note(&self, step: Step) {
            self.log.borrow_mut().push((self.name.clone(), step));
        }
    }

    impl Probe for Logged {
        fn connect(&mut self, port: &PortDesc) -> Result<(), ProbeError> {
            self.note(Step::Connect);
            self.inner.connect(port)
        }
        fn chip(&mut self) -> Result<Chip, ProbeError> {
            self.note(Step::Chip);
            self.inner.chip()
        }
        fn identity(&mut self) -> Result<ChipIdentity, ProbeError> {
            self.note(Step::Identity);
            self.inner.identity()
        }
        fn revision(&mut self) -> Result<Revision, ProbeError> {
            self.note(Step::Revision);
            self.inner.revision()
        }
        fn crystal_mhz(&mut self) -> Result<u32, ProbeError> {
            self.note(Step::CrystalMhz);
            self.inner.crystal_mhz()
        }
        fn mac(&mut self) -> Result<Mac, ProbeError> {
            self.note(Step::Mac);
            self.inner.mac()
        }
        fn read_reg(&mut self, addr: u32) -> Result<u32, ProbeError> {
            self.note(Step::ReadReg);
            self.inner.read_reg(addr)
        }
        fn package_version(&mut self) -> Result<u32, ProbeError> {
            self.note(Step::PackageVersion);
            self.inner.package_version()
        }
        fn embedded_flash(&mut self) -> Result<Option<EmbeddedMem>, ProbeError> {
            self.note(Step::EmbeddedFlash);
            self.inner.embedded_flash()
        }
        fn embedded_psram(&mut self) -> Result<Option<EmbeddedMem>, ProbeError> {
            self.note(Step::EmbeddedPsram);
            self.inner.embedded_psram()
        }
        fn flash_id(&mut self) -> Result<FlashId, ProbeError> {
            self.note(Step::FlashId);
            self.inner.flash_id()
        }
        fn hard_reset(&mut self) -> Result<(), ProbeError> {
            self.note(Step::HardReset);
            self.inner.hard_reset()
        }
    }

    /// The boards on the bench: which fake answers behind each port name.
    struct Bench {
        boards: HashMap<String, FakeProbe>,
        log: Log,
    }

    impl Bench {
        fn new(boards: &[(&str, FakeProbe)]) -> Bench {
            Bench {
                boards: boards
                    .iter()
                    .map(|(name, probe)| ((*name).to_owned(), probe.clone()))
                    .collect(),
                log: Rc::default(),
            }
        }

        /// The probe factory: a fresh copy of the board behind the port's name.
        fn factory(&self) -> impl FnMut(&PortDesc) -> Box<dyn Probe> + '_ {
            |port: &PortDesc| -> Box<dyn Probe> {
                let inner = self
                    .boards
                    .get(&port.name)
                    .unwrap_or_else(|| panic!("no board behind {}", port.name))
                    .clone();
                Box::new(Logged {
                    name: port.name.clone(),
                    inner,
                    log: Rc::clone(&self.log),
                })
            }
        }

        fn calls(&self) -> Vec<(String, Step)> {
            self.log.borrow().clone()
        }

        fn steps_on(&self, name: &str) -> Vec<Step> {
            self.calls()
                .into_iter()
                .filter(|(port, _)| port == name)
                .map(|(_, step)| step)
                .collect()
        }
    }

    /// Time stands still until someone sleeps; sleeping advances it exactly.
    #[derive(Default)]
    struct FakeTime {
        now: Cell<u64>,
        sleeps: Cell<u32>,
    }

    impl Clock for FakeTime {
        fn now_ms(&self) -> u64 {
            self.now.get()
        }
    }

    impl Sleeper for FakeTime {
        fn sleep(&self, duration: Duration) {
            self.now.set(self.now.get() + millis(duration));
            self.sleeps.set(self.sleeps.get() + 1);
        }
    }

    /// An OS port list that changes on every read; the last list repeats forever.
    fn script(lists: Vec<Vec<PortDesc>>) -> impl FnMut() -> Vec<PortDesc> {
        let mut lists: VecDeque<Vec<PortDesc>> = lists.into();
        move || {
            if lists.len() > 1 {
                lists.pop_front().unwrap_or_default()
            } else {
                lists.front().cloned().unwrap_or_default()
            }
        }
    }

    fn names<'a>(ports: impl Iterator<Item = &'a PortDesc>) -> Vec<&'a str> {
        ports.map(|port| port.name.as_str()).collect()
    }

    fn outcome(report: &Report, id: crate::report::TestId) -> &Outcome {
        &report
            .tests()
            .iter()
            .find(|test| test.id == id)
            .unwrap_or_else(|| panic!("{id} missing"))
            .outcome
    }

    /// The C6 of the bench with both cables: CH343 on "COM13", native on "COM11".
    fn c6_ports() -> Vec<PortDesc> {
        vec![native("COM11", "E8:F6:0A:F7:FF:D4"), ch343("COM13")]
    }

    #[test]
    fn groups_native_and_bridge_ports_of_one_board() {
        let ports = c6_ports();
        let grouping = pre_group(&ports);
        assert_eq!(grouping.groups.len(), 1);
        assert_eq!(grouping.groups[0].mac, mac(C6_MAC));
        assert_eq!(names(grouping.unidentified.iter()), ["COM13"]);
        assert!(grouping.others.is_empty());

        let bench = Bench::new(&[("COM13", fake("esp32c6"))]);
        let identified = identify(&mut bench.factory(), &grouping.unidentified);
        // Only the bridge is opened, once: connect, MAC, chip, hard reset.
        assert_eq!(
            bench.steps_on("COM13"),
            [Step::Connect, Step::Mac, Step::Chip, Step::HardReset]
        );
        assert_eq!(bench.calls().len(), 4);

        let boards = merge(&grouping, &identified);
        assert_eq!(boards.len(), 1);
        let board = &boards[0];
        assert_eq!(board.mac(), mac(C6_MAC));
        assert_eq!(board.chip(), Some(chip("esp32c6")));
        assert_eq!(names(board.ports()), ["COM13", "COM11"]);
        assert_eq!(board.primary().name, "COM13");
        assert!(board.has_bridge());
    }

    #[test]
    fn each_group_has_exactly_one_primary_port() {
        let c6 = mac(C6_MAC);
        let cases = [
            (vec![native("N", C6_MAC), ch343("B")], "B"),
            (vec![ch343("B"), native("N", C6_MAC)], "B"),
            (vec![native("N", C6_MAC)], "N"),
            (vec![ch343("B")], "B"),
        ];
        for (ports, primary) in cases {
            let board = Board::new(c6, None, ports).unwrap();
            assert_eq!(board.primary().name, primary);
            let infos = board.port_infos();
            assert_eq!(infos.iter().filter(|info| info.primary).count(), 1);
            assert!(infos[0].primary, "primary is listed first");
            if infos.len() == 2 {
                assert_eq!(infos[0].kind, PortKind::Bridge);
                assert_eq!(infos[1].kind, PortKind::Native);
            }
        }
        assert_eq!(Board::new(c6, None, Vec::new()), None);
        let jtag = usb("J", 0x0403, 0x6010, None, Some(0));
        assert_eq!(Board::new(c6, None, vec![jtag]), None);
    }

    #[test]
    fn bridge_without_native_pair_is_its_own_board() {
        // The original ESP32 has no native port: its CH9102 is the whole board.
        let ports = [ch9102("COM5")];
        let grouping = pre_group(&ports);
        assert!(grouping.groups.is_empty());
        let bench = Bench::new(&[("COM5", fake("esp32"))]);
        let identified = identify(&mut bench.factory(), &grouping.unidentified);
        let boards = merge(&grouping, &identified);
        assert_eq!(boards.len(), 1);
        assert_eq!(boards[0].mac(), mac(ESP32_MAC));
        assert_eq!(boards[0].chip(), Some(chip("esp32")));
        assert_eq!(names(boards[0].ports()), ["COM5"]);
        assert!(boards[0].others().is_empty());
    }

    #[test]
    fn two_boards_of_the_same_chip_stay_apart() {
        // Two C3 with a CH340K each: the bridges' serial numbers are both made up and say
        // nothing; only the MAC read through each bridge tells which native port it belongs to.
        let other_mac = mac("80:53:e0:0a:5d:ac");
        let ports = vec![
            native("COM6", "80:53:E0:0A:5D:70"),
            native("COM8", "80:53:E0:0A:5D:AC"),
            ch340k("COM10"),
            ch340k("COM12"),
        ];
        let grouping = pre_group(&ports);
        assert_eq!(grouping.groups.len(), 2);
        let bench = Bench::new(&[
            ("COM10", fake("esp32c3").with_mac(other_mac)),
            ("COM12", fake("esp32c3")),
        ]);
        let identified = identify(&mut bench.factory(), &grouping.unidentified);
        let boards = merge(&grouping, &identified);
        assert_eq!(boards.len(), 2);
        assert_eq!(boards[0].mac(), mac(C3_MAC));
        assert_eq!(names(boards[0].ports()), ["COM12", "COM6"]);
        assert_eq!(boards[1].mac(), other_mac);
        assert_eq!(names(boards[1].ports()), ["COM10", "COM8"]);
    }

    #[test]
    fn native_with_unreadable_serial_is_identified_like_a_bridge() {
        for serial in [None, Some("5"), Some("\u{1b}[2J")] {
            let port = usb("COM11", 0x303A, 0x1001, serial, Some(0));
            let grouping = pre_group(std::slice::from_ref(&port));
            assert!(grouping.groups.is_empty(), "{serial:?}");
            assert_eq!(grouping.unidentified, std::slice::from_ref(&port));

            let bench = Bench::new(&[("COM11", fake("esp32c6"))]);
            let identified = identify(&mut bench.factory(), &grouping.unidentified);
            let boards = merge(&grouping, &identified);
            assert_eq!(boards.len(), 1);
            assert_eq!(boards[0].mac(), mac(C6_MAC));
            assert_eq!(boards[0].primary(), &port);
        }
    }

    #[test]
    fn failed_identification_keeps_bridge_out_with_reason() {
        let ports = c6_ports();
        let grouping = pre_group(&ports);
        let bench = Bench::new(&[(
            "COM13",
            fake("esp32c6").fail_at(Step::Connect, ProbeError::Timeout),
        )]);
        let identified = identify(&mut bench.factory(), &grouping.unidentified);
        assert_eq!(
            identified,
            [Identified::Failed {
                port: ch343("COM13"),
                reason: "identification failed: timeout".to_owned(),
            }]
        );
        // Two sessions (the timeout is retried once); the hard reset is attempted in each even
        // though the session never opened.
        assert_eq!(
            bench.steps_on("COM13"),
            [
                Step::Connect,
                Step::HardReset,
                Step::Connect,
                Step::HardReset
            ]
        );
        let boards = merge(&grouping, &identified);
        assert_eq!(boards.len(), 1);
        assert_eq!(names(boards[0].ports()), ["COM11"]);
    }

    /// Bench, 2026-11-26: the ESP32 DevKit V1 (CH9102) timed out once on identification. The
    /// first session times out, the second syncs: the bridge is found.
    #[test]
    fn bridge_timeout_is_retried_once_with_a_new_session() {
        let log: Log = Rc::default();
        let mut sessions: VecDeque<FakeProbe> = VecDeque::from([
            fake("esp32").fail_at(Step::Connect, ProbeError::Timeout),
            fake("esp32"),
        ]);
        let mut factory = |port: &PortDesc| -> Box<dyn Probe> {
            let inner = sessions.pop_front().expect("a third session was opened");
            Box::new(Logged {
                name: port.name.clone(),
                inner,
                log: Rc::clone(&log),
            })
        };
        let identified = identify(&mut factory, &[ch9102("COM9")]);
        assert_eq!(
            identified,
            [Identified::Found {
                port: ch9102("COM9"),
                mac: mac(ESP32_MAC),
                chip: Some(chip("esp32")),
                reset: HardReset::done(),
            }]
        );
        let steps: Vec<Step> = log.borrow().iter().map(|(_, step)| *step).collect();
        assert_eq!(
            steps,
            [
                Step::Connect,
                Step::HardReset,
                Step::Connect,
                Step::Mac,
                Step::Chip,
                Step::HardReset
            ]
        );
    }

    /// Only a sync timeout is retried: a busy or vanished port fails at once, in one session.
    #[test]
    fn busy_or_gone_bridge_is_not_retried() {
        for error in [ProbeError::Busy, ProbeError::Disconnected] {
            let bench =
                Bench::new(&[("COM9", fake("esp32").fail_at(Step::Connect, error.clone()))]);
            let identified = identify(&mut bench.factory(), &[ch9102("COM9")]);
            assert!(
                matches!(&identified[..], [Identified::Failed { .. }]),
                "{error:?}"
            );
            assert_eq!(
                bench.steps_on("COM9"),
                [Step::Connect, Step::HardReset],
                "{error:?}"
            );
        }
    }

    #[test]
    fn unknown_and_jtag_ports_are_listed_never_probed() {
        let ports = vec![
            usb("COM3", 0xDEAD, 0xBEEF, Some("E8:F6:0A:F7:FF:D4"), None),
            PortDesc {
                name: "COM1".to_owned(),
                usb: None,
            },
            usb("COM20", 0x0403, 0x6010, None, Some(0)),
            usb("COM21", 0x0403, 0x6010, None, None),
            // S3 app on the OTG controller (TinyUSB): its serial "123456" is not a MAC.
            usb("COM14", 0x303A, 0x4001, Some("123456"), Some(0)),
        ];
        let grouping = pre_group(&ports);
        assert!(grouping.groups.is_empty());
        assert!(grouping.unidentified.is_empty());
        assert_eq!(grouping.others, ports);

        let bench = Bench::new(&[]);
        let identified = identify(&mut bench.factory(), &grouping.unidentified);
        assert!(identified.is_empty());
        // Even if asked directly, none of them is opened.
        let identified = identify(&mut bench.factory(), &ports);
        assert_eq!(identified.len(), ports.len());
        assert!(
            identified
                .iter()
                .all(|result| matches!(result, Identified::Failed { .. }))
        );
        assert!(bench.calls().is_empty());
        assert!(merge(&grouping, &identified).is_empty());
    }

    #[test]
    fn ch340k_serial_is_not_trusted() {
        for port in [
            ch340k("COM10"),
            usb("COM9", 0x1A86, 0x7523, Some("5"), None),
        ] {
            assert_eq!(trusted_serial(&port), None, "{}", port.name);
            let info = port_info(&port, true).unwrap();
            assert_eq!(info.usb_serial, None);
            assert!(info.bridge_model.is_some());
        }
        let info = port_info(&ch343("COM13"), true).unwrap();
        assert_eq!(
            info.usb_serial.as_ref().map(SafeText::as_str),
            Some("5D28088406")
        );
        let info = port_info(&native("COM11", "E8:F6:0A:F7:FF:D4"), false).unwrap();
        assert_eq!(
            info.usb_serial.as_ref().map(SafeText::as_str),
            Some("E8:F6:0A:F7:FF:D4")
        );
        assert_eq!(info.bridge_model, None);
    }

    /// The C6 board of the bench, after identification.
    fn c6_board() -> Board {
        Board::new(
            mac(C6_MAC),
            Some(chip("esp32c6")),
            vec![native("COM11", "E8:F6:0A:F7:FF:D4"), ch343("COM13")],
        )
        .unwrap()
    }

    #[test]
    fn run_board_tests_both_ports_and_suites_once() {
        let board = c6_board();
        // The bridge's hard reset drops the native port; it comes back under another name.
        let back = native("COM12", "E8:F6:0A:F7:FF:D4");
        let mut enumerate = script(vec![
            vec![ch343("COM13"), native("COM11", "E8:F6:0A:F7:FF:D4")],
            vec![ch343("COM13")],
            vec![ch343("COM13")],
            vec![ch343("COM13"), back],
        ]);
        let bench = Bench::new(&[("COM13", fake("esp32c6")), ("COM12", fake("esp32c6"))]);
        let time = FakeTime::default();
        let report = run_board(
            &mut bench.factory(),
            &board,
            &time,
            &time,
            &mut enumerate,
            &mut |_| {},
        );

        assert_eq!(
            report.summary().to_string(),
            "Tests: 0 failed, 11 passed, 0 skipped, 11 total"
        );
        assert_eq!(report.exit_code(), 0);
        assert!(report.hard_reset().is_done());
        for id in [UART_BRIDGE, USB_SERIAL_JTAG] {
            assert_eq!(
                outcome(&report, id).read().map(SafeText::as_str),
                Some(C6_MAC)
            );
        }

        // Primary first, with every suite step exactly once; then the native port, found by
        // serial number under its new name; the old name is never opened.
        let calls = bench.calls();
        let order: Vec<&str> = calls.iter().map(|(name, _)| name.as_str()).collect();
        let split = order.iter().position(|name| *name == "COM12").unwrap();
        assert!(order[..split].iter().all(|name| *name == "COM13"));
        assert!(order[split..].iter().all(|name| *name == "COM12"));
        let primary = bench.steps_on("COM13");
        assert_eq!(primary.first(), Some(&Step::Connect));
        assert_eq!(primary.last(), Some(&Step::HardReset));
        for step in [
            Step::Identity,
            Step::Revision,
            Step::CrystalMhz,
            Step::PackageVersion,
            Step::EmbeddedPsram,
            Step::EmbeddedFlash,
            Step::FlashId,
        ] {
            let count = calls.iter().filter(|(_, s)| *s == step).count();
            assert_eq!(count, 1, "{step:?}");
        }
        assert_eq!(
            bench.steps_on("COM12"),
            [Step::Connect, Step::Mac, Step::HardReset]
        );
        // Three polls waited for: present (not yet gone), gone, gone, back.
        assert_eq!(time.sleeps.get(), 3);

        let ports = &report.board().ports;
        assert_eq!(ports.len(), 2);
        assert_eq!((ports[0].kind, ports[0].primary), (PortKind::Bridge, true));
        assert_eq!((ports[1].kind, ports[1].primary), (PortKind::Native, false));
        assert_eq!(
            report.board().mac.as_ref().map(SafeText::as_str),
            Some(C6_MAC)
        );
        assert_eq!(
            report.board().chip.as_ref().map(|info| info.chip),
            Some(chip("esp32c6"))
        );
    }

    #[test]
    fn native_port_that_never_returns_fails_its_port_test() {
        let board = c6_board();
        let mut enumerate = script(vec![vec![ch343("COM13")]]);
        let bench = Bench::new(&[("COM13", fake("esp32c6"))]);
        let time = FakeTime::default();
        let report = run_board(
            &mut bench.factory(),
            &board,
            &time,
            &time,
            &mut enumerate,
            &mut |_| {},
        );

        let native = outcome(&report, USB_SERIAL_JTAG);
        assert_eq!(native.state(), State::Fail);
        assert_eq!(
            native.reason().map(SafeText::as_str),
            Some("port did not re-enumerate within 5 s")
        );
        assert_eq!(native.duration_ms(), 5_000);
        assert_eq!(outcome(&report, UART_BRIDGE).state(), State::Pass);
        assert_eq!(
            report.summary().to_string(),
            "Tests: 1 failed, 10 passed, 0 skipped, 11 total"
        );
        assert_eq!(report.exit_code(), 1);
        // Only the bridge was ever opened; the wait stopped at the timeout.
        assert!(bench.calls().iter().all(|(name, _)| name == "COM13"));
        assert_eq!(time.now.get(), 5_000);
        assert_eq!(time.sleeps.get(), 25);
    }

    /// Bench, 2026-11-26: unplugging another board from the same hub made the C6 native port
    /// listed-but-not-openable for a moment; one try failed in 5 ms. It is retried by serial.
    #[test]
    fn native_port_busy_for_a_moment_is_retried_and_passes() {
        let board = c6_board();
        let mut enumerate = script(vec![
            vec![ch343("COM13")],
            vec![ch343("COM13"), native("COM11", "E8:F6:0A:F7:FF:D4")],
            vec![ch343("COM13"), native("COM12", "E8:F6:0A:F7:FF:D4")],
        ]);
        let bench = Bench::new(&[
            ("COM13", fake("esp32c6")),
            (
                "COM11",
                fake("esp32c6").fail_at(Step::Connect, ProbeError::Busy),
            ),
            ("COM12", fake("esp32c6")),
        ]);
        let time = FakeTime::default();
        let report = run_board(
            &mut bench.factory(),
            &board,
            &time,
            &time,
            &mut enumerate,
            &mut |_| {},
        );

        assert_eq!(outcome(&report, USB_SERIAL_JTAG).state(), State::Pass);
        assert_eq!(report.summary().failed, 0);
        assert!(report.hard_reset().is_done());
    }

    /// A port that stays busy (a serial monitor has it open) fails with a reason that says so,
    /// and the board is still reported back to its app: that port never opened a session.
    #[test]
    fn native_port_kept_busy_fails_with_reason_and_board_is_still_back() {
        let board = c6_board();
        let mut enumerate = script(vec![
            vec![ch343("COM13")],
            vec![ch343("COM13"), native("COM11", "E8:F6:0A:F7:FF:D4")],
        ]);
        let bench = Bench::new(&[
            ("COM13", fake("esp32c6")),
            (
                "COM11",
                fake("esp32c6").fail_at(Step::Connect, ProbeError::Busy),
            ),
        ]);
        let time = FakeTime::default();
        let report = run_board(
            &mut bench.factory(),
            &board,
            &time,
            &time,
            &mut enumerate,
            &mut |_| {},
        );

        let native = outcome(&report, USB_SERIAL_JTAG);
        assert_eq!(native.state(), State::Fail);
        let reason = native.reason().map(SafeText::as_str).unwrap_or_default();
        assert!(reason.contains("port busy"), "{reason}");
        assert!(
            report.hard_reset().is_done(),
            "{:?}",
            report.hard_reset().reason()
        );
        // Bounded: retried within the re-enumeration window, never forever.
        assert!(time.now.get() <= 5_000 + 1_000 + 200);
    }

    #[test]
    fn primary_sync_failure_skips_suites_and_still_tries_the_native_port() {
        let board = c6_board();
        let mut enumerate = script(vec![
            vec![],
            vec![ch343("COM13"), native("COM11", "E8:F6:0A:F7:FF:D4")],
        ]);
        let bench = Bench::new(&[
            (
                "COM13",
                fake("esp32c6").fail_at(Step::Connect, ProbeError::Timeout),
            ),
            ("COM11", fake("esp32c6")),
        ]);
        let time = FakeTime::default();
        let report = run_board(
            &mut bench.factory(),
            &board,
            &time,
            &time,
            &mut enumerate,
            &mut |_| {},
        );

        let bridge = outcome(&report, UART_BRIDGE);
        assert_eq!(bridge.state(), State::Fail);
        assert_eq!(
            bridge.reason().map(SafeText::as_str),
            Some("sync failed: timeout")
        );
        assert_eq!(outcome(&report, USB_SERIAL_JTAG).state(), State::Pass);
        for id in BOARD_TESTS {
            let test = outcome(&report, id);
            assert_eq!(test.state(), State::Skip, "{id}");
            assert_eq!(
                test.reason().map(SafeText::as_str),
                Some("not run: port/uart-bridge failed"),
                "{id}"
            );
        }
        assert_eq!(bench.steps_on("COM13"), [Step::Connect, Step::HardReset]);
        // The chip came from identification, so the native session only syncs.
        assert_eq!(
            bench.steps_on("COM11"),
            [Step::Connect, Step::Mac, Step::HardReset]
        );
        assert_eq!(report.exit_code(), 1);
        assert!(report.hard_reset().is_done());
    }

    #[test]
    fn unknown_chip_still_gives_a_complete_report() {
        // A bridge-only board whose sync fails and whose chip was never read.
        let board = Board::new(mac(ESP32_MAC), None, vec![ch9102("COM5")]).unwrap();
        let bench = Bench::new(&[(
            "COM5",
            fake("esp32").fail_at(Step::Connect, ProbeError::Busy),
        )]);
        let time = FakeTime::default();
        let mut enumerate = script(vec![vec![]]);
        let report = run_board(
            &mut bench.factory(),
            &board,
            &time,
            &time,
            &mut enumerate,
            &mut |_| {},
        );
        assert_eq!(report.board().chip, None);
        assert_eq!(report.tests().len(), 11);
        assert_eq!(
            outcome(&report, USB_SERIAL_JTAG)
                .reason()
                .map(SafeText::as_str),
            Some("not run: port/uart-bridge failed")
        );
        assert_eq!(
            report.summary().to_string(),
            "Tests: 1 failed, 0 passed, 10 skipped, 11 total"
        );
    }

    #[test]
    fn report_board_info_has_revision() {
        let s3 = || {
            Board::new(
                mac("10:51:db:84:14:0c"),
                Some(chip("esp32s3")),
                vec![ch343("COM12")],
            )
            .unwrap()
        };
        let run = |probe: FakeProbe| {
            let bench = Bench::new(&[("COM12", probe)]);
            let time = FakeTime::default();
            run_board(
                &mut bench.factory(),
                &s3(),
                &time,
                &time,
                &mut script(vec![vec![ch343("COM12")]]),
                &mut |_| {},
            )
        };
        let revision = |report: &Report| {
            report
                .board()
                .chip
                .as_ref()
                .and_then(|info| info.revision)
                .map(|revision| (revision.major, revision.minor))
        };

        // Read once by `rom/revision`: the same fact in the test and in `board.chip`.
        let report = run(fake("esp32s3"));
        assert_eq!(revision(&report), Some((0, 2)));
        assert_eq!(
            outcome(&report, crate::runner::REVISION)
                .read()
                .map(SafeText::as_str),
            Some("v0.2")
        );
        // The probe has no description fact yet.
        assert_eq!(
            report.board().chip.as_ref().map(|info| &info.description),
            Some(&None)
        );

        // Not read: the chip is still known, the revision is not made up.
        let failing = fake("esp32s3").fail_at(Step::Revision, ProbeError::Protocol("bad".into()));
        let report = run(failing);
        assert_eq!(
            report.board().chip.as_ref().map(|info| info.chip),
            Some(chip("esp32s3"))
        );
        assert_eq!(revision(&report), None);
        // An aborted session after the revision read keeps it.
        let report = run(fake("esp32s3").fail_at(Step::FlashId, ProbeError::Timeout));
        assert_eq!(revision(&report), Some((0, 2)));
    }
}
