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
//! # Board suites (H4, H5)
//! [`run_suites`] runs `rom`, `efuse` and `flash` ([`BOARD_TESTS`]) on the session left open by
//! [`sync_port`] on the primary port, timing each test with a [`Clock`]. The value texts come
//! from the same `Display`/`FieldView` notation as the capability table, so a comparison is a
//! plain string comparison done by `report::Outcome::from_comparison`. [`catalog`] gives the
//! whole plan for `report::Run::new`, in spec order, so no test can go missing from a report.
//!
//! # One board, all its ports (H2, ADR-008)
//! [`group`] finds the boards behind the enumerated ports (by MAC, never by port name) and
//! [`run_board`] tests one of them in sequence: primary port first, then the others, waiting
//! for the native port to re-enumerate after each hard reset.
//!
//!
//! # Threads and events (#20, ADR-011 §5)
//! The functions above are sequential and blocking. [`run_board`] and [`run_suites`] report what
//! they do to an observer (`&mut dyn FnMut(Progress)`) without changing their logic;
//! [`worker`] runs them on a thread of their own and turns that [`Progress`] into
//! [`worker::Event`]s for the UI, and [`watch`] lists the ports on another thread (hot-plug,
//! never opening a port).

pub mod group;
pub mod watch;
pub mod worker;

pub use self::group::{
    Board, Grouping, Identified, POLL_INTERVAL, PreGroup, REENUMERATE_TIMEOUT, identify, merge,
    pre_group, run_board,
};

use std::fmt;
use std::time::{Duration, Instant};

use crate::chips::{ChipCaps, mhz_text};
use crate::probe::{EmbeddedMem, Probe, ProbeError};
use crate::report::{Outcome, Run, SafeText, State, Suite, TestId, TestResult};
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

/// What the runner is doing, reported while it does it (the threaded runner turns it into UI
/// events). Pure notification: the [`Report`](crate::report::Report) stays the source of truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// Test `id` is in flight.
    Started(TestId),
    /// Test `id` ended with this outcome (the same one the report will carry).
    Finished(TestResult),
    /// The runner is waiting for `port` to come back after a hard reset (it re-enumerates).
    Waiting {
        /// The port as it was enumerated before the reset.
        port: PortDesc,
    },
}

/// Why a port of the board gave no MAC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncError {
    /// The ROM session through this port failed.
    Probe(ProbeError),
    /// The port did not come back within [`REENUMERATE_TIMEOUT`] after the hard reset of an
    /// earlier session, so it was never opened.
    NotReenumerated,
}

impl fmt::Display for SyncError {
    /// The `reason` of the FAIL in the `port` suite (host text, English).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncError::Probe(error) => write!(f, "sync failed: {error}"),
            SyncError::NotReenumerated => write!(
                f,
                "port did not re-enumerate within {} s",
                REENUMERATE_TIMEOUT.as_secs()
            ),
        }
    }
}

impl std::error::Error for SyncError {}

impl From<ProbeError> for SyncError {
    fn from(error: ProbeError) -> Self {
        SyncError::Probe(error)
    }
}

/// What syncing through one port of the board gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortSync {
    /// The port.
    pub port: PortDesc,
    /// MAC read from the ROM through this port, or why there is none.
    pub mac: Result<Mac, SyncError>,
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
        mac: mac.map_err(SyncError::Probe),
        duration_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

/// The `port` suite of one board, in catalog order (`uart-bridge`, then `usb-serial-jtag`).
///
/// `caps` is the chip row (`None` when no port could identify the chip), `board_mac` the board
/// identity (ADR-008 §1) and `syncs` one entry per port of the group. At most one port of each
/// kind is expected; if there are more, the first one counts. Without `caps` and without a
/// native port, `port/usb-serial-jtag` cannot be decided: it is SKIP `not run: <first port
/// test> failed`, pointing at the port whose sync failed.
pub fn port_tests(
    caps: Option<&ChipCaps>,
    board_mac: Option<Mac>,
    syncs: &[PortSync],
) -> Vec<TestResult> {
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

    let outcome = match (caps, native) {
        (Some(caps), None) if caps.usb_serial_jtag.value => {
            Outcome::skip(SKIP_NATIVE_NOT_CONNECTED)
        }
        (Some(_), None) => Outcome::skip(SKIP_NO_USB_SERIAL_JTAG),
        (None, None) => Outcome::skip(&format!("not run: {UART_BRIDGE} failed")),
        // A native port answered with this board's MAC, but the table says the chip has no
        // USB-Serial/JTAG: the table or the identification is wrong (invariant finding).
        (Some(caps), Some(sync)) if !caps.usb_serial_jtag.value && sync.mac.is_ok() => {
            Outcome::from_comparison(
                caps.chip,
                "usb_serial_jtag",
                SafeText::from_host("yes"),
                sync.duration_ms,
            )
        }
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
            return Outcome::fail(&error.to_string(), sync.duration_ms, expected, None);
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

/// Source of time for test durations. The runner never reads the system clock directly, so
/// tests can prove where each duration comes from.
pub trait Clock {
    /// Milliseconds since a fixed, arbitrary origin; never decreases.
    fn now_ms(&self) -> u64;
}

/// The real clock: monotonic milliseconds since the clock was created ([`Instant`]).
#[derive(Debug, Clone, Copy)]
pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    /// A clock whose origin is now.
    pub fn new() -> SystemClock {
        SystemClock {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock::new()
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// Pauses the runner while it waits for a port to come back. Separate from [`Clock`] so tests
/// can advance a fake clock instead of sleeping.
pub trait Sleeper {
    /// Blocks the calling thread (never the UI thread) for about `duration`.
    fn sleep(&self, duration: Duration);
}

impl Sleeper for SystemClock {
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// `rom/chip-identity`: the ROM's chip identity equals the table's (spec §4).
pub const CHIP_IDENTITY: TestId = TestId::new(Suite::Rom, "chip-identity");

/// `rom/revision`: the silicon revision is readable.
pub const REVISION: TestId = TestId::new(Suite::Rom, "revision");

/// `rom/crystal`: crystal frequency detected by the ROM, compared with the table.
pub const CRYSTAL: TestId = TestId::new(Suite::Rom, "crystal");

/// `rom/mac-efuse-reg`: `READ_REG` of the eFuse MAC words decodes the ROM's MAC (H5).
pub const MAC_EFUSE_REG: TestId = TestId::new(Suite::Rom, "mac-efuse-reg");

/// `efuse/package`: package version in eFuse.
pub const PACKAGE: TestId = TestId::new(Suite::Efuse, "package");

/// `efuse/psram`: PSRAM inside the package, as declared in eFuse.
pub const PSRAM: TestId = TestId::new(Suite::Efuse, "psram");

/// `efuse/embedded-flash`: flash inside the package, as declared in eFuse.
pub const EMBEDDED_FLASH: TestId = TestId::new(Suite::Efuse, "embedded-flash");

/// `flash/jedec-id`: the SPI flash answers RDID (`0x9F`).
pub const JEDEC_ID: TestId = TestId::new(Suite::Flash, "jedec-id");

/// `flash/size`: flash size from the capacity byte of the JEDEC ID.
pub const FLASH_SIZE: TestId = TestId::new(Suite::Flash, "size");

/// Tests run by [`run_suites`] through the primary port, in catalog order (spec §4).
pub const BOARD_TESTS: [TestId; 9] = [
    CHIP_IDENTITY,
    REVISION,
    CRYSTAL,
    MAC_EFUSE_REG,
    PACKAGE,
    PSRAM,
    EMBEDDED_FLASH,
    JEDEC_ID,
    FLASH_SIZE,
];

/// FAIL reason of `flash/jedec-id` when nothing drove the bus (spec §4).
pub const FAIL_BLANK_FLASH: &str = "no flash answered RDID (0x000000/0xFFFFFF)";

/// FAIL reason of `rom/mac-efuse-reg` when the eFuse words decode another MAC (H5).
pub const FAIL_MAC_EFUSE_MISMATCH: &str =
    "MAC decoded from the eFuse words differs from the ROM MAC";

/// The v0.1 test plan of one board, in catalog order (spec §4, D7): `port/uart-bridge` only
/// when the board has a bridge port, `port/usb-serial-jtag` always, then [`BOARD_TESTS`].
///
/// `caps` is the chip row (`None` when no port identified the chip). No v0.1 test depends on it
/// (a test that does not apply is a SKIP, never left out), but the catalog is per chip from
/// v0.2 on.
pub fn catalog(caps: Option<&ChipCaps>, has_bridge: bool) -> Vec<TestId> {
    // Every v0.1 chip has the same catalog; see the doc comment.
    let _ = caps;
    let mut plan = Vec::with_capacity(BOARD_TESTS.len() + 2);
    if has_bridge {
        plan.push(UART_BRIDGE);
    }
    plan.push(USB_SERIAL_JTAG);
    plan.extend(BOARD_TESTS);
    plan
}

/// The session with the board died during `test` (timeout or disconnection): the run was
/// aborted (ADR-010 §3) and the remaining tests are `not run`. The caller still owes the
/// board a [`Probe::hard_reset`] attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interrupted {
    /// The test in flight when the session died; it is a `fail` in the run.
    pub test: TestId,
    /// What the probe reported.
    pub error: ProbeError,
}

impl fmt::Display for Interrupted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "session interrupted during {}: {}",
            self.test, self.error
        )
    }
}

impl std::error::Error for Interrupted {}

/// Runs the `rom`, `efuse` and `flash` suites ([`BOARD_TESTS`]) on the session already opened
/// by [`sync_port`] on the primary port, recording each outcome in `run`.
///
/// Per test, a probe error becomes:
/// - [`ProbeError::Unsupported`]: SKIP with the probe's reason;
/// - [`ProbeError::Protocol`] / [`ProbeError::Busy`]: FAIL of that test only; the next runs;
/// - [`ProbeError::Timeout`] / [`ProbeError::Disconnected`] / [`ProbeError::NotConnected`]:
///   the session is gone, so [`Run::abort`] and `Err(Interrupted)`.
///
/// `flash/size` depends on `flash/jedec-id`: if that did not pass, it is a SKIP
/// `not run: flash/jedec-id failed`. A test `run` refuses (not in the plan, already finished)
/// is a runner bug: it trips a `debug_assert!` and is left alone in release builds.
/// The hard reset is not done here: the caller does it in every case.
///
/// `observer` hears [`Progress::Started`] when a test begins and [`Progress::Finished`] when
/// its outcome is recorded. Tests closed by [`Run::abort`] are not announced here: the caller
/// reads them from `run` (see [`run_board`]).
pub fn run_suites(
    probe: &mut dyn Probe,
    caps: &ChipCaps,
    clock: &dyn Clock,
    run: &mut Run,
    observer: &mut dyn FnMut(Progress),
) -> Result<(), Interrupted> {
    let mut session = Session {
        probe,
        clock,
        run,
        observer,
    };
    let chip = caps.chip;

    if let Some((identity, ms)) = session.read(CHIP_IDENTITY, "chip identity read", |probe| {
        probe.identity()
    })? {
        let read = SafeText::from_host(&identity.to_string());
        session.record(
            CHIP_IDENTITY,
            Outcome::from_comparison(chip, "identity", read, ms),
        );
    }

    if let Some((revision, ms)) =
        session.read(REVISION, "revision read", |probe| probe.revision())?
    {
        let read = SafeText::from_host(&revision.to_string());
        session.record(REVISION, Outcome::pass(ms, None, Some(read)));
    }

    if let Some((mhz, ms)) = session.read(CRYSTAL, "crystal read", |probe| probe.crystal_mhz())? {
        let read = SafeText::from_host(&mhz_text(mhz));
        session.record(
            CRYSTAL,
            Outcome::from_comparison(chip, "crystal_mhz", read, ms),
        );
    }

    let reg = caps.mac_efuse_reg.value;
    if let Some(((rom, efuse), ms)) = session.read(MAC_EFUSE_REG, "eFuse MAC read", |probe| {
        efuse_mac(probe, reg)
    })? {
        let expected = Some(SafeText::from_host(&rom.to_string()));
        let read = Some(SafeText::from_host(&efuse.to_string()));
        let outcome = if rom == efuse {
            Outcome::pass(ms, expected, read)
        } else {
            Outcome::fail(FAIL_MAC_EFUSE_MISMATCH, ms, expected, read)
        };
        session.record(MAC_EFUSE_REG, outcome);
    }

    if let Some((package, ms)) = session.read(PACKAGE, "package version read", |probe| {
        probe.package_version()
    })? {
        let read = SafeText::from_host(&format!("package {package}"));
        session.record(PACKAGE, Outcome::pass(ms, None, Some(read)));
    }

    if let Some((psram, ms)) =
        session.read(PSRAM, "PSRAM eFuse read", |probe| probe.embedded_psram())?
    {
        let read = match psram {
            None => SafeText::from_host("none in package"),
            Some(mem) => {
                SafeText::from_board(&format!("{} in package{}", size_of(&mem), vendor_of(&mem)))
            }
        };
        session.record(PSRAM, Outcome::pass(ms, None, Some(read)));
    }

    if let Some((flash, ms)) =
        session.read(EMBEDDED_FLASH, "embedded flash eFuse read", |probe| {
            probe.embedded_flash()
        })?
    {
        let read = match flash {
            None => SafeText::from_host("none"),
            Some(mem) => SafeText::from_board(&format!("{}{}", size_of(&mem), vendor_of(&mem))),
        };
        session.record(EMBEDDED_FLASH, Outcome::pass(ms, None, Some(read)));
    }

    let mut flash_id = None;
    if let Some((id, ms)) = session.read(JEDEC_ID, "flash ID read", |probe| probe.flash_id())? {
        let read = Some(SafeText::from_host(&id.to_string()));
        if id.is_blank() {
            session.record(JEDEC_ID, Outcome::fail(FAIL_BLANK_FLASH, ms, None, read));
        } else {
            session.record(JEDEC_ID, Outcome::pass(ms, None, read));
            flash_id = Some(id);
        }
    }

    let Some(id) = flash_id else {
        // "failed" is the spec's wording (§3.3); a jedec-id that was itself a SKIP says so.
        let jedec_skipped = session
            .run
            .entries()
            .iter()
            .find(|entry| entry.id() == JEDEC_ID)
            .and_then(|entry| entry.outcome())
            .is_some_and(|outcome| outcome.state() == State::Skip);
        let verb = if jedec_skipped { "skipped" } else { "failed" };
        let reason = format!("not run: {JEDEC_ID} {verb}");
        session.record(FLASH_SIZE, Outcome::skip(&reason));
        return Ok(());
    };
    if session.begin(FLASH_SIZE) {
        let start = clock.now_ms();
        let size = id.size_bytes();
        let ms = clock.now_ms().saturating_sub(start);
        let outcome = match size {
            Some(bytes) => Outcome::pass(ms, None, Some(SafeText::from_host(&size_text(bytes)))),
            None => Outcome::fail(
                &format!("capacity byte {:#04X} is not a flash size", id.capacity),
                ms,
                None,
                Some(SafeText::from_host(&id.to_string())),
            ),
        };
        session.record(FLASH_SIZE, outcome);
    }
    Ok(())
}

/// The ROM's MAC and the MAC decoded from the two eFuse words at `reg` (H5).
fn efuse_mac(probe: &mut dyn Probe, reg: u32) -> Result<(Mac, Mac), ProbeError> {
    let rom = probe.mac()?;
    let hi_reg = reg.checked_add(4).ok_or_else(|| {
        ProbeError::Unsupported(format!("eFuse MAC register {reg:#010X} has no next word"))
    })?;
    let lo = probe.read_reg(reg)?;
    let hi = probe.read_reg(hi_reg)?;
    Ok((rom, Mac::from_efuse_words(lo, hi)))
}

/// `"8 MB"`, or `"unknown size"` when the eFuse capacity code is not in the table.
fn size_of(mem: &EmbeddedMem) -> String {
    mem.size_mb
        .map_or_else(|| "unknown size".to_owned(), |mb| format!("{mb} MB"))
}

/// `" (AP_3v3)"`, or nothing when the vendor code has no name.
fn vendor_of(mem: &EmbeddedMem) -> String {
    mem.vendor
        .as_deref()
        .map_or_else(String::new, |vendor| format!(" ({vendor})"))
}

/// Flash size in the notation of spec §4: `<n> MB` with MB = 2^20 bytes, as esptool. Sizes
/// that are not whole MB (capacity bytes below `0x14`) fall back to KB, then bytes.
fn size_text(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    if bytes >= MB && bytes.is_multiple_of(MB) {
        format!("{} MB", bytes / MB)
    } else if bytes >= KB && bytes.is_multiple_of(KB) {
        format!("{} KB", bytes / KB)
    } else {
        format!("{bytes} bytes")
    }
}

/// One run of the board suites: the probe, the clock, the run being filled and who listens.
struct Session<'a> {
    probe: &'a mut dyn Probe,
    clock: &'a dyn Clock,
    run: &'a mut Run,
    observer: &'a mut dyn FnMut(Progress),
}

impl Session<'_> {
    /// Starts test `id`, times `op` and maps its error (see [`run_suites`]).
    ///
    /// `Ok(Some((value, ms)))`: the caller records the outcome. `Ok(None)`: the outcome is
    /// already recorded (SKIP or FAIL) or `run` refused the test. `what` names the read in the
    /// failure reason (`"port disconnected during the flash ID read"`, spec §8.3).
    fn read<T>(
        &mut self,
        id: TestId,
        what: &str,
        op: impl FnOnce(&mut dyn Probe) -> Result<T, ProbeError>,
    ) -> Result<Option<(T, u64)>, Interrupted> {
        if !self.begin(id) {
            return Ok(None);
        }
        let start = self.clock.now_ms();
        let result = op(&mut *self.probe);
        let ms = self.clock.now_ms().saturating_sub(start);
        match result {
            Ok(value) => Ok(Some((value, ms))),
            Err(ProbeError::Unsupported(detail)) => {
                let reason = if detail.trim().is_empty() {
                    "not supported on this chip".to_owned()
                } else {
                    detail
                };
                self.record(id, Outcome::skip(&reason));
                Ok(None)
            }
            Err(error @ (ProbeError::Protocol(_) | ProbeError::Busy)) => {
                let reason = format!("{error} during the {what}");
                self.record(id, Outcome::fail(&reason, ms, None, None));
                Ok(None)
            }
            Err(
                error @ (ProbeError::Timeout | ProbeError::Disconnected | ProbeError::NotConnected),
            ) => {
                let reason = format!("{error} during the {what}");
                let aborted = self.run.abort(&reason, ms);
                debug_assert!(aborted.is_ok(), "runner bug: {aborted:?}");
                Err(Interrupted { test: id, error })
            }
        }
    }

    /// Marks `id` in flight; false (and a debug assertion) if `run` refuses it.
    fn begin(&mut self, id: TestId) -> bool {
        let started = self.run.start(id);
        debug_assert!(started.is_ok(), "runner bug: {started:?}");
        if started.is_ok() {
            (self.observer)(Progress::Started(id));
        }
        started.is_ok()
    }

    /// Records the outcome of `id`; a refusal is a runner bug (debug assertion).
    fn record(&mut self, id: TestId, outcome: Outcome) {
        let finished = self.run.finish(id, outcome.clone());
        debug_assert!(finished.is_ok(), "runner bug: {finished:?}");
        if finished.is_ok() {
            (self.observer)(Progress::Finished(TestResult { id, outcome }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chips::Chip;
    use crate::probe::fake::{FakeProbe, Step};
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
        let tests = port_tests(Some(caps_of("esp32")), mac_of(&bridge), &[bridge]);
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
        let tests = port_tests(Some(caps_of("esp32s3")), mac_of(&bridge), &[bridge]);
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
        let tests = port_tests(Some(caps_of("esp32c6")), mac_of(&sync), &[sync]);
        assert_eq!(ids(&tests), ["port/usb-serial-jtag"]);
        assert_eq!(tests[0].outcome.state(), State::Pass);
    }

    #[test]
    fn port_suite_passes_synced_ports() {
        // C6 with both cables: bridge first (primary), then the native port.
        let bridge = sync_with("esp32c6", &ch343(), None);
        let native = sync_with("esp32c6", &native("E8:F6:0A:F7:FF:D4"), None);
        let board = mac_of(&bridge);
        let tests = port_tests(Some(caps_of("esp32c6")), board, &[bridge, native]);
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
        assert_eq!(bridge.mac, Err(SyncError::Probe(ProbeError::Timeout)));
        let tests = port_tests(Some(caps_of("esp32s3")), None, &[bridge]);
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
        let tests = port_tests(Some(caps_of("esp32c6")), board, &[bridge, other]);
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
        let tests = port_tests(Some(caps_of("esp32c6")), board, &[wrong]);
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
        let tests = port_tests(Some(caps_of("esp32")), board, &[bridge, odd]);
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

    // ---- board suites (rom, efuse, flash) ----

    use std::cell::Cell;

    use crate::chips::ChipIdentity;
    use crate::probe::FlashId;
    use crate::report::{BoardInfo, HardReset, Report};

    const FIXTURES: [&str; 4] = ["esp32", "esp32c3", "esp32c6", "esp32s3"];

    /// Advances `step` ms on every read, so each timed probe call lasts exactly `step` ms.
    struct FakeClock {
        now: Cell<u64>,
        step: u64,
    }

    impl FakeClock {
        fn new(step: u64) -> FakeClock {
            FakeClock {
                now: Cell::new(1_000),
                step,
            }
        }
    }

    impl Clock for FakeClock {
        fn now_ms(&self) -> u64 {
            let now = self.now.get();
            self.now.set(now + self.step);
            now
        }
    }

    fn empty_board() -> BoardInfo {
        BoardInfo {
            chip: None,
            mac: None,
            ports: Vec::new(),
        }
    }

    /// What the bench example does, through one bridge port: catalog, sync, port suite, board
    /// suites, hard reset, complete.
    fn run_one_port(
        mut probe: FakeProbe,
        fixture: &str,
        clock: &dyn Clock,
    ) -> (Result<(), Interrupted>, Report, FakeProbe) {
        let caps = caps_of(fixture);
        let mut run = Run::new("0.1.0", empty_board(), catalog(Some(caps), true));
        let sync = sync_port(&mut probe, &ch343());
        for test in port_tests(Some(caps), mac_of(&sync), &[sync]) {
            run.finish(test.id, test.outcome).unwrap();
        }
        let result = run_suites(&mut probe, caps, clock, &mut run, &mut |_| {});
        let reset = match probe.hard_reset() {
            Ok(()) => HardReset::done(),
            Err(error) => HardReset::not_done(&error.to_string()),
        };
        (result, run.complete(reset), probe)
    }

    fn fake(fixture: &str) -> FakeProbe {
        FakeProbe::from_fixture(fixture).unwrap()
    }

    fn outcome(report: &Report, id: TestId) -> &Outcome {
        &report
            .tests()
            .iter()
            .find(|test| test.id == id)
            .unwrap_or_else(|| panic!("{id} missing from the report"))
            .outcome
    }

    fn text(value: Option<&SafeText>) -> Option<&str> {
        value.map(SafeText::as_str)
    }

    #[test]
    fn rom_suite_passes_on_every_bench_fixture() {
        // (fixture, identity, revision, package, psram, embedded flash, JEDEC ID, size)
        let expected = [
            (
                "esp32",
                "magic register 0x40001000 = 0x00F01D83",
                "v3.1",
                "package 1",
                "none in package",
                "none",
                "0x5E4016",
                "4 MB",
            ),
            (
                "esp32c3",
                "security info chip id 5",
                "v0.4",
                "package 0",
                "none in package",
                "4 MB (XMC)",
                "0x464016",
                "4 MB",
            ),
            (
                "esp32c6",
                "security info chip id 13",
                "v0.2",
                "package 0",
                "none in package",
                "none",
                "0x684018",
                "16 MB",
            ),
            (
                "esp32s3",
                "security info chip id 9",
                "v0.2",
                "package 0",
                "8 MB in package (AP_3v3)",
                "none",
                "0x684018",
                "16 MB",
            ),
        ];
        for (name, identity, revision, package, psram, embedded, jedec, size) in expected {
            let (result, report, probe) = run_one_port(fake(name), name, &FakeClock::new(1));
            assert_eq!(result, Ok(()), "{name}");
            for id in BOARD_TESTS {
                let outcome = outcome(&report, id);
                assert_eq!(outcome.state(), State::Pass, "{name} {id}: {outcome:?}");
                assert!(outcome.finding().is_none(), "{name} {id}");
            }
            let read = |id| text(outcome(&report, id).read());
            let expected_of = |id| text(outcome(&report, id).expected());
            assert_eq!(read(CHIP_IDENTITY), Some(identity), "{name}");
            assert_eq!(expected_of(CHIP_IDENTITY), Some(identity), "{name}");
            assert_eq!(read(REVISION), Some(revision), "{name}");
            assert_eq!(expected_of(REVISION), None, "{name}");
            assert_eq!(read(CRYSTAL), Some("40 MHz"), "{name}");
            assert_eq!(expected_of(CRYSTAL), Some("40 MHz"), "{name}");
            assert_eq!(read(MAC_EFUSE_REG), expected_of(MAC_EFUSE_REG), "{name}");
            assert_eq!(read(PACKAGE), Some(package), "{name}");
            assert_eq!(read(PSRAM), Some(psram), "{name}");
            assert_eq!(read(EMBEDDED_FLASH), Some(embedded), "{name}");
            assert_eq!(read(JEDEC_ID), Some(jedec), "{name}");
            assert_eq!(read(FLASH_SIZE), Some(size), "{name}");
            assert_eq!(report.exit_code(), 0, "{name}");
            assert_eq!(probe.calls().last(), Some(&Step::HardReset), "{name}");
        }
    }

    #[test]
    fn mac_efuse_mismatch_fails_with_expected_and_read() {
        let rom = Mac([0x02, 0, 0, 0, 0, 1]);
        let (result, report, _) =
            run_one_port(fake("esp32c6").with_mac(rom), "esp32c6", &FakeClock::new(1));
        assert_eq!(result, Ok(()));
        let outcome = outcome(&report, MAC_EFUSE_REG);
        assert_eq!(outcome.state(), State::Fail);
        assert_eq!(text(outcome.reason()), Some(FAIL_MAC_EFUSE_MISMATCH));
        assert_eq!(text(outcome.expected()), Some("02:00:00:00:00:01"));
        assert_eq!(text(outcome.read()), Some("e8:f6:0a:f7:ff:d4"));
        assert_eq!(report.exit_code(), 1);
    }

    #[test]
    fn missing_efuse_register_is_skip_with_reason() {
        let probe = fake("esp32s3").fail_at(
            Step::ReadReg,
            ProbeError::Unsupported("eFuse MAC register not readable on this chip".to_owned()),
        );
        let (result, report, _) = run_one_port(probe, "esp32s3", &FakeClock::new(1));
        assert_eq!(result, Ok(()));
        let outcome = outcome(&report, MAC_EFUSE_REG);
        assert_eq!(outcome.state(), State::Skip);
        assert_eq!(
            text(outcome.reason()),
            Some("eFuse MAC register not readable on this chip")
        );
        assert_eq!(outcome.duration_ms(), 0);
        // The following tests still run.
        assert_eq!(outcome_state(&report, PACKAGE), State::Pass);
        assert_eq!(outcome_state(&report, FLASH_SIZE), State::Pass);
        assert_eq!(report.exit_code(), 0);
    }

    fn outcome_state(report: &Report, id: TestId) -> State {
        outcome(report, id).state()
    }

    #[test]
    fn identity_mismatch_is_invariant_fail_with_finding() {
        let probe = fake("esp32c6").with_identity(ChipIdentity::SecurityChipId(9));
        let (result, report, _) = run_one_port(probe, "esp32c6", &FakeClock::new(1));
        assert_eq!(result, Ok(()));
        let outcome = outcome(&report, CHIP_IDENTITY);
        assert_eq!(outcome.state(), State::Fail);
        assert_eq!(text(outcome.expected()), Some("security info chip id 13"));
        assert_eq!(text(outcome.read()), Some("security info chip id 9"));
        let finding = outcome.finding().unwrap();
        assert_eq!(finding.field(), "identity");
        assert_eq!(finding.kind(), crate::chips::FieldKind::Invariant);
        assert_eq!(finding.table().as_str(), "security info chip id 13");
        assert_eq!(report.exit_code(), 1);
    }

    #[test]
    fn crystal_mismatch_is_variable_pass_with_finding() {
        let probe = fake("esp32").with_crystal_mhz(26);
        let (result, report, _) = run_one_port(probe, "esp32", &FakeClock::new(1));
        assert_eq!(result, Ok(()));
        let outcome = outcome(&report, CRYSTAL);
        assert_eq!(outcome.state(), State::Pass);
        assert_eq!(outcome.reason(), None);
        assert_eq!(text(outcome.expected()), Some("40 MHz"));
        assert_eq!(text(outcome.read()), Some("26 MHz"));
        let finding = outcome.finding().unwrap();
        assert_eq!(finding.field(), "crystal_mhz");
        assert_eq!(finding.kind(), crate::chips::FieldKind::Variable);
        assert_eq!(finding.table().as_str(), "40 MHz");
        assert_eq!(finding.read().as_str(), "26 MHz");
        assert_eq!(
            finding.source().as_str(),
            "ESP32 Series Datasheet v5.3, Features; ESP32 Series Comparison"
        );
        assert_eq!(report.exit_code(), 0);
    }

    #[test]
    fn blank_flash_fails_jedec_and_skips_size_as_not_run() {
        for blank in [0x00, 0xFF] {
            let id = FlashId {
                manufacturer: blank,
                memory_type: blank,
                capacity: blank,
            };
            let probe = fake("esp32c3").with_flash_id(id);
            let (result, report, _) = run_one_port(probe, "esp32c3", &FakeClock::new(1));
            assert_eq!(result, Ok(()));
            let jedec = outcome(&report, JEDEC_ID);
            assert_eq!(jedec.state(), State::Fail);
            assert_eq!(
                text(jedec.reason()),
                Some("no flash answered RDID (0x000000/0xFFFFFF)")
            );
            assert_eq!(text(jedec.read()), Some(id.to_string().as_str()));
            let size = outcome(&report, FLASH_SIZE);
            assert_eq!(size.state(), State::Skip);
            assert_eq!(text(size.reason()), Some("not run: flash/jedec-id failed"));
            assert_eq!(report.exit_code(), 1);
            assert!(report.hard_reset().is_done());
        }
    }

    #[test]
    fn disconnect_mid_suite_aborts_and_rest_is_not_run() {
        // Spec §8.3: cable pulled during the flash ID read.
        let probe = fake("esp32s3")
            .fail_at(Step::FlashId, ProbeError::Disconnected)
            .fail_at(Step::HardReset, ProbeError::Disconnected);
        let (result, report, probe) = run_one_port(probe, "esp32s3", &FakeClock::new(7));
        assert_eq!(
            result,
            Err(Interrupted {
                test: JEDEC_ID,
                error: ProbeError::Disconnected
            })
        );
        let jedec = outcome(&report, JEDEC_ID);
        assert_eq!(jedec.state(), State::Fail);
        assert_eq!(
            text(jedec.reason()),
            Some("port disconnected during the flash ID read")
        );
        assert_eq!(jedec.duration_ms(), 7);
        assert_eq!(jedec.read(), None);
        let size = outcome(&report, FLASH_SIZE);
        assert_eq!(text(size.reason()), Some("not run: flash/jedec-id failed"));
        assert_eq!(
            report.summary().to_string(),
            "Tests: 1 failed, 8 passed, 2 skipped, 11 total"
        );
        assert_eq!(
            text(report.hard_reset().reason()),
            Some("port disconnected")
        );
        assert_eq!(probe.calls().last(), Some(&Step::HardReset));

        // A timeout early in `rom`: every later test is in the report, pointing at the cause.
        let probe = fake("esp32c6").fail_at(Step::Revision, ProbeError::Timeout);
        let (result, report, _) = run_one_port(probe, "esp32c6", &FakeClock::new(1));
        assert_eq!(result.unwrap_err().test, REVISION);
        let ids: Vec<TestId> = report.tests().iter().map(|test| test.id).collect();
        assert_eq!(ids, catalog(Some(caps_of("esp32c6")), true));
        assert_eq!(
            text(outcome(&report, REVISION).reason()),
            Some("timeout during the revision read")
        );
        for id in &BOARD_TESTS[2..] {
            let outcome = outcome(&report, *id);
            assert_eq!(outcome.state(), State::Skip, "{id}");
            assert_eq!(
                text(outcome.reason()),
                Some("not run: rom/revision failed"),
                "{id}"
            );
        }
        // Passed: uart-bridge, chip-identity. Skipped: usb-serial-jtag (native port not
        // connected) and the 7 tests after revision.
        let summary = report.summary();
        assert_eq!(
            (
                summary.passed,
                summary.failed,
                summary.skipped,
                summary.total
            ),
            (2, 1, 8, 11)
        );
        assert!(report.hard_reset().is_done());
    }

    #[test]
    fn protocol_error_fails_only_that_test() {
        let probe = fake("esp32c6").fail_at(
            Step::PackageVersion,
            ProbeError::Protocol("unexpected response 0x01".to_owned()),
        );
        let (result, report, _) = run_one_port(probe, "esp32c6", &FakeClock::new(1));
        assert_eq!(result, Ok(()));
        let package = outcome(&report, PACKAGE);
        assert_eq!(package.state(), State::Fail);
        assert_eq!(
            text(package.reason()),
            Some("protocol error: unexpected response 0x01 during the package version read")
        );
        for id in [PSRAM, EMBEDDED_FLASH, JEDEC_ID, FLASH_SIZE] {
            assert_eq!(outcome_state(&report, id), State::Pass, "{id}");
        }
    }

    #[test]
    fn catalog_order_matches_spec() {
        let names =
            |plan: Vec<TestId>| -> Vec<String> { plan.iter().map(ToString::to_string).collect() };
        let board = [
            "rom/chip-identity",
            "rom/revision",
            "rom/crystal",
            "rom/mac-efuse-reg",
            "efuse/package",
            "efuse/psram",
            "efuse/embedded-flash",
            "flash/jedec-id",
            "flash/size",
        ];
        for fixture in FIXTURES {
            let caps = caps_of(fixture);
            let mut with_bridge = vec!["port/uart-bridge", "port/usb-serial-jtag"];
            with_bridge.extend(board);
            assert_eq!(names(catalog(Some(caps), true)), with_bridge, "{fixture}");
            let mut native_only = vec!["port/usb-serial-jtag"];
            native_only.extend(board);
            assert_eq!(names(catalog(Some(caps), false)), native_only, "{fixture}");
        }
    }

    #[test]
    fn durations_come_from_the_clock() {
        let (_, report, _) = run_one_port(fake("esp32c6"), "esp32c6", &FakeClock::new(5));
        for id in BOARD_TESTS {
            assert_eq!(outcome(&report, id).duration_ms(), 5, "{id}");
        }

        // A SKIP takes 0 ms whatever the clock says.
        let probe = fake("esp32c6").fail_at(
            Step::EmbeddedPsram,
            ProbeError::Unsupported("no PSRAM field".to_owned()),
        );
        let (_, report, _) = run_one_port(probe, "esp32c6", &FakeClock::new(5));
        assert_eq!(outcome(&report, PSRAM).duration_ms(), 0);
    }

    #[test]
    fn flash_size_text_uses_mb_as_esptool() {
        assert_eq!(size_text(16 * 1024 * 1024), "16 MB");
        assert_eq!(size_text(512 * 1024), "512 KB");
        assert_eq!(size_text(1), "1 bytes");
    }
}
