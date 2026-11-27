//! The recorded session: what one real run on the bench saw, as a versioned JSON file in
//! `docs/project/manual/sessions/<name>.json` (format below, [`SESSION_FORMAT`]).
//!
//! ```text
//! {
//!   "format_version": 1,
//!   "captured": { "date": "2026-11-27T14:03:12Z", "host_version": "0.1.0",
//!                 "note": "C6, CH343 + native", "port": null, "run": true },
//!   "ports":      [ { "name": "COM13", "usb": { "vid": "1A86", "pid": "55D3", ... } }, ... ],
//!   "identified": [ { "result": "found", "port": {...}, "mac": "...", "chip": "esp32c6",
//!                     "reset": { "done": true, "reason": null } }, ... ],
//!   "report":     <the --json document of spec report-json.md> | null
//! }
//! ```
//!
//! # Why DTOs here and no serde in `transport` or `runner`
//! [`PortDesc`], [`UsbIds`] and [`Identified`] stay plain domain types: the session is a
//! documentation artefact with its own version, and the day its format changes the domain must
//! not change with it. The records below are converted field by field, and on the way back
//! through the domain's own parsers ([`Mac::parse`], [`Chip::from_target`], [`HardReset`]), so a
//! hand-edited session cannot build what the runner never would. VID/PID are written as in the
//! spec (`"303A"`), the chip by its target (`"esp32c6"`).
//!
//! # Port names
//! The session is the only file that keeps the OS port names (`COM13`): the screens show them,
//! exactly as on the bench. The report inside it is the spec document, which has none (D1).

use serde::{Deserialize, Serialize};

use crate::chips::Chip;
use crate::probe::Probe;
use crate::report::json::{self, ParseError, to_json_line};
use crate::report::{HardReset, Report};
use crate::runner::{self, Board, Clock, Identified, Sleeper};
use crate::transport::{Mac, PortDesc, UsbIds};
use crate::ui::cli::{board_of_identified, native_mac, pick_only_board, select_port};

/// The only `format_version` of a session this host reads and writes.
pub const SESSION_FORMAT: u32 = 1;

/// One recorded session (module docs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// [`SESSION_FORMAT`].
    pub format_version: u32,
    /// When and how it was recorded.
    pub captured: Captured,
    /// The enumeration at the start, in OS order.
    pub ports: Vec<PortRecord>,
    /// The identification of the bridges (and of the `--port` bridge), in order.
    pub identified: Vec<IdentifiedRecord>,
    /// The `--json` document of the run (spec `report-json.md`); `null` when nothing ran.
    pub report: Option<serde_json::Value>,
}

/// `captured`: when, by which host, and the options of the recording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Captured {
    /// UTC, ISO-8601 (`2026-11-27T14:03:12Z`).
    pub date: String,
    /// `CARGO_PKG_VERSION` of the host that recorded it.
    pub host_version: String,
    /// Free text for the manual's author (which board, which cable, what was done).
    pub note: String,
    /// The `--port` value of the recording (`None`: every port was considered, as without
    /// `--port`). Decides which images are drawn: the TUI has no `--port`.
    pub port: Option<String>,
    /// Whether a run was asked for (`false` for `--no-run`).
    pub run: bool,
}

/// One enumerated port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortRecord {
    /// The OS name (`COM13`).
    pub name: String,
    /// USB identity; `None` for a port that is not USB.
    pub usb: Option<UsbRecord>,
}

/// The USB identity of a port, VID/PID as 4 uppercase hex digits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsbRecord {
    /// `"303A"`.
    pub vid: String,
    /// `"1001"`.
    pub pid: String,
    /// USB serial number as the OS reported it.
    pub serial: Option<String>,
    /// Manufacturer string as the OS reported it.
    pub manufacturer: Option<String>,
    /// Product string as the OS reported it.
    pub product: Option<String>,
    /// USB interface number, when the OS exposed it.
    pub interface: Option<u8>,
}

/// One identification result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "lowercase")]
pub enum IdentifiedRecord {
    /// The ROM answered.
    Found {
        /// The port.
        port: PortRecord,
        /// Canonical MAC (`e8:f6:0a:f7:ff:d4`).
        mac: String,
        /// Chip target (`esp32c6`); `None` if that read failed.
        chip: Option<String>,
        /// The hard reset afterwards.
        reset: ResetRecord,
    },
    /// The port could not be identified.
    Failed {
        /// The port.
        port: PortRecord,
        /// Why (host text).
        reason: String,
    },
}

/// A hard reset: `done`, or not with a reason (as in the spec's `hard_reset`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResetRecord {
    /// Whether the board went back to its app.
    pub done: bool,
    /// Why not; `None` when it did.
    pub reason: Option<String>,
}

/// A session file that cannot be used.
#[derive(Debug)]
pub enum SessionError {
    /// Not JSON, or not the shape above.
    Json(serde_json::Error),
    /// A `format_version` this host does not know.
    UnsupportedFormat(u32),
    /// A record the domain rejects (host text).
    Invalid(String),
    /// The `report` is not a valid spec document.
    Report(ParseError),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::Json(error) => write!(f, "not a session file: {error}"),
            SessionError::UnsupportedFormat(version) => write!(
                f,
                "session format_version {version} is not supported (only {SESSION_FORMAT})"
            ),
            SessionError::Invalid(reason) => write!(f, "invalid session: {reason}"),
            SessionError::Report(error) => write!(f, "session report: {error}"),
        }
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SessionError::Json(error) => Some(error),
            SessionError::Report(error) => Some(error),
            SessionError::UnsupportedFormat(_) | SessionError::Invalid(_) => None,
        }
    }
}

impl From<serde_json::Error> for SessionError {
    fn from(error: serde_json::Error) -> Self {
        SessionError::Json(error)
    }
}

fn hex4(value: u16) -> String {
    format!("{value:04X}")
}

fn parse_hex4(text: &str, what: &str) -> Result<u16, SessionError> {
    let canonical = text.len() == 4 && text.bytes().all(|b| b.is_ascii_hexdigit());
    canonical
        .then(|| u16::from_str_radix(text, 16).ok())
        .flatten()
        .ok_or_else(|| SessionError::Invalid(format!("{what} {text:?} is not 4 hex digits")))
}

impl PortRecord {
    /// The record of `port`.
    pub fn of(port: &PortDesc) -> PortRecord {
        PortRecord {
            name: port.name.clone(),
            usb: port.usb.as_ref().map(|usb| UsbRecord {
                vid: hex4(usb.vid),
                pid: hex4(usb.pid),
                serial: usb.serial.clone(),
                manufacturer: usb.manufacturer.clone(),
                product: usb.product.clone(),
                interface: usb.interface,
            }),
        }
    }

    /// The port again.
    pub fn port(&self) -> Result<PortDesc, SessionError> {
        let usb = match &self.usb {
            None => None,
            Some(usb) => Some(UsbIds {
                vid: parse_hex4(&usb.vid, "vid")?,
                pid: parse_hex4(&usb.pid, "pid")?,
                serial: usb.serial.clone(),
                manufacturer: usb.manufacturer.clone(),
                product: usb.product.clone(),
                interface: usb.interface,
            }),
        };
        Ok(PortDesc {
            name: self.name.clone(),
            usb,
        })
    }
}

impl IdentifiedRecord {
    /// The record of `result`.
    pub fn of(result: &Identified) -> IdentifiedRecord {
        match result {
            Identified::Found {
                port,
                mac,
                chip,
                reset,
            } => IdentifiedRecord::Found {
                port: PortRecord::of(port),
                mac: mac.to_string(),
                chip: chip.map(|chip| chip.caps().target.to_owned()),
                reset: ResetRecord {
                    done: reset.is_done(),
                    reason: reset.reason().map(|reason| reason.as_str().to_owned()),
                },
            },
            Identified::Failed { port, reason } => IdentifiedRecord::Failed {
                port: PortRecord::of(port),
                reason: reason.clone(),
            },
        }
    }

    /// The result again, through the domain's parsers.
    pub fn identified(&self) -> Result<Identified, SessionError> {
        match self {
            IdentifiedRecord::Found {
                port,
                mac,
                chip,
                reset,
            } => {
                let mac = Mac::parse(mac)
                    .map_err(|_| SessionError::Invalid(format!("mac {mac:?} is not a MAC")))?;
                let chip = match chip {
                    None => None,
                    Some(target) => Some(Chip::from_target(target).ok_or_else(|| {
                        SessionError::Invalid(format!("unknown chip target {target:?}"))
                    })?),
                };
                let reset = match (reset.done, &reset.reason) {
                    (true, None) => HardReset::done(),
                    (false, Some(reason)) if !reason.trim().is_empty() => {
                        HardReset::not_done(reason)
                    }
                    _ => {
                        return Err(SessionError::Invalid(
                            "reset: done without a reason, or not done with one".to_owned(),
                        ));
                    }
                };
                Ok(Identified::Found {
                    port: port.port()?,
                    mac,
                    chip,
                    reset,
                })
            }
            IdentifiedRecord::Failed { port, reason } => Ok(Identified::Failed {
                port: port.port()?,
                reason: reason.clone(),
            }),
        }
    }
}

impl Session {
    /// Reads a session file.
    pub fn from_json(text: &str) -> Result<Session, SessionError> {
        let session: Session = serde_json::from_str(text)?;
        if session.format_version != SESSION_FORMAT {
            return Err(SessionError::UnsupportedFormat(session.format_version));
        }
        Ok(session)
    }

    /// The file's text: pretty JSON ending in `\n`. Deterministic: the same session gives the
    /// same bytes (struct fields in declaration order, the report's keys sorted).
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        Ok(text)
    }

    /// The enumerated ports.
    pub fn ports(&self) -> Result<Vec<PortDesc>, SessionError> {
        self.ports.iter().map(PortRecord::port).collect()
    }

    /// The identification results.
    pub fn identified(&self) -> Result<Vec<Identified>, SessionError> {
        self.identified
            .iter()
            .map(IdentifiedRecord::identified)
            .collect()
    }

    /// The report, through [`json::parse`]; `None` when nothing ran.
    pub fn report(&self) -> Result<Option<Report>, SessionError> {
        self.report
            .as_ref()
            .map(|value| json::parse(&value.to_string()).map_err(SessionError::Report))
            .transpose()
    }
}

/// What to record (the options of `examples/record_session.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    /// [`Captured::date`].
    pub date: String,
    /// [`Captured::note`].
    pub note: String,
    /// `--port`: only this port (name, USB serial number or MAC), as `devkit-tester --port`.
    pub port: Option<String>,
    /// `false` for `--no-run`: the enumeration and the identification only.
    pub run: bool,
}

/// The board access a recording uses, injected so tests record from fake probes.
pub struct Bench<'a> {
    /// A fresh [`Probe`] per session (`EspflashProbe` on the bench).
    pub probe: &'a mut dyn FnMut(&PortDesc) -> Box<dyn Probe>,
    /// Times the tests.
    pub clock: &'a dyn Clock,
    /// Pauses while a port comes back.
    pub sleeper: &'a dyn Sleeper,
    /// Lists the ports again while one re-enumerates.
    pub enumerate: &'a mut dyn FnMut() -> Vec<PortDesc>,
}

/// Records one session from `ports` (the enumeration at the start), choosing the board exactly
/// as `devkit-tester --no-tui` does (same functions): with `--port`, that port alone (identified
/// first unless it is a native port with its MAC as serial number); without it, the bridges are
/// identified and the run happens only when exactly one board is found. Read-only like the
/// tester: the same [`runner::identify`] and [`runner::run_board`].
///
/// `Err` is the stderr text of a `--port` that matches no testable port (nothing was opened).
pub fn capture(
    options: &Capture,
    ports: Vec<PortDesc>,
    bench: Bench<'_>,
) -> Result<Session, String> {
    let Bench {
        probe,
        clock,
        sleeper,
        enumerate,
    } = bench;
    let (identified, board): (Vec<Identified>, Option<Board>) = match &options.port {
        Some(wanted) => {
            let port = select_port(&ports, wanted)?;
            match native_mac(&port) {
                Some(mac) => (Vec::new(), Board::new(mac, None, vec![port])),
                None => {
                    let found = runner::identify(&mut *probe, std::slice::from_ref(&port));
                    let board = board_of_identified(&port, wanted, found.first()).ok();
                    (found, board)
                }
            }
        }
        None => {
            let grouping = runner::pre_group(&ports);
            let found = runner::identify(&mut *probe, &grouping.unidentified);
            let board = pick_only_board(&grouping, &found).ok();
            (found, board)
        }
    };
    let report = match board.filter(|_| options.run) {
        Some(board) => {
            let report = runner::run_board(probe, &board, clock, sleeper, enumerate, &mut |_| {});
            let line = to_json_line(&report).map_err(|error| error.to_string())?;
            Some(serde_json::from_str(&line).map_err(|error| error.to_string())?)
        }
        None => None,
    };
    Ok(Session {
        format_version: SESSION_FORMAT,
        captured: Captured {
            date: options.date.clone(),
            host_version: env!("CARGO_PKG_VERSION").to_owned(),
            note: options.note.clone(),
            port: options.port.clone(),
            run: options.run,
        },
        ports: ports.iter().map(PortRecord::of).collect(),
        identified: identified.iter().map(IdentifiedRecord::of).collect(),
        report,
    })
}

/// `secs` after the Unix epoch as UTC ISO-8601 (`2026-11-27T14:03:12Z`), without a date crate
/// (days to civil date: H. Hinnant's `civil_from_days`).
pub fn iso8601_utc(secs: u64) -> String {
    let days = secs / 86_400;
    let rest = secs % 86_400;
    let (hour, minute, second) = (rest / 3600, rest % 3600 / 60, rest % 60);
    // Shift the epoch to 0000-03-01 so that a leap day is the last day of its year.
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}
