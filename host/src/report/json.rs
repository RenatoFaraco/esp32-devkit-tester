//! The `--json` document: one [`Report`] as `docs/context/spec/report-json.md` (schema v1).
//!
//! Pure: [`to_json_line`] turns the model into the text the host prints, nothing else. The DTOs
//! below carry the spec's names exactly and are filled field by field, so a key can never go
//! missing (D3: an absent value is `null`, never an omitted key) and the declaration order puts
//! `schema_version` first.
//!
//! Every string comes from a [`SafeText`] or a host literal, so the document is printable ASCII
//! (§5.1, D4): `serde_json` then has nothing to escape but `"` and `\`. The port name is not in
//! the model, so it cannot reach the document (D1).

use std::fmt;
use std::sync::{Mutex, OnceLock, PoisonError};

use serde::{Deserialize, Serialize};

use super::{
    BoardInfo, ChipInfo, Finding, HardReset as ModelHardReset, Outcome, PortInfo,
    PortKind as ModelPortKind, Report, Revision as ModelRevision, Run, SafeText, State, Suite,
    TOOL_NAME, TestId, TestResult,
};
use crate::chips::Chip as ModelChip;

/// The only `schema_version` this host emits.
pub const SCHEMA_VERSION: u32 = 1;

/// The whole document (spec §2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Document {
    schema_version: u32,
    tool: Tool,
    board: Board,
    tests: Vec<Test>,
    summary: Summary,
    hard_reset: HardReset,
}

/// `tool` (spec §3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Tool {
    name: String,
    version: String,
}

/// `board` (spec §3.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Board {
    chip: Option<Chip>,
    mac: Option<String>,
    ports: Vec<Port>,
}

/// `board.chip`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Chip {
    target: String,
    name: String,
    description: Option<String>,
    revision: Option<Revision>,
}

/// `board.chip.revision`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Revision {
    major: u32,
    minor: u32,
}

/// `board.ports[]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Port {
    kind: String,
    vid: String,
    pid: String,
    bridge_model: Option<String>,
    usb_serial: Option<String>,
    primary: bool,
}

/// `tests[]` (spec §3.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Test {
    suite: String,
    name: String,
    state: String,
    reason: Option<String>,
    duration_ms: u64,
    expected: Option<String>,
    read: Option<String>,
    finding: Option<FindingDto>,
}

/// `tests[].finding` (spec §3.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FindingDto {
    field: String,
    kind: String,
    table: String,
    read: String,
    source: String,
}

/// `summary` (spec §3.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Summary {
    passed: usize,
    failed: usize,
    skipped: usize,
    total: usize,
}

/// `hard_reset` (spec §3.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct HardReset {
    done: bool,
    reason: Option<String>,
}

fn text(value: &SafeText) -> String {
    value.as_str().to_owned()
}

fn optional(value: Option<&SafeText>) -> Option<String> {
    value.map(text)
}

impl Document {
    fn of(report: &Report) -> Document {
        let board = report.board();
        let summary = report.summary();
        Document {
            schema_version: SCHEMA_VERSION,
            tool: Tool {
                name: TOOL_NAME.to_owned(),
                version: text(report.tool_version()),
            },
            board: Board {
                chip: board.chip.as_ref().map(|info| {
                    let caps = info.chip.caps();
                    Chip {
                        target: caps.target.to_owned(),
                        name: caps.name.to_owned(),
                        description: optional(info.description.as_ref()),
                        revision: info.revision.map(|revision| Revision {
                            major: revision.major,
                            minor: revision.minor,
                        }),
                    }
                }),
                mac: optional(board.mac.as_ref()),
                ports: board.ports.iter().map(Port::of).collect(),
            },
            tests: report.tests().iter().map(Test::of).collect(),
            summary: Summary {
                passed: summary.passed,
                failed: summary.failed,
                skipped: summary.skipped,
                total: summary.total,
            },
            hard_reset: HardReset {
                done: report.hard_reset().is_done(),
                reason: optional(report.hard_reset().reason()),
            },
        }
    }
}

impl Port {
    fn of(port: &PortInfo) -> Port {
        Port {
            kind: port.kind.as_str().to_owned(),
            vid: port.vid_hex(),
            pid: port.pid_hex(),
            bridge_model: optional(port.bridge_model.as_ref()),
            usb_serial: optional(port.usb_serial.as_ref()),
            primary: port.primary,
        }
    }
}

impl Test {
    fn of(test: &TestResult) -> Test {
        let outcome = &test.outcome;
        Test {
            suite: test.id.suite.as_str().to_owned(),
            name: test.id.name.to_owned(),
            state: outcome.state().as_str().to_owned(),
            reason: optional(outcome.reason()),
            duration_ms: outcome.duration_ms(),
            expected: optional(outcome.expected()),
            read: optional(outcome.read()),
            finding: outcome.finding().map(FindingDto::of),
        }
    }
}

impl FindingDto {
    fn of(finding: &Finding) -> FindingDto {
        FindingDto {
            field: finding.field().to_owned(),
            kind: finding.kind().to_string(),
            table: text(finding.table()),
            read: text(finding.read()),
            source: text(finding.source()),
        }
    }
}

/// `report` as the `--json` document: one compact line ending in `\n` (LF on every OS, D5).
///
/// The error can only come from `serde_json` itself (the DTOs hold strings, integers and
/// booleans); the caller then prints nothing on stdout, as for any run without a document.
pub fn to_json_line(report: &Report) -> Result<String, serde_json::Error> {
    let mut line = serde_json::to_string(&Document::of(report))?;
    line.push('\n');
    Ok(line)
}

/// Longest test name [`parse`] accepts (the v0.1 names are under 20 characters).
const TEST_NAME_MAX: usize = 64;

/// Why [`parse`] could not turn a document back into a [`Report`].
#[derive(Debug)]
pub enum ParseError {
    /// Not JSON, or not the shape of spec §2.
    Json(serde_json::Error),
    /// A `schema_version` this host does not know (spec §6: refuse it).
    UnsupportedVersion(u32),
    /// Well-formed, but the model cannot hold it as it is (an unknown chip or suite, a
    /// finding that disagrees with the capability table, a summary that disagrees with the
    /// tests...). The text is host-written English.
    Invalid(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Json(error) => write!(f, "not a report document: {error}"),
            ParseError::UnsupportedVersion(version) => {
                write!(
                    f,
                    "schema_version {version} is not supported (only {SCHEMA_VERSION})"
                )
            }
            ParseError::Invalid(reason) => write!(f, "invalid report: {reason}"),
        }
    }
}

impl std::error::Error for ParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ParseError::Json(error) => Some(error),
            ParseError::UnsupportedVersion(_) | ParseError::Invalid(_) => None,
        }
    }
}

impl From<serde_json::Error> for ParseError {
    fn from(error: serde_json::Error) -> Self {
        ParseError::Json(error)
    }
}

fn invalid(reason: impl Into<String>) -> ParseError {
    ParseError::Invalid(reason.into())
}

/// The inverse of [`to_json_line`]: a `--json` document back into a [`Report`].
///
/// The report is rebuilt through the model's own constructors ([`Run`], [`Outcome`],
/// [`HardReset`](ModelHardReset)), never by filling private fields, so every invariant of the
/// model holds for a parsed report too. A document those constructors would not produce is
/// [`ParseError::Invalid`] rather than "fixed": a `pass` with a reason, a `skip` that took time
/// or read something, a blank `fail`/`skip` reason, a finding that is not what the capability
/// table of this host says, tests out of suite order, a duplicate test, a summary that does not
/// count the tests. Every string goes through [`SafeText`] again (board text with the board
/// limit, host text with the host one), so the result is as safe to print as a live report.
///
/// As spec §6 asks of a consumer: an unknown `schema_version` is refused and unknown keys are
/// ignored. Closed sets of the model (suites, port kinds, chips) are not extended: a value this
/// host does not know is [`ParseError::Invalid`].
///
/// Test names are `&'static str` in the model; a name is validated (kebab-case, at most 64
/// characters) and interned once per distinct name for the life of the process. Used by the
/// manual's image generator (`examples/manual_screenshots.rs`) to redraw recorded sessions.
pub fn parse(text: &str) -> Result<Report, ParseError> {
    let document: Document = serde_json::from_str(text)?;
    if document.schema_version != SCHEMA_VERSION {
        return Err(ParseError::UnsupportedVersion(document.schema_version));
    }
    if document.tool.name != TOOL_NAME {
        return Err(invalid(format!(
            "tool.name is {:?}, not {TOOL_NAME:?}",
            SafeText::from_host(&document.tool.name).as_str()
        )));
    }
    let board = document.board.model()?;
    let chip = board.chip.as_ref().map(|info| info.chip);

    let mut plan: Vec<TestId> = Vec::with_capacity(document.tests.len());
    for test in &document.tests {
        let id = test.id()?;
        if plan.contains(&id) {
            return Err(invalid(format!("test {id} appears twice")));
        }
        plan.push(id);
    }
    let mut run = Run::new(document.tool.version.as_str(), board, plan.clone());
    for (test, id) in document.tests.iter().zip(&plan) {
        let outcome = test
            .outcome(chip)
            .map_err(|reason| invalid(format!("{id}: {reason}")))?;
        run.finish(*id, outcome)
            .map_err(|error| invalid(error.to_string()))?;
    }
    let report = run.complete(document.hard_reset.model()?);

    let order: Vec<TestId> = report.tests().iter().map(|test| test.id).collect();
    if order != plan {
        return Err(invalid("tests are not ordered by suite"));
    }
    let summary = report.summary();
    let given = &document.summary;
    if (
        summary.passed,
        summary.failed,
        summary.skipped,
        summary.total,
    ) != (given.passed, given.failed, given.skipped, given.total)
    {
        return Err(invalid("summary does not count the tests"));
    }
    Ok(report)
}

/// `"e8:f6:0a:f7:ff:d4"`: six pairs of lowercase hex digits (spec §5.2).
fn is_canonical_mac(text: &str) -> bool {
    let parts: Vec<&str> = text.split(':').collect();
    parts.len() == 6
        && parts.iter().all(|part| {
            part.len() == 2
                && part
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

/// `"303A"`: four uppercase hex digits (spec §5.3).
fn parse_hex4(text: &str) -> Option<u16> {
    let canonical = text.len() == 4
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b));
    canonical
        .then(|| u16::from_str_radix(text, 16).ok())
        .flatten()
}

/// `^[a-z][a-z0-9]*(-[a-z0-9]+)*$` (spec §3.3).
fn is_kebab(text: &str) -> bool {
    text.starts_with(|c: char| c.is_ascii_lowercase())
        && text.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// The model's `&'static str` for a test name read from a document, leaked once per distinct
/// name (bounded by the names a process ever parses; validated before it is kept).
fn intern(name: &str) -> Result<&'static str, String> {
    if name.len() > TEST_NAME_MAX || !is_kebab(name) {
        return Err(format!(
            "test name {:?} is not kebab-case of at most {TEST_NAME_MAX} characters",
            SafeText::from_board(name).as_str()
        ));
    }
    static NAMES: OnceLock<Mutex<Vec<&'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(known) = names.iter().find(|known| **known == name) {
        return Ok(known);
    }
    let kept: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.push(kept);
    Ok(kept)
}

fn suite_of(name: &str) -> Option<Suite> {
    [Suite::Port, Suite::Rom, Suite::Efuse, Suite::Flash]
        .into_iter()
        .find(|suite| suite.as_str() == name)
}

/// A reason a `fail`/`skip` (or a hard reset that did not happen) must carry: present and not
/// blank. Checked here so that the model's `debug_assert!` on blank reasons is never reached
/// from a document.
fn required_reason<'r>(reason: Option<&'r String>, what: &str) -> Result<&'r str, String> {
    match reason {
        Some(reason) if !reason.trim().is_empty() => Ok(reason),
        _ => Err(format!("{what} needs a non-blank reason")),
    }
}

impl Board {
    fn model(&self) -> Result<BoardInfo, ParseError> {
        let chip = match &self.chip {
            None => None,
            Some(dto) => {
                let chip = ModelChip::from_target(&dto.target).ok_or_else(|| {
                    invalid(format!(
                        "unknown chip target {:?}",
                        SafeText::from_board(&dto.target).as_str()
                    ))
                })?;
                if chip.caps().name != dto.name {
                    return Err(invalid(format!(
                        "chip name {:?} is not the name of target {}",
                        SafeText::from_board(&dto.name).as_str(),
                        chip.caps().target
                    )));
                }
                Some(ChipInfo {
                    chip,
                    description: dto.description.as_deref().map(SafeText::from_board),
                    revision: dto.revision.as_ref().map(|revision| ModelRevision {
                        major: revision.major,
                        minor: revision.minor,
                    }),
                })
            }
        };
        let mac = match &self.mac {
            None => None,
            Some(mac) if is_canonical_mac(mac) => Some(SafeText::from_host(mac)),
            Some(mac) => {
                return Err(invalid(format!(
                    "board.mac {:?} is not in canonical form",
                    SafeText::from_board(mac).as_str()
                )));
            }
        };
        let ports = self
            .ports
            .iter()
            .map(Port::model)
            .collect::<Result<Vec<_>, _>>()?;
        if !ports.is_empty() && ports.iter().filter(|port| port.primary).count() != 1 {
            return Err(invalid("board.ports needs exactly one primary port"));
        }
        Ok(BoardInfo { chip, mac, ports })
    }
}

impl Port {
    fn model(&self) -> Result<PortInfo, ParseError> {
        let kind = match self.kind.as_str() {
            "bridge" => ModelPortKind::Bridge,
            "native" => ModelPortKind::Native,
            other => {
                return Err(invalid(format!(
                    "port kind {:?} is not known to this host",
                    SafeText::from_board(other).as_str()
                )));
            }
        };
        let hex = |text: &str, what: &str| {
            parse_hex4(text).ok_or_else(|| {
                invalid(format!(
                    "{what} {:?} is not 4 uppercase hex digits",
                    SafeText::from_board(text).as_str()
                ))
            })
        };
        Ok(PortInfo {
            kind,
            vid: hex(&self.vid, "vid")?,
            pid: hex(&self.pid, "pid")?,
            bridge_model: self.bridge_model.as_deref().map(SafeText::from_host),
            usb_serial: self.usb_serial.as_deref().map(SafeText::from_board),
            primary: self.primary,
        })
    }
}

impl Test {
    fn id(&self) -> Result<TestId, ParseError> {
        let suite = suite_of(&self.suite).ok_or_else(|| {
            invalid(format!(
                "suite {:?} is not known to this host",
                SafeText::from_board(&self.suite).as_str()
            ))
        })?;
        Ok(TestId::new(suite, intern(&self.name).map_err(invalid)?))
    }

    /// The outcome through the model's constructors; `Err` is the reason for
    /// [`ParseError::Invalid`].
    fn outcome(&self, chip: Option<ModelChip>) -> Result<Outcome, String> {
        let board_text = |value: &Option<String>| value.as_deref().map(SafeText::from_board);
        let expected = board_text(&self.expected);
        let read = board_text(&self.read);
        if let Some(finding) = &self.finding {
            return self.outcome_with_finding(finding, chip, expected, read);
        }
        match self.state.as_str() {
            "pass" => {
                if self.reason.is_some() {
                    return Err("a pass has no reason".to_owned());
                }
                Ok(Outcome::pass(self.duration_ms, expected, read))
            }
            "fail" => {
                let reason = required_reason(self.reason.as_ref(), "a fail")?;
                Ok(Outcome::fail(reason, self.duration_ms, expected, read))
            }
            "skip" => {
                let reason = required_reason(self.reason.as_ref(), "a skip")?;
                if self.duration_ms != 0 || expected.is_some() || read.is_some() {
                    return Err("a skip takes 0 ms and reads nothing".to_owned());
                }
                Ok(Outcome::skip(reason))
            }
            other => Err(format!(
                "state {:?} is not pass, fail or skip",
                SafeText::from_board(other).as_str()
            )),
        }
    }

    /// A test with a finding is rebuilt by [`Outcome::from_comparison`] (the only place the
    /// ADR-010 rule lives) and must come out exactly as the document says.
    fn outcome_with_finding(
        &self,
        finding: &FindingDto,
        chip: Option<ModelChip>,
        expected: Option<SafeText>,
        read: Option<SafeText>,
    ) -> Result<Outcome, String> {
        let chip = chip.ok_or("a finding needs board.chip")?;
        let read = read.ok_or("a finding needs the value read")?;
        if Finding::from_table(chip, &finding.field, read.clone()).is_none() {
            return Err(format!(
                "finding field {:?} is not in the capability table",
                SafeText::from_board(&finding.field).as_str()
            ));
        }
        let outcome = Outcome::from_comparison(chip, &finding.field, read, self.duration_ms);
        let host_text = |value: &Option<String>| value.as_deref().map(SafeText::from_host);
        let agrees = outcome.state().as_str() == self.state
            && outcome.state() != State::Skip
            && outcome.expected() == expected.as_ref()
            && outcome.reason() == host_text(&self.reason).as_ref()
            && outcome.finding().is_some_and(|built| {
                built.kind().to_string() == finding.kind
                    && *built.table() == SafeText::from_host(&finding.table)
                    && *built.source() == SafeText::from_host(&finding.source)
                    && *built.read() == SafeText::from_board(&finding.read)
            });
        if !agrees {
            return Err(format!(
                "finding on {} disagrees with this host's capability table",
                finding.field
            ));
        }
        Ok(outcome)
    }
}

impl HardReset {
    fn model(&self) -> Result<ModelHardReset, ParseError> {
        match (self.done, &self.reason) {
            (true, None) => Ok(ModelHardReset::done()),
            (true, Some(_)) => Err(invalid("hard_reset.done is true but it has a reason")),
            (false, reason) => Ok(ModelHardReset::not_done(
                required_reason(reason.as_ref(), "a hard reset not done").map_err(invalid)?,
            )),
        }
    }
}

/// The examples of the spec, for the tests of every layer that renders a report.
#[cfg(test)]
pub(crate) mod spec_examples {
    use std::path::PathBuf;

    use serde_json::Value;

    /// The text of the spec, read from the repository (one source for the examples, spec §10).
    fn spec_text() -> String {
        let path: PathBuf = [
            env!("CARGO_MANIFEST_DIR"),
            "..",
            "docs",
            "context",
            "spec",
            "report-json.md",
        ]
        .iter()
        .collect();
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    /// `(id, json)` for each `<!-- example: <id> -->` marker line of the spec (a line of its
    /// own, so the prose that mentions the marker is not one) and the ```json block under it.
    pub(crate) fn all() -> Vec<(String, String)> {
        let spec = spec_text();
        let mut examples = Vec::new();
        let mut lines = spec.lines();
        while let Some(line) = lines.next() {
            let Some(id) = line
                .trim()
                .strip_prefix("<!-- example:")
                .and_then(|rest| rest.strip_suffix("-->"))
            else {
                continue;
            };
            let opening = lines.next().map(str::trim);
            assert_eq!(opening, Some("```json"), "example {id}: no json block");
            let body: Vec<&str> = lines.by_ref().take_while(|l| l.trim() != "```").collect();
            examples.push((id.trim().to_owned(), body.join("\n")));
        }
        examples
    }

    /// The example `id` of the spec, parsed.
    pub(crate) fn example(id: &str) -> Value {
        let (_, text) = all()
            .into_iter()
            .find(|(name, _)| name == id)
            .unwrap_or_else(|| panic!("no example {id} in the spec"));
        serde_json::from_str(&text).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::spec_examples;
    use super::*;
    use crate::chips;
    use crate::report::{
        BoardInfo, ChipInfo, HardReset as ModelHardReset, Outcome, PortKind, Revision as Rev, Run,
        Suite, TestId,
    };

    #[test]
    fn spec_examples_round_trip() {
        let examples = spec_examples::all();
        let ids: Vec<&str> = examples.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["pass", "skip", "fail", "finding"]);
        for (id, text) in &examples {
            let original: Value = serde_json::from_str(text).unwrap();
            let document: Document =
                serde_json::from_str(text).unwrap_or_else(|error| panic!("{id}: {error}"));
            let again = serde_json::to_value(&document).unwrap();
            assert_eq!(again, original, "{id} does not round-trip");

            // The summary is derived from the tests.
            let count = |state: &str| document.tests.iter().filter(|t| t.state == state).count();
            assert_eq!(document.summary.passed, count("pass"), "{id}");
            assert_eq!(document.summary.failed, count("fail"), "{id}");
            assert_eq!(document.summary.skipped, count("skip"), "{id}");
            assert_eq!(document.summary.total, document.tests.len(), "{id}");
            check_shape(&document);
        }
    }

    /// The formats of spec §3–§5 that a round-trip alone does not check.
    fn check_shape(document: &Document) {
        assert_eq!(document.schema_version, 1);
        assert_eq!(document.tool.name, "devkit-tester");
        if let Some(mac) = &document.board.mac {
            assert!(is_mac(mac), "{mac}");
        }
        assert_eq!(document.board.ports.iter().filter(|p| p.primary).count(), 1);
        for port in &document.board.ports {
            assert!(is_hex4(&port.vid) && is_hex4(&port.pid), "{port:?}");
            assert!(port.kind == "bridge" || port.kind == "native");
            assert_eq!(
                port.kind == "native",
                port.bridge_model.is_none(),
                "{port:?}"
            );
        }
        for test in &document.tests {
            assert!(is_kebab(&test.suite) && is_kebab(&test.name), "{test:?}");
            match test.state.as_str() {
                "pass" => assert!(test.reason.is_none(), "{test:?}"),
                "fail" | "skip" => {
                    assert!(test.reason.as_ref().is_some_and(|r| !r.trim().is_empty()));
                }
                other => panic!("state {other}"),
            }
            if test.state == "skip" {
                assert_eq!(test.duration_ms, 0);
            }
        }
        assert_eq!(
            document.hard_reset.done,
            document.hard_reset.reason.is_none()
        );
    }

    fn is_mac(text: &str) -> bool {
        let parts: Vec<&str> = text.split(':').collect();
        parts.len() == 6
            && parts.iter().all(|part| {
                part.len() == 2
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
    }

    fn is_hex4(text: &str) -> bool {
        text.len() == 4
            && text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
    }

    /// `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`
    fn is_kebab(text: &str) -> bool {
        text.starts_with(|c: char| c.is_ascii_lowercase())
            && text.split('-').all(|part| {
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            })
    }

    fn chip(target: &str) -> chips::Chip {
        chips::Chip::from_target(target).unwrap()
    }

    /// A report whose board texts carry an escape, a DEL, non-ASCII and a quote.
    fn hostile_report() -> Report {
        let crystal = TestId::new(Suite::Rom, "crystal");
        let flash = TestId::new(Suite::Flash, "jedec-id");
        let board = BoardInfo {
            chip: Some(ChipInfo {
                chip: chip("esp32s3"),
                description: Some(SafeText::from_board(
                    "\u{1b}[31mESP32-S3\u{7f} (QFN56) é \"x\" \\",
                )),
                revision: Some(Rev { major: 0, minor: 2 }),
            }),
            mac: Some(SafeText::from_host("10:51:db:84:14:0c")),
            ports: vec![PortInfo {
                kind: PortKind::Bridge,
                vid: 0x1A86,
                pid: 0x55D3,
                bridge_model: Some(SafeText::from_host("CH343")),
                usb_serial: Some(SafeText::from_board(&"\n5D2808\u{0}".repeat(40))),
                primary: true,
            }],
        };
        let mut run = Run::new("0.1.0", board, vec![crystal, flash]);
        run.finish(
            crystal,
            Outcome::from_comparison(
                chip("esp32s3"),
                "crystal_mhz",
                SafeText::from_board("26 MHz"),
                3,
            ),
        )
        .unwrap();
        run.finish(
            flash,
            Outcome::fail(
                "port disconnected during the flash ID read",
                212,
                None,
                None,
            ),
        )
        .unwrap();
        run.complete(ModelHardReset::not_done("port disconnected"))
    }

    #[test]
    fn json_is_one_ascii_line() {
        let line = to_json_line(&hostile_report()).unwrap();
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1, "{line}");
        let body = line.trim_end_matches('\n');
        assert!(body.bytes().all(|b| (0x20..=0x7E).contains(&b)), "{body}");
        assert!(!body.contains("\\u"), "no \\uXXXX escape: {body}");
        assert!(body.starts_with("{\"schema_version\":1,"), "{body}");

        let value: Value = serde_json::from_str(body).unwrap();
        let description = value["board"]["chip"]["description"].as_str().unwrap();
        assert_eq!(description, "?[31mESP32-S3? (QFN56) ? \"x\" \\");
        let serial = value["board"]["ports"][0]["usb_serial"].as_str().unwrap();
        assert_eq!(serial.chars().count(), 128);
        assert!(serial.ends_with("...") && serial.starts_with("?5D2808?"));
        // Every key is present, `null` when absent.
        let test = &value["tests"][1];
        for key in [
            "suite",
            "name",
            "state",
            "reason",
            "duration_ms",
            "expected",
            "read",
            "finding",
        ] {
            assert!(test.get(key).is_some(), "{key} missing");
        }
        assert!(test["finding"].is_null() && test["read"].is_null());
        assert_eq!(value["tests"][0]["finding"]["kind"], "variable");
        assert_eq!(value["hard_reset"]["done"], false);

        let document: Document = serde_json::from_str(body).unwrap();
        check_shape(&document);
    }

    #[test]
    fn spec_examples_parse_back_to_the_same_document() {
        for (id, text) in spec_examples::all() {
            let original: Value = serde_json::from_str(&text).unwrap();
            let report = parse(&text).unwrap_or_else(|error| panic!("{id}: {error}"));
            let line = to_json_line(&report).unwrap();
            let again: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(
                again, original,
                "{id} does not survive parse → to_json_line"
            );
        }
    }

    #[test]
    fn a_hostile_report_survives_parse() {
        let report = hostile_report();
        let line = to_json_line(&report).unwrap();
        assert_eq!(parse(&line).unwrap(), report);
    }

    /// The spec example `id`, changed by `edit`, as text.
    fn edited(id: &str, edit: impl FnOnce(&mut Value)) -> String {
        let mut value = spec_examples::example(id);
        edit(&mut value);
        value.to_string()
    }

    fn invalid_reason(text: &str) -> String {
        match parse(text) {
            Err(ParseError::Invalid(reason)) => reason,
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn parse_refuses_other_versions_and_ignores_unknown_keys() {
        let text = edited("pass", |v| v["schema_version"] = 2.into());
        assert!(matches!(
            parse(&text),
            Err(ParseError::UnsupportedVersion(2))
        ));
        let text = edited("pass", |v| {
            v["extra"] = "added later".into();
            v["tests"][0]["extra"] = 1.into();
        });
        assert!(parse(&text).is_ok());
        assert!(matches!(parse("{"), Err(ParseError::Json(_))));
        assert!(matches!(parse("[]"), Err(ParseError::Json(_))));
    }

    #[test]
    fn parse_refuses_what_the_model_would_not_build() {
        type Edit = Box<dyn FnOnce(&mut Value)>;
        let cases: Vec<(&str, Edit)> = vec![
            ("pass", Box::new(|v| v["tests"][1]["reason"] = "why".into())),
            (
                "skip",
                Box::new(|v| v["tests"][1]["duration_ms"] = 5.into()),
            ),
            ("skip", Box::new(|v| v["tests"][1]["reason"] = "  ".into())),
            ("fail", Box::new(|v| v["tests"][9]["reason"] = Value::Null)),
            (
                "pass",
                Box::new(|v| v["tests"][1]["state"] = "flaky".into()),
            ),
            ("pass", Box::new(|v| v["tests"][1]["suite"] = "wifi".into())),
            (
                "pass",
                Box::new(|v| v["tests"][1]["name"] = "Not Kebab".into()),
            ),
            (
                "pass",
                Box::new(|v| {
                    let first = v["tests"][0].clone();
                    v["tests"].as_array_mut().unwrap().push(first);
                }),
            ),
            (
                "pass",
                Box::new(|v| {
                    let tests = v["tests"].as_array_mut().unwrap();
                    tests.swap(0, 9);
                }),
            ),
            ("pass", Box::new(|v| v["summary"]["passed"] = 9.into())),
            (
                "pass",
                Box::new(|v| v["board"]["chip"]["target"] = "esp8266".into()),
            ),
            (
                "pass",
                Box::new(|v| v["board"]["chip"]["name"] = "ESP32-C3".into()),
            ),
            (
                "pass",
                Box::new(|v| v["board"]["mac"] = "E8:F6:0A:F7:FF:D4".into()),
            ),
            (
                "pass",
                Box::new(|v| v["board"]["ports"][0]["vid"] = "303a".into()),
            ),
            (
                "pass",
                Box::new(|v| v["board"]["ports"][0]["kind"] = "jtag".into()),
            ),
            (
                "pass",
                Box::new(|v| v["board"]["ports"][0]["primary"] = false.into()),
            ),
            ("pass", Box::new(|v| v["hard_reset"]["reason"] = "x".into())),
            (
                "fail",
                Box::new(|v| v["hard_reset"]["reason"] = Value::Null),
            ),
            ("pass", Box::new(|v| v["tool"]["name"] = "other".into())),
            (
                "finding",
                Box::new(|v| v["tests"][4]["finding"]["table"] = "24 MHz".into()),
            ),
            (
                "finding",
                Box::new(|v| v["tests"][4]["finding"]["field"] = "nope".into()),
            ),
            (
                "finding",
                Box::new(|v| v["tests"][4]["state"] = "fail".into()),
            ),
            ("finding", Box::new(|v| v["board"]["chip"] = Value::Null)),
        ];
        for (index, (id, edit)) in cases.into_iter().enumerate() {
            let text = edited(id, edit);
            let reason = invalid_reason(&text);
            assert!(!reason.is_empty(), "case {index}");
        }
    }

    #[test]
    fn interned_names_are_shared() {
        let a = intern("usb-serial-jtag").unwrap();
        let b = intern("usb-serial-jtag").unwrap();
        assert!(std::ptr::eq(a, b));
        assert!(intern(&"a".repeat(TEST_NAME_MAX + 1)).is_err());
        assert!(intern("x\u{1b}[31m").is_err());
    }

    #[test]
    fn exit_code_one_iff_failed() {
        let failed = hostile_report();
        let line = to_json_line(&failed).unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(failed.exit_code(), 1);
        assert_eq!(value["summary"]["failed"], 1);

        let id = TestId::new(Suite::Rom, "revision");
        let mut run = Run::new(
            "0.1.0",
            BoardInfo {
                chip: None,
                mac: None,
                ports: Vec::new(),
            },
            vec![id],
        );
        run.finish(id, Outcome::skip("not applicable")).unwrap();
        let passed = run.complete(ModelHardReset::done());
        let value: Value = serde_json::from_str(&to_json_line(&passed).unwrap()).unwrap();
        assert_eq!(passed.exit_code(), 0);
        assert_eq!(value["summary"]["failed"], 0);
        assert!(value["board"]["chip"].is_null() && value["board"]["mac"].is_null());
    }
}
