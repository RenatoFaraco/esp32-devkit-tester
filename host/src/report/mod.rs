//! Pure, serializable result model: PASS/FAIL/SKIP per suite, durations and the
//! `Tests: X failed, Y passed, Z skipped, N total` summary.
//!
//! No I/O and no terminal: testable on its own. It must not import `transport`, `probe`,
//! `ui`, `ratatui` or `crossterm`. The only crate module it uses is `chips` (pure data).
//!
//! The model mirrors `docs/context/spec/report-json.md` (schema v1); [`json`] serializes it.
//! The TUI, the `--no-tui` text and the `--json` document are three renderings of the same
//! [`Report`] (ADR-010).
//!
//! # Safe text
//! Every string that ends up on screen or in the JSON is a [`SafeText`], which can only be
//! built by sanitizing (spec §5.1): printable ASCII `0x20..=0x7E`, everything else becomes `?`,
//! and the length is capped (128 characters for board/USB-descriptor text, 256 for host text).
//! An `ESC` from the board therefore cannot reach the model, because the type does not allow it.
//!
//! # Live run vs. final report
//! [`Run`] is created with the whole test plan, every test pending; the runner records
//! [`Run::start`], [`Run::finish`] and, when the session dies, [`Run::abort`].
//! [`Run::complete`] turns it into a [`Report`] in which no planned test is missing: whatever is
//! still pending becomes a `skip` that points at the failure that interrupted the session.
//!
//! # Reasons and panics
//! `fail` and `skip` require a non-blank reason (ADR-010 §1). Reasons are host-authored
//! English text, so a blank one is a programming error: it trips a `debug_assert!` in debug
//! builds (and therefore in `cargo test`) and is normalized to [`NO_REASON_GIVEN`] in release
//! builds. A `Result` was rejected because every runner call site would have to handle an
//! error that can only come from a literal; nothing here panics in a release build.

pub mod json;

use std::fmt;

use crate::chips::{Chip, FieldKind};

/// Maximum length, in characters, of text that came from the board or a USB descriptor.
pub const BOARD_TEXT_MAX: usize = 128;

/// Maximum length, in characters, of any other (host-authored) text.
pub const HOST_TEXT_MAX: usize = 256;

/// Marker that ends a truncated [`SafeText`]; it counts towards the length limit.
pub const ELLIPSIS: &str = "...";

/// Replacement for every character outside printable ASCII.
pub const REPLACEMENT: char = '?';

/// Reason stored when a `fail`/`skip` is built with a blank reason (release builds only).
pub const NO_REASON_GIVEN: &str = "no reason given";

/// Reason of a test left pending when the run was never interrupted (a runner bug).
pub const NOT_RUN_INTERNAL_ERROR: &str = "not run: internal error";

/// Value of `tool.name` in the JSON document.
pub const TOOL_NAME: &str = "devkit-tester";

/// Text that is safe to print to a terminal and to put in the JSON document.
///
/// Only printable ASCII (`0x20..=0x7E`), at most [`BOARD_TEXT_MAX`] or [`HOST_TEXT_MAX`]
/// characters. The inner `String` is private: the only way in is through sanitization.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SafeText(String);

impl SafeText {
    /// Sanitizes text that came from the board or from a USB descriptor (limit
    /// [`BOARD_TEXT_MAX`]).
    ///
    /// Raw serial bytes should be decoded with `String::from_utf8_lossy` first: invalid UTF-8
    /// becomes `U+FFFD`, which is not ASCII and therefore becomes `?` here.
    pub fn from_board(text: &str) -> Self {
        Self::sanitize(text, BOARD_TEXT_MAX)
    }

    /// Sanitizes host-authored text (limit [`HOST_TEXT_MAX`]).
    pub fn from_host(text: &str) -> Self {
        Self::sanitize(text, HOST_TEXT_MAX)
    }

    /// The sanitized text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the text is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Replaces every char outside `0x20..=0x7E` with [`REPLACEMENT`] and, if the result has
    /// more than `max` chars, keeps the first `max - 3` and appends [`ELLIPSIS`].
    ///
    /// Works per `char`, so a multi-byte UTF-8 sequence becomes a single `?` and is never cut
    /// in the middle.
    fn sanitize(text: &str, max: usize) -> Self {
        let mut chars = text.chars().map(|c| {
            if (' '..='~').contains(&c) {
                c
            } else {
                REPLACEMENT
            }
        });
        let mut out: String = chars.by_ref().take(max).collect();
        if chars.next().is_some() {
            // Every char in `out` is ASCII, so byte length == char count and any index is a
            // char boundary. `max` is one of the constants above, both larger than the marker.
            out.truncate(max.saturating_sub(ELLIPSIS.len()));
            out.push_str(ELLIPSIS);
        }
        SafeText(out)
    }
}

impl fmt::Display for SafeText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Builds the reason of a `fail`/`skip`: sanitized host text, or [`NO_REASON_GIVEN`] if blank.
fn reason_or_placeholder(reason: &str) -> SafeText {
    if reason.trim().is_empty() {
        SafeText::from_host(NO_REASON_GIVEN)
    } else {
        SafeText::from_host(reason)
    }
}

/// Checks (debug builds only) that a `fail`/`skip` reason is not blank, then builds it.
fn required_reason(reason: &str) -> SafeText {
    debug_assert!(
        !reason.trim().is_empty(),
        "fail/skip requires a non-blank reason (ADR-010 §1)"
    );
    reason_or_placeholder(reason)
}

/// A test suite. The declaration order is the order of suites in the report (spec §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Suite {
    /// Port-level checks: ROM sync and MAC through each port.
    Port,
    /// ROM bootloader queries: chip identity, revision, crystal, MAC in eFuse.
    Rom,
    /// eFuse fields: package, in-package PSRAM and flash.
    Efuse,
    /// SPI flash: JEDEC ID and size.
    Flash,
}

impl Suite {
    /// Stable lowercase name used in the report (`"port"`, `"rom"`, `"efuse"`, `"flash"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Suite::Port => "port",
            Suite::Rom => "rom",
            Suite::Efuse => "efuse",
            Suite::Flash => "flash",
        }
    }
}

impl fmt::Display for Suite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Stable identifier of a test: `suite/name` (spec §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TestId {
    /// Suite the test belongs to.
    pub suite: Suite,
    /// Test name inside the suite, kebab-case (`"jedec-id"`).
    pub name: &'static str,
}

impl TestId {
    /// Identifier of test `name` in `suite`.
    pub const fn new(suite: Suite, name: &'static str) -> Self {
        TestId { suite, name }
    }
}

impl fmt::Display for TestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.suite, self.name)
    }
}

/// Final state of a test. Closed set (ADR-010 §1): there is no fourth state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    /// The test ran and the board behaved as expected (possibly with a finding).
    Pass,
    /// The test ran and the board did not behave as expected, or the session died during it.
    Fail,
    /// The test does not apply to this board or could not run.
    Skip,
}

impl State {
    /// Stable lowercase name used in the report (`"pass"`, `"fail"`, `"skip"`).
    pub fn as_str(self) -> &'static str {
        match self {
            State::Pass => "pass",
            State::Fail => "fail",
            State::Skip => "skip",
        }
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Mismatch between the capability table and what the board reported (ADR-010 §2, spec §3.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    field: &'static str,
    kind: FieldKind,
    table: SafeText,
    read: SafeText,
    source: SafeText,
}

impl Finding {
    /// Finding for capability `field` of `chip`, with the value read from the board in the same
    /// notation as the table (`FieldView::value`). `None` if the table has no such field.
    ///
    /// `table` and `kind` come from `ChipCaps::fields()`; `source` is the row's first source,
    /// formatted `"<doc>, <section>"`.
    pub fn from_table(chip: Chip, field: &str, read: SafeText) -> Option<Self> {
        let caps = chip.caps();
        let view = caps.fields().into_iter().find(|view| view.name == field)?;
        let source = caps.sources.first().map_or_else(
            || SafeText::from_host("unknown source"),
            |source| SafeText::from_host(&format!("{}, {}", source.doc, source.section)),
        );
        Some(Finding {
            field: view.name,
            kind: view.kind,
            table: SafeText::from_host(&view.value),
            read,
            source,
        })
    }

    /// Stable field name, as in `ChipCaps::fields()` (`"crystal_mhz"`).
    pub fn field(&self) -> &'static str {
        self.field
    }

    /// Mark of the field in this chip's row; decides `fail` (invariant) vs. `pass` (variable).
    pub fn kind(&self) -> FieldKind {
        self.kind
    }

    /// Value in the table (`"40 MHz"`).
    pub fn table(&self) -> &SafeText {
        &self.table
    }

    /// Value read from the board (`"26 MHz"`).
    pub fn read(&self) -> &SafeText {
        &self.read
    }

    /// Source of the table row (`"ESP32 Series Datasheet v5.3, Features; ..."`).
    pub fn source(&self) -> &SafeText {
        &self.source
    }
}

/// Result of one test. The constructors enforce the ADR-010 rules: `pass` has no reason,
/// `fail` and `skip` always have one, `skip` takes 0 ms and reads nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    state: State,
    reason: Option<SafeText>,
    duration_ms: u64,
    expected: Option<SafeText>,
    read: Option<SafeText>,
    finding: Option<Finding>,
}

impl Outcome {
    /// A `pass`. `expected` is `None` for tests that only read a value.
    pub fn pass(duration_ms: u64, expected: Option<SafeText>, read: Option<SafeText>) -> Self {
        Outcome {
            state: State::Pass,
            reason: None,
            duration_ms,
            expected,
            read,
            finding: None,
        }
    }

    /// A `fail` with a non-blank host-authored `reason` (see the module docs for a blank one).
    /// `read` is `None` if the test failed before reading anything.
    pub fn fail(
        reason: &str,
        duration_ms: u64,
        expected: Option<SafeText>,
        read: Option<SafeText>,
    ) -> Self {
        Outcome {
            state: State::Fail,
            reason: Some(required_reason(reason)),
            duration_ms,
            expected,
            read,
            finding: None,
        }
    }

    /// A `skip` with a non-blank host-authored `reason`; it did not run, so 0 ms and no values.
    pub fn skip(reason: &str) -> Self {
        Outcome {
            state: State::Skip,
            reason: Some(required_reason(reason)),
            duration_ms: 0,
            expected: None,
            read: None,
            finding: None,
        }
    }

    /// Compares `read` with capability `field` of `chip` (ADR-010 §2, the only place that rule
    /// lives): equal → `pass`; different and the field is `Invariant` → `fail` with a finding;
    /// different and `Variable` → `pass` with a finding. `expected` is the table value.
    ///
    /// An unknown `field` is a runner bug: it trips a `debug_assert!` and is a `fail` with an
    /// internal-error reason in release builds.
    pub fn from_comparison(chip: Chip, field: &str, read: SafeText, duration_ms: u64) -> Self {
        let finding = Finding::from_table(chip, field, read.clone());
        debug_assert!(finding.is_some(), "unknown capability field {field:?}");
        let Some(finding) = finding else {
            let reason = format!("internal error: unknown capability field {field}");
            return Outcome::fail(&reason, duration_ms, None, Some(read));
        };
        let expected = Some(finding.table.clone());
        if finding.table == finding.read {
            return Outcome::pass(duration_ms, expected, Some(read));
        }
        let (state, reason) = match finding.kind {
            FieldKind::Invariant => (
                State::Fail,
                Some(SafeText::from_host(&format!(
                    "{} differs from the capability table",
                    finding.field
                ))),
            ),
            FieldKind::Variable => (State::Pass, None),
        };
        Outcome {
            state,
            reason,
            duration_ms,
            expected,
            read: Some(read),
            finding: Some(finding),
        }
    }

    /// Final state.
    pub fn state(&self) -> State {
        self.state
    }

    /// Reason: always `Some` (non-empty) for `fail`/`skip`, `None` for `pass`.
    pub fn reason(&self) -> Option<&SafeText> {
        self.reason.as_ref()
    }

    /// Duration in milliseconds, rounded down; 0 for `skip`. Measured by the runner.
    pub fn duration_ms(&self) -> u64 {
        self.duration_ms
    }

    /// Expected value when the test compares something; `None` when it only reads.
    pub fn expected(&self) -> Option<&SafeText> {
        self.expected.as_ref()
    }

    /// Value read from the board; `None` if nothing was read.
    pub fn read(&self) -> Option<&SafeText> {
        self.read.as_ref()
    }

    /// Table-vs-board mismatch, if any (at most one per test).
    pub fn finding(&self) -> Option<&Finding> {
        self.finding.as_ref()
    }
}

/// One line of the report: which test and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestResult {
    /// The test.
    pub id: TestId,
    /// How it ended.
    pub outcome: Outcome,
}

/// Kind of USB port of a board (spec §3.2). Open set: future versions may add kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PortKind {
    /// USB-UART bridge wired to UART0 (CP210x, CH340, CH343, CH9102, FTDI).
    Bridge,
    /// Native USB-Serial/JTAG (`303A:1001`).
    Native,
}

impl PortKind {
    /// Stable lowercase name used in the report (`"bridge"`, `"native"`).
    pub fn as_str(self) -> &'static str {
        match self {
            PortKind::Bridge => "bridge",
            PortKind::Native => "native",
        }
    }
}

impl fmt::Display for PortKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A USB port of the board. The port name (`COM5`, `/dev/ttyACM0`) is deliberately absent
/// (spec §7, D1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortInfo {
    /// Bridge or native.
    pub kind: PortKind,
    /// USB Vendor ID.
    pub vid: u16,
    /// USB Product ID.
    pub pid: u16,
    /// Bridge model from the host's VID:PID table (`"CH343"`); `None` on a native port.
    pub bridge_model: Option<SafeText>,
    /// USB serial number as the OS reports it (descriptor text, use [`SafeText::from_board`]).
    pub usb_serial: Option<SafeText>,
    /// Whether the `rom`, `efuse` and `flash` suites ran through this port.
    pub primary: bool,
}

impl PortInfo {
    /// VID as 4 uppercase hex digits without `0x` (`"303A"`, spec §5.3).
    pub fn vid_hex(&self) -> String {
        format!("{:04X}", self.vid)
    }

    /// PID as 4 uppercase hex digits without `0x` (`"1001"`, spec §5.3).
    pub fn pid_hex(&self) -> String {
        format!("{:04X}", self.pid)
    }
}

/// Silicon revision read from eFuse (`v0.2` = major 0, minor 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Revision {
    /// Major revision.
    pub major: u32,
    /// Minor revision.
    pub minor: u32,
}

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}.{}", self.major, self.minor)
    }
}

/// The identified chip. Target and marketing name come from the table via [`Chip::caps`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChipInfo {
    /// Chip family.
    pub chip: Chip,
    /// Description given by the ROM/eFuse (`"ESP32-S3 (QFN56)"`); board text.
    pub description: Option<SafeText>,
    /// Silicon revision; `None` if not read.
    pub revision: Option<Revision>,
}

/// The board under test (spec §3.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardInfo {
    /// Identified chip; `None` if identification failed.
    pub chip: Option<ChipInfo>,
    /// Factory base MAC in canonical form (`"e8:f6:0a:f7:ff:d4"`, spec §5.2), formatted by the
    /// runner; `None` if not read.
    pub mac: Option<SafeText>,
    /// Ports in the order they were tested (bridge first, then native).
    pub ports: Vec<PortInfo>,
}

/// Whether the session ended with the hard reset back to the application (ADR-009 §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardReset {
    reason: Option<SafeText>,
}

impl HardReset {
    /// The board was reset back to its application.
    pub fn done() -> Self {
        HardReset { reason: None }
    }

    /// The reset was not possible, with a non-blank host-authored `reason`
    /// (`"port disconnected"`); a blank one is handled as described in the module docs.
    pub fn not_done(reason: &str) -> Self {
        HardReset {
            reason: Some(required_reason(reason)),
        }
    }

    /// Whether the reset happened.
    pub fn is_done(&self) -> bool {
        self.reason.is_none()
    }

    /// Why it did not happen; `None` when it did.
    pub fn reason(&self) -> Option<&SafeText> {
        self.reason.as_ref()
    }
}

/// Misuse of a [`Run`] by the runner. Never caused by board data alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunError {
    /// The test is not in the plan.
    UnknownTest(TestId),
    /// The test already has an outcome.
    AlreadyFinished(TestId),
    /// Another test is in flight; finish or abort it first.
    AnotherInFlight {
        /// The test currently in flight.
        running: TestId,
    },
    /// [`Run::abort`] was called with no test in flight.
    NothingInFlight,
    /// The run was already aborted; no more tests may start or finish.
    Interrupted,
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::UnknownTest(id) => write!(f, "test {id} is not in the plan"),
            RunError::AlreadyFinished(id) => write!(f, "test {id} already finished"),
            RunError::AnotherInFlight { running } => write!(f, "test {running} is in flight"),
            RunError::NothingInFlight => f.write_str("no test is in flight"),
            RunError::Interrupted => f.write_str("the run was aborted"),
        }
    }
}

impl std::error::Error for RunError {}

/// A planned test and its outcome so far (`None` while pending or in flight).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunEntry {
    id: TestId,
    outcome: Option<Outcome>,
}

impl RunEntry {
    /// The test.
    pub fn id(&self) -> TestId {
        self.id
    }

    /// Its outcome, once finished.
    pub fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }
}

/// A run in progress: every planned test, in plan order, pending until finished.
///
/// The UI draws progress from [`Run::entries`] and [`Run::in_flight`]; the runner feeds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    tool_version: SafeText,
    board: BoardInfo,
    entries: Vec<RunEntry>,
    in_flight: Option<usize>,
    interrupted_by: Option<TestId>,
}

impl Run {
    /// Starts a run of `plan` (the test catalog for this board, in catalog order) on `board`.
    ///
    /// A duplicate id in `plan` is a runner bug (`suite/name` is unique, spec §3.3): it trips a
    /// `debug_assert!` and only the first occurrence is kept.
    pub fn new(tool_version: &str, board: BoardInfo, plan: Vec<TestId>) -> Self {
        let mut entries: Vec<RunEntry> = Vec::with_capacity(plan.len());
        for id in plan {
            let duplicate = entries.iter().any(|entry| entry.id == id);
            debug_assert!(!duplicate, "test {id} planned twice");
            if !duplicate {
                entries.push(RunEntry { id, outcome: None });
            }
        }
        Run {
            tool_version: SafeText::from_host(tool_version),
            board,
            entries,
            in_flight: None,
            interrupted_by: None,
        }
    }

    /// The board under test.
    pub fn board(&self) -> &BoardInfo {
        &self.board
    }

    /// Every planned test, in plan order.
    pub fn entries(&self) -> &[RunEntry] {
        &self.entries
    }

    /// The test currently running, if any.
    pub fn in_flight(&self) -> Option<TestId> {
        self.in_flight
            .and_then(|index| self.entries.get(index))
            .map(|entry| entry.id)
    }

    /// The test whose failure interrupted the session, once [`Run::abort`] was called.
    pub fn interrupted_by(&self) -> Option<TestId> {
        self.interrupted_by
    }

    /// Marks `id` as running. Starting the test that is already in flight is a no-op.
    pub fn start(&mut self, id: TestId) -> Result<(), RunError> {
        let index = self.pending_index(id)?;
        match self.in_flight {
            Some(running) if running == index => Ok(()),
            Some(running) => Err(RunError::AnotherInFlight {
                running: self.entries.get(running).map_or(id, |entry| entry.id),
            }),
            None => {
                self.in_flight = Some(index);
                Ok(())
            }
        }
    }

    /// Records the outcome of `id`. A test may finish without being started (e.g. a `skip`
    /// decided up front); if it was in flight, nothing is in flight afterwards.
    pub fn finish(&mut self, id: TestId, outcome: Outcome) -> Result<(), RunError> {
        let index = self.pending_index(id)?;
        if self.in_flight == Some(index) {
            self.in_flight = None;
        }
        if let Some(entry) = self.entries.get_mut(index) {
            entry.outcome = Some(outcome);
        }
        Ok(())
    }

    /// The session died during the test in flight (disconnection, timeout; ADR-010 §3): that
    /// test becomes a `fail` with `reason` and `duration_ms`, and it is the cause every test
    /// still pending will point to in [`Run::complete`]. Returns that test.
    pub fn abort(&mut self, reason: &str, duration_ms: u64) -> Result<TestId, RunError> {
        if self.interrupted_by.is_some() {
            return Err(RunError::Interrupted);
        }
        let entry = self
            .in_flight
            .take()
            .and_then(|index| self.entries.get_mut(index))
            .ok_or(RunError::NothingInFlight)?;
        entry.outcome = Some(Outcome::fail(reason, duration_ms, None, None));
        self.interrupted_by = Some(entry.id);
        Ok(entry.id)
    }

    /// Ends the run. Every test without an outcome becomes a `skip`:
    /// `not run: <suite>/<name> failed` pointing at the aborting test, or
    /// [`NOT_RUN_INTERNAL_ERROR`] if the run was never aborted (a runner bug, but the report
    /// still lists every planned test). Tests are ordered by suite, then plan order.
    pub fn complete(self, hard_reset: HardReset) -> Report {
        let leftover_reason = match self.interrupted_by {
            Some(cause) => format!("not run: {cause} failed"),
            None => NOT_RUN_INTERNAL_ERROR.to_owned(),
        };
        let mut tests: Vec<TestResult> = self
            .entries
            .into_iter()
            .map(|entry| TestResult {
                id: entry.id,
                outcome: entry
                    .outcome
                    .unwrap_or_else(|| Outcome::skip(&leftover_reason)),
            })
            .collect();
        // Stable sort: plan order is kept inside each suite.
        tests.sort_by_key(|test| test.id.suite);
        Report {
            tool_version: self.tool_version,
            board: self.board,
            tests,
            hard_reset,
        }
    }

    /// Index of `id` if it is planned, still without an outcome, and the run is not aborted.
    fn pending_index(&self, id: TestId) -> Result<usize, RunError> {
        if self.interrupted_by.is_some() {
            return Err(RunError::Interrupted);
        }
        let index = self
            .entries
            .iter()
            .position(|entry| entry.id == id)
            .ok_or(RunError::UnknownTest(id))?;
        match self.entries.get(index) {
            Some(entry) if entry.outcome.is_none() => Ok(index),
            _ => Err(RunError::AlreadyFinished(id)),
        }
    }
}

/// Counts of a report (spec §3.5), derived from its tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Summary {
    /// Tests in `pass` (with or without a finding).
    pub passed: usize,
    /// Tests in `fail`.
    pub failed: usize,
    /// Tests in `skip`.
    pub skipped: usize,
    /// All tests: `passed + failed + skipped`.
    pub total: usize,
}

impl fmt::Display for Summary {
    /// Jest-style line: `Tests: X failed, Y passed, Z skipped, N total`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Tests: {} failed, {} passed, {} skipped, {} total",
            self.failed, self.passed, self.skipped, self.total
        )
    }
}

/// The final, complete report of one run. Built only by [`Run::complete`], so every planned
/// test is present, ordered by suite and then by plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    tool_version: SafeText,
    board: BoardInfo,
    tests: Vec<TestResult>,
    hard_reset: HardReset,
}

impl Report {
    /// SemVer of the binary that produced the report, without `v` (`"0.1.0"`).
    pub fn tool_version(&self) -> &SafeText {
        &self.tool_version
    }

    /// The board under test.
    pub fn board(&self) -> &BoardInfo {
        &self.board
    }

    /// Every test, ordered by suite (`port`, `rom`, `efuse`, `flash`) and then by plan.
    pub fn tests(&self) -> &[TestResult] {
        &self.tests
    }

    /// Whether the board went back to its application at the end.
    pub fn hard_reset(&self) -> &HardReset {
        &self.hard_reset
    }

    /// Counts derived from [`Report::tests`], so they cannot disagree with them.
    pub fn summary(&self) -> Summary {
        let mut summary = Summary {
            total: self.tests.len(),
            ..Summary::default()
        };
        for test in &self.tests {
            match test.outcome.state() {
                State::Pass => summary.passed += 1,
                State::Fail => summary.failed += 1,
                State::Skip => summary.skipped += 1,
            }
        }
        summary
    }

    /// Process exit code for a completed run (ADR-010 §5): `1` if any test failed, else `0`.
    /// Code `2` (no run at all) never has a report, so it is not produced here.
    pub fn exit_code(&self) -> u8 {
        u8::from(self.summary().failed > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UART_BRIDGE: TestId = TestId::new(Suite::Port, "uart-bridge");
    const USB_SERIAL_JTAG: TestId = TestId::new(Suite::Port, "usb-serial-jtag");
    const CHIP_IDENTITY: TestId = TestId::new(Suite::Rom, "chip-identity");
    const REVISION: TestId = TestId::new(Suite::Rom, "revision");
    const CRYSTAL: TestId = TestId::new(Suite::Rom, "crystal");
    const MAC_EFUSE_REG: TestId = TestId::new(Suite::Rom, "mac-efuse-reg");
    const PACKAGE: TestId = TestId::new(Suite::Efuse, "package");
    const PSRAM: TestId = TestId::new(Suite::Efuse, "psram");
    const EMBEDDED_FLASH: TestId = TestId::new(Suite::Efuse, "embedded-flash");
    const JEDEC_ID: TestId = TestId::new(Suite::Flash, "jedec-id");
    const FLASH_SIZE: TestId = TestId::new(Suite::Flash, "size");

    /// v0.1 catalog with a bridge (spec §4), in catalog order.
    fn catalog() -> Vec<TestId> {
        vec![
            UART_BRIDGE,
            USB_SERIAL_JTAG,
            CHIP_IDENTITY,
            REVISION,
            CRYSTAL,
            MAC_EFUSE_REG,
            PACKAGE,
            PSRAM,
            EMBEDDED_FLASH,
            JEDEC_ID,
            FLASH_SIZE,
        ]
    }

    fn chip(target: &str) -> Chip {
        Chip::from_target(target).expect("chip in the table")
    }

    fn board() -> BoardInfo {
        BoardInfo {
            chip: None,
            mac: None,
            ports: Vec::new(),
        }
    }

    fn text(value: &str) -> Option<SafeText> {
        Some(SafeText::from_host(value))
    }

    fn pass() -> Outcome {
        Outcome::pass(1, None, text("ok"))
    }

    fn reason_of(report: &Report, id: TestId) -> Option<&str> {
        report
            .tests()
            .iter()
            .find(|test| test.id == id)
            .and_then(|test| test.outcome.reason())
            .map(SafeText::as_str)
    }

    #[test]
    fn summary_counts_each_state() {
        let mut run = Run::new("0.1.0", board(), vec![UART_BRIDGE, CRYSTAL, JEDEC_ID]);
        run.finish(UART_BRIDGE, pass()).unwrap();
        run.finish(CRYSTAL, Outcome::fail("crystal mismatch", 3, None, None))
            .unwrap();
        run.finish(JEDEC_ID, Outcome::skip("not applicable"))
            .unwrap();
        let summary = run.complete(HardReset::done()).summary();

        assert_eq!(
            summary,
            Summary {
                passed: 1,
                failed: 1,
                skipped: 1,
                total: 3
            }
        );
        assert_eq!(
            summary.to_string(),
            "Tests: 1 failed, 1 passed, 1 skipped, 3 total"
        );
    }

    #[test]
    fn exit_code_is_one_iff_any_test_failed() {
        let mut run = Run::new("0.1.0", board(), vec![UART_BRIDGE, USB_SERIAL_JTAG]);
        run.finish(UART_BRIDGE, pass()).unwrap();
        run.finish(USB_SERIAL_JTAG, Outcome::skip("no native port"))
            .unwrap();
        let report = run.complete(HardReset::done());
        assert_eq!(report.exit_code(), 0, "skip is not a failure");

        let mut run = Run::new("0.1.0", board(), vec![UART_BRIDGE, USB_SERIAL_JTAG]);
        run.finish(UART_BRIDGE, Outcome::fail("no sync", 5, None, None))
            .unwrap();
        run.finish(USB_SERIAL_JTAG, pass()).unwrap();
        let report = run.complete(HardReset::done());
        assert_eq!(report.exit_code(), 1);
        assert!(report.summary().failed > 0);
    }

    #[test]
    fn abort_marks_in_flight_fail_and_rest_skip_not_run() {
        let mut run = Run::new("0.1.0", board(), vec![PACKAGE, JEDEC_ID, FLASH_SIZE]);
        run.finish(PACKAGE, pass()).unwrap();
        run.start(JEDEC_ID).unwrap();
        assert_eq!(run.in_flight(), Some(JEDEC_ID));

        let cause = run.abort("port disconnected", 212).unwrap();
        assert_eq!(cause, JEDEC_ID);
        assert_eq!(run.in_flight(), None);
        assert_eq!(run.start(FLASH_SIZE), Err(RunError::Interrupted));

        let report = run.complete(HardReset::not_done("port disconnected"));
        let jedec = &report.tests()[1].outcome;
        assert_eq!(jedec.state(), State::Fail);
        assert_eq!(jedec.duration_ms(), 212);
        assert_eq!(reason_of(&report, JEDEC_ID), Some("port disconnected"));

        let size = &report.tests()[2].outcome;
        assert_eq!(size.state(), State::Skip);
        assert_eq!(size.duration_ms(), 0);
        assert_eq!(
            reason_of(&report, FLASH_SIZE),
            Some("not run: flash/jedec-id failed")
        );
    }

    #[test]
    fn abort_without_a_test_in_flight_is_an_error() {
        let mut run = Run::new("0.1.0", board(), vec![JEDEC_ID]);
        assert_eq!(run.abort("gone", 0), Err(RunError::NothingInFlight));
    }

    #[test]
    fn misuse_of_a_run_is_reported_not_panicked() {
        let mut run = Run::new("0.1.0", board(), vec![JEDEC_ID, FLASH_SIZE]);
        assert_eq!(run.start(CRYSTAL), Err(RunError::UnknownTest(CRYSTAL)));
        run.start(JEDEC_ID).unwrap();
        run.start(JEDEC_ID).unwrap();
        assert_eq!(
            run.start(FLASH_SIZE),
            Err(RunError::AnotherInFlight { running: JEDEC_ID })
        );
        run.finish(JEDEC_ID, pass()).unwrap();
        assert_eq!(
            run.finish(JEDEC_ID, pass()),
            Err(RunError::AlreadyFinished(JEDEC_ID))
        );
    }

    #[test]
    fn no_planned_test_is_omitted() {
        let mut run = Run::new("0.1.0", board(), catalog());
        run.finish(UART_BRIDGE, pass()).unwrap();
        run.start(CHIP_IDENTITY).unwrap();
        run.abort("timeout", 3000).unwrap();
        let report = run.complete(HardReset::done());

        let mut ids: Vec<TestId> = report.tests().iter().map(|test| test.id).collect();
        assert_eq!(ids, catalog());
        ids.dedup();
        assert_eq!(ids.len(), catalog().len());
        assert_eq!(report.summary().total, catalog().len());
    }

    #[test]
    fn pending_without_abort_becomes_internal_error_skip() {
        let mut run = Run::new("0.1.0", board(), vec![UART_BRIDGE, CRYSTAL]);
        run.finish(UART_BRIDGE, pass()).unwrap();
        run.start(CRYSTAL).unwrap();
        let report = run.complete(HardReset::done());

        assert_eq!(report.tests()[1].outcome.state(), State::Skip);
        assert_eq!(reason_of(&report, CRYSTAL), Some(NOT_RUN_INTERNAL_ERROR));
    }

    #[test]
    fn tests_are_ordered_by_suite_then_plan() {
        let plan = vec![
            FLASH_SIZE,
            CRYSTAL,
            JEDEC_ID,
            UART_BRIDGE,
            REVISION,
            PACKAGE,
        ];
        let mut run = Run::new("0.1.0", board(), plan.clone());
        // Finish in yet another order: the report order must not depend on it.
        for id in plan.iter().rev() {
            run.finish(*id, pass()).unwrap();
        }
        let report = run.complete(HardReset::done());

        let ids: Vec<TestId> = report.tests().iter().map(|test| test.id).collect();
        assert_eq!(
            ids,
            vec![
                UART_BRIDGE,
                CRYSTAL,
                REVISION,
                PACKAGE,
                FLASH_SIZE,
                JEDEC_ID
            ]
        );
    }

    #[test]
    fn duplicate_planned_test_is_kept_once() {
        // Runs only in release builds: in debug the duplicate trips a `debug_assert!`.
        if cfg!(debug_assertions) {
            return;
        }
        let run = Run::new("0.1.0", board(), vec![CRYSTAL, CRYSTAL]);
        assert_eq!(run.entries().len(), 1);
    }

    #[test]
    fn matching_comparison_passes_without_finding() {
        let outcome = Outcome::from_comparison(
            chip("esp32s3"),
            "crystal_mhz",
            SafeText::from_board("40 MHz"),
            3,
        );
        assert_eq!(outcome.state(), State::Pass);
        assert_eq!(outcome.finding(), None);
        assert_eq!(outcome.expected().map(SafeText::as_str), Some("40 MHz"));
        assert_eq!(outcome.read().map(SafeText::as_str), Some("40 MHz"));
    }

    #[test]
    fn invariant_mismatch_fails_with_finding() {
        let outcome = Outcome::from_comparison(
            chip("esp32s3"),
            "identity",
            SafeText::from_board("security info chip id 13"),
            12,
        );
        assert_eq!(outcome.state(), State::Fail);
        assert!(outcome.reason().is_some_and(|reason| !reason.is_empty()));
        let finding = outcome.finding().expect("finding");
        assert_eq!(finding.field(), "identity");
        assert_eq!(finding.kind(), FieldKind::Invariant);
        assert_eq!(finding.table().as_str(), "security info chip id 9");
        assert_eq!(finding.read().as_str(), "security info chip id 13");
    }

    #[test]
    fn variable_mismatch_passes_with_finding() {
        // Spec example 8.4: ESP32 with a 26 MHz crystal.
        let outcome = Outcome::from_comparison(
            chip("esp32"),
            "crystal_mhz",
            SafeText::from_board("26 MHz"),
            3,
        );
        assert_eq!(outcome.state(), State::Pass);
        assert_eq!(outcome.reason(), None);
        assert_eq!(outcome.expected().map(SafeText::as_str), Some("40 MHz"));
        assert_eq!(outcome.read().map(SafeText::as_str), Some("26 MHz"));
        let finding = outcome.finding().expect("finding");
        assert_eq!(finding.field(), "crystal_mhz");
        assert_eq!(finding.kind(), FieldKind::Variable);
        assert_eq!(finding.table().as_str(), "40 MHz");
        assert_eq!(finding.read().as_str(), "26 MHz");
        assert_eq!(
            finding.source().as_str(),
            "ESP32 Series Datasheet v5.3, Features; ESP32 Series Comparison"
        );
    }

    #[test]
    fn unknown_capability_field_has_no_finding() {
        assert_eq!(
            Finding::from_table(chip("esp32"), "flux_capacitor", SafeText::from_board("x")),
            None
        );
    }

    #[test]
    #[cfg_attr(debug_assertions, should_panic(expected = "non-blank reason"))]
    fn fail_and_skip_require_a_reason() {
        // Debug builds: a blank reason is a runner bug caught by `debug_assert!`.
        // Release builds: it is normalized and the reason is still non-empty.
        let fail = Outcome::fail("  ", 1, None, None);
        let skip = Outcome::skip("");
        assert_eq!(fail.reason().map(SafeText::as_str), Some(NO_REASON_GIVEN));
        assert_eq!(skip.reason().map(SafeText::as_str), Some(NO_REASON_GIVEN));
    }

    #[test]
    fn blank_reason_is_normalized_to_a_fixed_text() {
        assert_eq!(reason_or_placeholder("").as_str(), NO_REASON_GIVEN);
        assert_eq!(reason_or_placeholder(" \t").as_str(), NO_REASON_GIVEN);
        assert_eq!(reason_or_placeholder("timeout").as_str(), "timeout");
    }

    #[test]
    fn pass_has_no_reason_and_skip_has_no_duration_or_values() {
        assert_eq!(pass().reason(), None);
        let skip = Outcome::skip("not applicable");
        assert_eq!(skip.duration_ms(), 0);
        assert_eq!(skip.expected(), None);
        assert_eq!(skip.read(), None);
        assert_eq!(skip.finding(), None);
    }

    #[test]
    fn sanitizes_escape_sequences_from_board_text() {
        assert_eq!(SafeText::from_board("\x1b[31mred").as_str(), "?[31mred");
        assert_eq!(SafeText::from_board("a\r\nb\tc\0").as_str(), "a??b?c?");
    }

    #[test]
    fn replaces_non_ascii_and_del() {
        assert_eq!(SafeText::from_board("caf\u{e9}\x7f").as_str(), "caf??");
        // One multi-byte char is one `?`, never half a UTF-8 sequence.
        assert_eq!(SafeText::from_host("\u{1F600}!").as_str(), "?!");
        // Invalid UTF-8 decoded lossily (U+FFFD) also becomes `?`.
        let lossy = String::from_utf8_lossy(&[b'o', 0xFF, b'k']);
        assert_eq!(SafeText::from_board(&lossy).as_str(), "o?k");
        // The whole printable range survives untouched.
        let printable: String = (' '..='~').collect();
        assert_eq!(SafeText::from_host(&printable).as_str(), printable);
    }

    #[test]
    fn truncates_board_text_at_128_with_ellipsis() {
        let exact = "x".repeat(BOARD_TEXT_MAX);
        assert_eq!(SafeText::from_board(&exact).as_str(), exact);

        let long = SafeText::from_board(&"x".repeat(BOARD_TEXT_MAX + 1));
        assert_eq!(long.as_str().len(), BOARD_TEXT_MAX);
        assert!(long.as_str().ends_with("..."));
        assert_eq!(&long.as_str()[..BOARD_TEXT_MAX - 3], "x".repeat(125));

        // Limit counts characters, not bytes: 200 two-byte chars become 125 `?` + "...".
        let wide = SafeText::from_board(&"\u{e9}".repeat(200));
        assert_eq!(wide.as_str(), format!("{}...", "?".repeat(125)));
    }

    #[test]
    fn truncates_host_text_at_256() {
        let exact = "y".repeat(HOST_TEXT_MAX);
        assert_eq!(SafeText::from_host(&exact).as_str(), exact);

        let long = SafeText::from_host(&"y".repeat(1000));
        assert_eq!(long.as_str().len(), HOST_TEXT_MAX);
        assert_eq!(long.as_str(), format!("{}...", "y".repeat(253)));
    }

    #[test]
    fn port_ids_are_four_uppercase_hex_digits() {
        let port = PortInfo {
            kind: PortKind::Native,
            vid: 0x303a,
            pid: 0x1001,
            bridge_model: None,
            usb_serial: None,
            primary: true,
        };
        assert_eq!(port.vid_hex(), "303A");
        assert_eq!(port.pid_hex(), "1001");
    }

    /// Spec example 8.3: ESP32-S3 over a CH343, cable pulled during `flash/jedec-id`.
    #[test]
    fn models_spec_example_fail_with_not_run() {
        let s3 = chip("esp32s3");
        let mac = "10:51:db:84:14:0c";
        let board = BoardInfo {
            chip: Some(ChipInfo {
                chip: s3,
                description: Some(SafeText::from_board("ESP32-S3 (QFN56)")),
                revision: Some(Revision { major: 0, minor: 2 }),
            }),
            mac: text(mac),
            ports: vec![PortInfo {
                kind: PortKind::Bridge,
                vid: 0x1A86,
                pid: 0x55D3,
                bridge_model: text("CH343"),
                usb_serial: Some(SafeText::from_board("5D28088406")),
                primary: true,
            }],
        };
        let mut run = Run::new("0.1.0", board, catalog());
        let mac_pass = |ms| Outcome::pass(ms, text(mac), text(mac));
        let read = |ms, value| Outcome::pass(ms, None, text(value));

        run.finish(UART_BRIDGE, mac_pass(2295)).unwrap();
        run.finish(
            USB_SERIAL_JTAG,
            Outcome::skip("native USB not connected on this board"),
        )
        .unwrap();
        run.finish(
            CHIP_IDENTITY,
            Outcome::from_comparison(
                s3,
                "identity",
                SafeText::from_board("security info chip id 9"),
                12,
            ),
        )
        .unwrap();
        run.finish(REVISION, read(9, "v0.2")).unwrap();
        run.finish(
            CRYSTAL,
            Outcome::from_comparison(s3, "crystal_mhz", SafeText::from_board("40 MHz"), 3),
        )
        .unwrap();
        run.finish(MAC_EFUSE_REG, mac_pass(8)).unwrap();
        run.finish(PACKAGE, read(6, "package 0")).unwrap();
        run.finish(PSRAM, read(5, "8 MB in package (AP_3v3)"))
            .unwrap();
        run.finish(EMBEDDED_FLASH, read(5, "none")).unwrap();
        run.start(JEDEC_ID).unwrap();
        run.abort("port disconnected during the flash ID read", 212)
            .unwrap();
        let report = run.complete(HardReset::not_done("port disconnected"));

        assert_eq!(report.tool_version().as_str(), "0.1.0");
        assert_eq!(
            report.summary(),
            Summary {
                passed: 8,
                failed: 1,
                skipped: 2,
                total: 11
            }
        );
        assert_eq!(report.exit_code(), 1);
        assert_eq!(
            reason_of(&report, USB_SERIAL_JTAG),
            Some("native USB not connected on this board")
        );
        assert_eq!(
            reason_of(&report, JEDEC_ID),
            Some("port disconnected during the flash ID read")
        );
        assert_eq!(
            reason_of(&report, FLASH_SIZE),
            Some("not run: flash/jedec-id failed")
        );
        assert!(
            report
                .tests()
                .iter()
                .all(|test| test.outcome.finding().is_none())
        );
        assert!(!report.hard_reset().is_done());
        assert_eq!(
            report.hard_reset().reason().map(SafeText::as_str),
            Some("port disconnected")
        );

        let chip_info = report.board().chip.as_ref().expect("chip");
        assert_eq!(chip_info.chip.caps().target, "esp32s3");
        assert_eq!(chip_info.chip.caps().name, "ESP32-S3");
        assert_eq!(
            chip_info.revision.map(|r| r.to_string()).as_deref(),
            Some("v0.2")
        );
        let port = &report.board().ports[0];
        assert_eq!(
            (port.vid_hex().as_str(), port.pid_hex().as_str()),
            ("1A86", "55D3")
        );
    }
}
