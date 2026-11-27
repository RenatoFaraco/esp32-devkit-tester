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

use serde::Serialize;

use super::{Finding, PortInfo, Report, SafeText, TOOL_NAME, TestResult};

/// The only `schema_version` this host emits.
pub const SCHEMA_VERSION: u32 = 1;

/// The whole document (spec §2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(serde::Deserialize), serde(deny_unknown_fields))]
struct Document {
    schema_version: u32,
    tool: Tool,
    board: Board,
    tests: Vec<Test>,
    summary: Summary,
    hard_reset: HardReset,
}

/// `tool` (spec §3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(serde::Deserialize), serde(deny_unknown_fields))]
struct Tool {
    name: String,
    version: String,
}

/// `board` (spec §3.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(serde::Deserialize), serde(deny_unknown_fields))]
struct Board {
    chip: Option<Chip>,
    mac: Option<String>,
    ports: Vec<Port>,
}

/// `board.chip`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(serde::Deserialize), serde(deny_unknown_fields))]
struct Chip {
    target: String,
    name: String,
    description: Option<String>,
    revision: Option<Revision>,
}

/// `board.chip.revision`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(serde::Deserialize), serde(deny_unknown_fields))]
struct Revision {
    major: u32,
    minor: u32,
}

/// `board.ports[]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(serde::Deserialize), serde(deny_unknown_fields))]
struct Port {
    kind: String,
    vid: String,
    pid: String,
    bridge_model: Option<String>,
    usb_serial: Option<String>,
    primary: bool,
}

/// `tests[]` (spec §3.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(serde::Deserialize), serde(deny_unknown_fields))]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(serde::Deserialize), serde(deny_unknown_fields))]
struct FindingDto {
    field: String,
    kind: String,
    table: String,
    read: String,
    source: String,
}

/// `summary` (spec §3.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(serde::Deserialize), serde(deny_unknown_fields))]
struct Summary {
    passed: usize,
    failed: usize,
    skipped: usize,
    total: usize,
}

/// `hard_reset` (spec §3.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(serde::Deserialize), serde(deny_unknown_fields))]
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
