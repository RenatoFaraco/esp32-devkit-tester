//! The command line: arguments, `--no-tui` text, `--json` and exit codes (#24, ADR-010 §5–§6).
//!
//! ```text
//! devkit-tester                              TUI
//! devkit-tester --no-tui [--port X]          plain-text report
//! devkit-tester --no-tui --json [--port X]   one JSON line (spec report-json.md); --json implies --no-tui
//! devkit-tester --help | --version
//! ```
//!
//! Lives in `ui` because it is the top of the stack: it renders the report (with the TUI's own
//! layout, see below) and is the only other place that talks to the terminal. It never talks to
//! a board either: like the TUI, it hands [`Job`]s to the runner's [`Worker`] and waits for its
//! [`Event`]s, so the cancellation, the hard reset and the threads are the TUI's.
//!
//! # Streams and exit codes (spec §1)
//! Only the report (text or JSON) goes to stdout, written in one piece after the run ended, so
//! there is never a partial document. Progress, the board list and every error go to stderr.
//! Exit codes: `0`/`1` from [`Report::exit_code`]; `2` when nothing ran (no board, several boards
//! without `--port`, bad argument, WSL1, Ctrl+C), always with an empty stdout.
//!
//! # Choosing the board
//! - `--port X` matches one enumerated port by name (`COM13`, any case), USB serial number or
//!   MAC (any case or separator, through [`Mac::parse`], against the native port's serial). Only
//!   that port is tested, as a board of one port; no other port is opened. A native port whose
//!   serial is a MAC needs no identification; any other port is identified first (one session).
//! - Without `--port`: the ports are enumerated, the bridges identified (each one is reset into
//!   the ROM and back, ADR-008 §4) and grouped. One board runs; none, or several, is code 2 (the
//!   list goes to stderr, with the `--port` value that picks each one).
//!
//! # Ctrl+C
//! The `ctrlc` handler only raises an [`AtomicBool`]; the waiting loop sees it and calls
//! [`Worker::cancel`], so the board gets its hard reset exactly as when the TUI cancels. It is a
//! flag of this module rather than the worker's own because [`Worker::submit`] clears the
//! worker's flag for each new job: a Ctrl+C between identification and the run would be lost.
//!
//! # The text
//! The same [`Line`]s as the TUI (the report body and summary of [`report_view`], the compact
//! card of [`board_view`], the header and rules of the `Done` screen), turned into plain strings:
//! - **colour** (ANSI SGR) only when stdout is a terminal and `NO_COLOR` is not set;
//! - **glyphs**: Unicode when stdout is a terminal that shows UTF-8
//!   ([`platform::terminal_is_utf8`]: always on Windows, the locale elsewhere); otherwise ASCII
//!   (`✓ ✕ ○` → `+ x -`, `├─ └─` → `|- \-`, `·` → `|`, `…` → `...`), and anything else outside
//!   ASCII becomes `?`. A file or pipe always gets ASCII: Windows PowerShell 5.1 reads a native
//!   program's output with the OEM code page;
//! - **width**: the terminal's, capped at the TUI's page width (80), or 80 when redirected.
//!
//! [`report_view`]: super::report_view
//! [`board_view`]: super::board_view

use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::{
    PAGE_WIDTH, SHUTDOWN_WAIT, UNKNOWN_CHIP, board_title, board_title_short, board_view, fits,
    green, red, report_view, rule, safe, spread,
};
use crate::platform::{self, Platform};
use crate::report::json::to_json_line;
use crate::report::{HardReset, Report};
use crate::runner::worker::{Event, Job, Worker, silence_shielded_panics};
use crate::runner::{self, Board, Grouping, Identified};
use crate::transport::{self, Mac, PortClass, PortDesc, classify, trusted_serial};

/// Exit code of a run with no failed test.
pub const EXIT_PASS: u8 = 0;
/// Exit code when no run happened (spec §1): stdout is empty.
pub const EXIT_NO_RUN: u8 = 2;

/// How long the loop waits for an event before looking at the Ctrl+C flag again.
const POLL: Duration = Duration::from_millis(50);

/// How long a cancelled job may take to end (the call in flight runs to its own timeout, about
/// 10 s for a connect, then the hard reset) before giving up on it.
const CANCEL_WAIT: Duration = Duration::from_secs(30);

/// Text width when stdout is not a terminal, and the widest the text is laid out.
const TEXT_WIDTH: u16 = PAGE_WIDTH;

/// Narrowest width the text is laid out for, whatever the terminal says.
const MIN_WIDTH: u16 = 40;

/// Shown by `--help`.
const HELP: &str = "\
devkit-tester - test an ESP32 dev board through its USB port

Usage:
  devkit-tester                          interactive terminal UI
  devkit-tester --no-tui [--port X]      plain-text report on stdout
  devkit-tester --json [--port X]        one-line JSON report on stdout
  devkit-tester --help | --version

Options:
  --no-tui     no terminal UI: progress on stderr, the report on stdout
  --json       the report as JSON (schema v1); implies --no-tui
  --port X     test only this port: its name, its USB serial number, or the
               board MAC (the serial number of the native USB port)
  -h, --help   show this help and exit
  -V, --version
               show the version and exit

Without --port, the bridge ports are identified (each board is reset into its
ROM and back to its app) and the test runs if exactly one board is found.

Exit codes:
  0  every test passed or was skipped
  1  at least one test failed
  2  nothing was run: no board, several boards without --port, bad argument,
     unsupported environment (WSL1) or cancelled with Ctrl+C
";

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// The terminal UI.
    Tui,
    /// One run without the TUI.
    Run(RunOptions),
    /// Print the usage and exit 0.
    Help,
    /// Print the version and exit 0.
    Version,
}

/// The options of a run without the TUI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOptions {
    /// The report as JSON instead of text.
    pub json: bool,
    /// Test only the port matching this name, USB serial number or MAC.
    pub port: Option<String>,
}

/// A command line that cannot be run; the text is for stderr (exit code 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageError(pub String);

impl std::fmt::Display for UsageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UsageError {}

/// Parses the arguments after the program name. `--json` implies `--no-tui`; `--port` needs
/// one of them (the TUI picks its board on screen). An argument is quoted back sanitized.
pub fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Command, UsageError> {
    let mut no_tui = false;
    let mut options = RunOptions::default();
    let mut help = false;
    let mut version = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let Some(arg) = arg.to_str().map(str::to_owned) else {
            return Err(UsageError("an argument is not valid text".to_owned()));
        };
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag.to_owned(), Some(value)),
            _ => (arg.clone(), None),
        };
        match flag.as_str() {
            "--no-tui" if inline.is_none() => no_tui = true,
            "--json" if inline.is_none() => options.json = true,
            "-h" | "--help" if inline.is_none() => help = true,
            "-V" | "--version" if inline.is_none() => version = true,
            "--port" => {
                if options.port.is_some() {
                    return Err(UsageError("--port given twice".to_owned()));
                }
                let value = match inline {
                    Some(value) => Some(value.to_owned()),
                    None => args
                        .next()
                        .and_then(|value| value.to_str().map(str::to_owned)),
                };
                match value.filter(|value| !value.trim().is_empty()) {
                    Some(value) => options.port = Some(value),
                    None => {
                        return Err(UsageError(
                            "--port needs a port name, USB serial number or MAC".to_owned(),
                        ));
                    }
                }
            }
            _ => {
                return Err(UsageError(format!(
                    "unknown argument '{}'",
                    safe(&arg).as_str()
                )));
            }
        }
    }
    if help {
        return Ok(Command::Help);
    }
    if version {
        return Ok(Command::Version);
    }
    if options.json || no_tui {
        return Ok(Command::Run(options));
    }
    if options.port.is_some() {
        return Err(UsageError(
            "--port needs --no-tui or --json (the TUI picks the board on screen)".to_owned(),
        ));
    }
    Ok(Command::Tui)
}

/// The whole program: parses `args` (without the program name), refuses an unsupported
/// platform before any port is listed, then runs the TUI or one run without it. Returns the
/// exit code; stdout is flushed before it returns.
pub fn main(args: impl IntoIterator<Item = OsString>) -> u8 {
    let mut stderr = io::stderr();
    let command = match parse_args(args) {
        Ok(command) => command,
        Err(error) => {
            let _ = writeln!(stderr, "devkit-tester: {error}");
            let _ = writeln!(stderr, "Try 'devkit-tester --help'.");
            return EXIT_NO_RUN;
        }
    };
    match command {
        Command::Help => return print_stdout(HELP),
        Command::Version => {
            return print_stdout(&format!("devkit-tester {}\n", env!("CARGO_PKG_VERSION")));
        }
        Command::Tui | Command::Run(_) => {}
    }
    launch(
        &command,
        platform::detect(),
        &mut stderr,
        &mut |command| match command {
            Command::Run(options) => run_system(options),
            Command::Tui | Command::Help | Command::Version => tui_system(),
        },
    )
}

/// Writes `text` to stdout and flushes it: 0, or 2 if stdout cannot be written.
fn print_stdout(text: &str) -> u8 {
    let mut stdout = io::stdout().lock();
    match stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Ok(()) => EXIT_PASS,
        Err(_) => EXIT_NO_RUN,
    }
}

/// [`platform::startup_check`], then `start` (the TUI or the run). `start` is never called on a
/// refused platform, so no port is listed nor opened there.
fn launch(
    command: &Command,
    platform: Platform,
    stderr: &mut dyn Write,
    start: &mut dyn FnMut(&Command) -> u8,
) -> u8 {
    if let Err(message) = platform::startup_check(platform) {
        let _ = writeln!(stderr, "devkit-tester: {message}");
        return EXIT_NO_RUN;
    }
    start(command)
}

/// The TUI, with an error that stops it reported on stderr (exit code 2: no report).
fn tui_system() -> u8 {
    match super::run() {
        Ok(()) => EXIT_PASS,
        Err(error) => {
            let _ = writeln!(io::stderr(), "devkit-tester: {error}");
            EXIT_NO_RUN
        }
    }
}

/// One run with the real ports, the real runner and the Ctrl+C handler.
fn run_system(options: &RunOptions) -> u8 {
    let mut stderr = io::stderr();
    let ports = match transport::enumerate() {
        Ok(ports) => ports,
        Err(error) => {
            let _ = writeln!(stderr, "devkit-tester: {error}");
            return EXIT_NO_RUN;
        }
    };
    let interrupted = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&interrupted);
    if let Err(error) = ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst)) {
        let _ = writeln!(
            stderr,
            "devkit-tester: Ctrl+C will not reset the board ({error})"
        );
    }
    let (events_tx, events) = mpsc::channel();
    let worker = match Worker::spawn_system(events_tx) {
        Ok(worker) => worker,
        Err(error) => {
            let _ = writeln!(stderr, "devkit-tester: could not start the runner: {error}");
            return EXIT_NO_RUN;
        }
    };
    silence_shielded_panics();

    let style = TextStyle::detect(&io::stdout());
    let session = Session {
        ports,
        worker: &worker,
        events: &events,
        interrupted: &interrupted,
        stderr_unicode: io::stderr().is_terminal() && platform::terminal_is_utf8(),
        prepare_colors: style.color,
    };
    let mut stdout = io::stdout();
    let code = execute(options, &session, style, &mut stdout, &mut stderr);
    worker.shutdown(SHUTDOWN_WAIT);
    code
}

/// What one run without the TUI works with; the tests build it with fake probes.
pub(crate) struct Session<'a> {
    /// The ports enumerated at start (the runner enumerates again while a port re-enumerates).
    pub(crate) ports: Vec<PortDesc>,
    /// The runner's thread.
    pub(crate) worker: &'a Worker,
    /// Its events.
    pub(crate) events: &'a Receiver<Event>,
    /// Raised by Ctrl+C.
    pub(crate) interrupted: &'a AtomicBool,
    /// Whether stderr may show `…` and `—` (else `...` and `-`).
    pub(crate) stderr_unicode: bool,
    /// Turn on the console's ANSI handling right before a coloured text report is written
    /// (crossterm does it on Windows by writing a colour reset to stdout, so it must not happen
    /// on a run that ends with code 2 and an empty stdout).
    pub(crate) prepare_colors: bool,
}

/// Why waiting on the runner stopped without the event asked for.
enum Stop {
    /// The runner's thread is gone.
    Gone,
    /// Cancelled, and the job did not end within [`CANCEL_WAIT`].
    NoAnswer,
}

impl Session<'_> {
    fn interrupted(&self) -> bool {
        self.interrupted.load(Ordering::SeqCst)
    }

    fn ellipsis(&self) -> &'static str {
        if self.stderr_unicode { "…" } else { "..." }
    }

    fn dash(&self) -> &'static str {
        if self.stderr_unicode { "—" } else { "-" }
    }

    /// Receives events until `done` returns a value. A Ctrl+C cancels the job in hand (once
    /// told, the job keeps being cancelled; [`Worker::cancel`] is idempotent) and the wait then
    /// lasts at most [`CANCEL_WAIT`].
    fn wait<T>(
        &self,
        stderr: &mut dyn Write,
        mut done: impl FnMut(Event) -> Option<T>,
    ) -> Result<T, Stop> {
        let mut deadline: Option<Instant> = None;
        loop {
            if self.interrupted() {
                self.worker.cancel();
                if deadline.is_none() {
                    deadline = Some(Instant::now() + CANCEL_WAIT);
                    let _ = writeln!(
                        stderr,
                        "cancelling{} the board still gets its reset back to its app",
                        self.ellipsis()
                    );
                }
            }
            match self.events.recv_timeout(POLL) {
                Ok(event) => {
                    if let Some(value) = done(event) {
                        return Ok(value);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                        return Err(Stop::NoAnswer);
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return Err(Stop::Gone),
            }
        }
    }

    /// Identifies `ports` on the runner's thread, one event per port. `None` when it stopped
    /// (Ctrl+C, or the runner is gone; already reported on stderr).
    fn identify(&self, ports: Vec<PortDesc>, stderr: &mut dyn Write) -> Option<Vec<Identified>> {
        if !self.worker.submit(Job::Identify(ports)) {
            let _ = writeln!(stderr, "devkit-tester: the runner thread is gone");
            return None;
        }
        let mut found = Vec::new();
        let waited = self.wait(stderr, |event| match event {
            Event::Identified(result) => {
                found.push(result);
                None
            }
            Event::IdentifyFinished => Some(()),
            _ => None,
        });
        if !self.stopped(waited, stderr) {
            return None;
        }
        let _ = write!(stderr, "{}", failure_lines(&found));
        Some(found)
    }

    /// Reports why a wait ended without its event, or a Ctrl+C that came while waiting; true
    /// when the program may go on.
    fn stopped(&self, waited: Result<(), Stop>, stderr: &mut dyn Write) -> bool {
        match waited {
            Ok(()) if !self.interrupted() => true,
            Ok(()) => {
                let _ = writeln!(stderr, "cancelled");
                false
            }
            Err(stop) => {
                self.report_stop(&stop, stderr);
                false
            }
        }
    }

    fn report_stop(&self, stop: &Stop, stderr: &mut dyn Write) {
        let _ = match stop {
            Stop::Gone => writeln!(stderr, "devkit-tester: the runner thread stopped"),
            Stop::NoAnswer => writeln!(
                stderr,
                "cancelled: the board did not answer in {} s and may still be in its ROM \
                 (press its RESET/EN button)",
                CANCEL_WAIT.as_secs()
            ),
        };
    }
}

/// One run without the TUI: picks the board, runs it, prints the report. Returns the exit code.
pub(crate) fn execute(
    options: &RunOptions,
    session: &Session,
    style: TextStyle,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u8 {
    let board = match &options.port {
        Some(wanted) => board_of_port(session, wanted, stderr),
        None => only_board(session, stderr),
    };
    let Some(board) = board else {
        return EXIT_NO_RUN;
    };
    if session.interrupted() {
        let _ = writeln!(stderr, "cancelled");
        return EXIT_NO_RUN;
    }

    let _ = writeln!(stderr, "{}", testing_message(&board, session.ellipsis()));
    if !session.worker.submit(Job::Run(board.clone())) {
        let _ = writeln!(stderr, "devkit-tester: the runner thread is gone");
        return EXIT_NO_RUN;
    }
    let ended = session.wait(stderr, |event| match event {
        Event::RunFinished(report) => Some(Ok(report)),
        Event::RunCancelled(reset) => Some(Err(reset)),
        _ => None,
    });
    let report = match ended {
        Ok(Ok(report)) if !session.interrupted() => report,
        // A Ctrl+C that came as the run ended still cancels: no document (spec §1).
        Ok(Ok(report)) => {
            report_cancelled(session, report.hard_reset(), stderr);
            return EXIT_NO_RUN;
        }
        Ok(Err(reset)) => {
            report_cancelled(session, &reset, stderr);
            return EXIT_NO_RUN;
        }
        Err(stop) => {
            session.report_stop(&stop, stderr);
            return EXIT_NO_RUN;
        }
    };

    let document = if options.json {
        match to_json_line(&report) {
            Ok(line) => line,
            Err(error) => {
                let _ = writeln!(stderr, "devkit-tester: internal error: {error}");
                return EXIT_NO_RUN;
            }
        }
    } else {
        if session.prepare_colors && style.color {
            super::prepare_ansi_colors();
        }
        text_report(&report, &board, style)
    };
    // One write after the run: the consumer never sees half a document.
    if let Err(error) = stdout
        .write_all(document.as_bytes())
        .and_then(|()| stdout.flush())
    {
        let _ = writeln!(stderr, "devkit-tester: could not write the report: {error}");
        return EXIT_NO_RUN;
    }
    report.exit_code()
}

fn report_cancelled(session: &Session, reset: &HardReset, stderr: &mut dyn Write) {
    let _ = match reset.reason() {
        None => writeln!(stderr, "cancelled: board back to its app"),
        Some(reason) => writeln!(
            stderr,
            "cancelled: board not back to its app {} {reason}",
            session.dash()
        ),
    };
}

/// `ESP32-C6 e8:f6:0a:f7:ff:d4 (COM13 + COM11)`; without the chip name when it is not known.
fn board_label(board: &Board, separator: &str) -> String {
    let ports: Vec<String> = board
        .ports()
        .map(|port| safe(&port.name).to_string())
        .collect();
    let ports = ports.join(separator);
    match board.chip() {
        Some(chip) => format!("{} {} ({ports})", chip.caps().name, board.mac()),
        None => format!("{} ({ports})", board.mac()),
    }
}

/// The MAC in the USB serial number of a native port, if it is one.
pub(super) fn native_mac(port: &PortDesc) -> Option<Mac> {
    if classify(port) != PortClass::Native {
        return None;
    }
    trusted_serial(port).and_then(|serial| Mac::parse(serial).ok())
}

/// The one port `wanted` names: by port name (any ASCII case), by USB serial number (any case),
/// or by MAC against the serial number of a native port (any case or separator). `Err` is the
/// message for stderr: no match, several matches, or a port that is never tested.
pub(crate) fn select_port(ports: &[PortDesc], wanted: &str) -> Result<PortDesc, String> {
    let wanted = wanted.trim();
    let mac = Mac::parse(wanted).ok();
    let matches: Vec<&PortDesc> = ports
        .iter()
        .filter(|port| {
            port.name.eq_ignore_ascii_case(wanted)
                || trusted_serial(port).is_some_and(|serial| serial.eq_ignore_ascii_case(wanted))
                || mac.is_some_and(|mac| native_mac(port) == Some(mac))
        })
        .collect();
    let shown = safe(wanted);
    match matches.as_slice() {
        [] => {
            let mut message = format!(
                "no serial port matches '{shown}' (a port name, USB serial number or board MAC)"
            );
            let candidates: Vec<String> = ports
                .iter()
                .filter(|port| classify(port).is_probe_candidate())
                .map(|port| {
                    // Same columns as the board list: name, what it is, and how to select it.
                    let (model, kind) = match classify(port) {
                        PortClass::Native => ("USB-Serial/JTAG", "native"),
                        class => (class.bridge_model().unwrap_or("?"), "bridge"),
                    };
                    let mut line = format!(
                        "\n  {:<6} {:<16} {:<7}",
                        safe(&port.name).as_str(),
                        model,
                        kind
                    );
                    match (native_mac(port), trusted_serial(port)) {
                        (Some(mac), _) => line.push_str(&format!("  --port {mac}")),
                        (None, Some(serial)) => {
                            line.push_str(&format!("  --port {}", safe(serial)));
                        }
                        (None, None) => line.push_str(&format!("  --port {}", safe(&port.name))),
                    }
                    line.trim_end().to_owned()
                })
                .collect();
            if !candidates.is_empty() {
                message.push_str("; ESP32 ports now:");
                message.extend(candidates);
            }
            Err(message)
        }
        [port] => match classify(port).not_tested_reason() {
            None => Ok((*port).clone()),
            Some(reason) => Err(format!("{} is never tested: {reason}", safe(&port.name))),
        },
        several => {
            let names: Vec<String> = several
                .iter()
                .map(|port| safe(&port.name).to_string())
                .collect();
            Err(format!(
                "'{shown}' matches several ports ({}); use the port name",
                names.join(", ")
            ))
        }
    }
}

/// `--port`: the board of that one port, identified first unless it is a native port whose
/// serial number is its MAC. `None` (code 2) when it cannot be run; the reason is on stderr.
fn board_of_port(session: &Session, wanted: &str, stderr: &mut dyn Write) -> Option<Board> {
    let port = match select_port(&session.ports, wanted) {
        Ok(port) => port,
        Err(message) => {
            let _ = writeln!(stderr, "devkit-tester: {message}");
            return None;
        }
    };
    if let Some(mac) = native_mac(&port) {
        return Board::new(mac, None, vec![port]);
    }
    let _ = writeln!(
        stderr,
        "{}",
        identifying_port_message(&port, session.ellipsis())
    );
    let found = session.identify(vec![port.clone()], stderr)?;
    match board_of_identified(&port, wanted, found.first()) {
        Ok(board) => Some(board),
        Err(message) => {
            let _ = write!(stderr, "{message}");
            None
        }
    }
}

/// `identifying COM13; the board is reset back to its app…` (`--port` on a bridge).
pub(super) fn identifying_port_message(port: &PortDesc, ellipsis: &str) -> String {
    format!(
        "identifying {}; the board is reset back to its app{ellipsis}",
        safe(&port.name)
    )
}

/// `--port` on a bridge, after its identification: the board of that one port, or the stderr
/// text (each line ending in `\n`) of why nothing runs. The reason of a failed identification
/// was already printed by [`failure_lines`].
pub(super) fn board_of_identified(
    port: &PortDesc,
    wanted: &str,
    found: Option<&Identified>,
) -> Result<Board, String> {
    match found {
        Some(Identified::Found { mac, chip, .. }) => Board::new(*mac, *chip, vec![port.clone()])
            .ok_or_else(|| format!("devkit-tester: {} cannot be tested\n", safe(wanted))),
        Some(Identified::Failed { .. }) | None => Err(format!(
            "devkit-tester: could not identify {}; nothing was tested\n",
            safe(&port.name)
        )),
    }
}

/// `  COM7: identification failed: timeout`, one line per failed identification.
pub(super) fn failure_lines(found: &[Identified]) -> String {
    found
        .iter()
        .filter_map(|result| match result {
            Identified::Failed { port, reason } => {
                Some(format!("  {}: {}\n", safe(&port.name), safe(reason)))
            }
            Identified::Found { .. } => None,
        })
        .collect()
}

/// `testing ESP32-C6 e8:f6:0a:f7:ff:d4 (COM13 + COM11)…`
pub(super) fn testing_message(board: &Board, ellipsis: &str) -> String {
    format!("testing {}{ellipsis}", board_label(board, " + "))
}

/// No `--port`: the only board connected, after identifying the bridges. `None` (code 2) with
/// no board or several (listed on stderr with the `--port` value of each).
fn only_board(session: &Session, stderr: &mut dyn Write) -> Option<Board> {
    let grouping = runner::pre_group(&session.ports);
    let mut identified = Vec::new();
    if !grouping.unidentified.is_empty() {
        let _ = writeln!(
            stderr,
            "{}",
            identifying_message(&grouping.unidentified, session.ellipsis())
        );
        identified = session.identify(grouping.unidentified.clone(), stderr)?;
    }
    match pick_only_board(&grouping, &identified) {
        Ok(board) => Some(board),
        Err(message) => {
            let _ = write!(stderr, "{message}");
            None
        }
    }
}

/// `identifying 2 bridge ports; each board is reset back to its app…` (no `--port`).
pub(super) fn identifying_message(unidentified: &[PortDesc], ellipsis: &str) -> String {
    let count = unidentified.len();
    let bridges = unidentified
        .iter()
        .all(|port| matches!(classify(port), PortClass::Bridge { .. }));
    let noun = match (bridges, count) {
        (true, 1) => "bridge port",
        (true, _) => "bridge ports",
        (false, 1) => "port",
        (false, _) => "ports",
    };
    format!("identifying {count} {noun}; each board is reset back to its app{ellipsis}")
}

/// No `--port`, after identification: the only board, or the stderr text (each line ending in
/// `\n`) for none or several (code 2).
pub(super) fn pick_only_board(
    grouping: &Grouping,
    identified: &[Identified],
) -> Result<Board, String> {
    let boards = runner::merge(grouping, identified);
    match boards.as_slice() {
        [] => {
            let mut text = "devkit-tester: no ESP32 board found\n".to_owned();
            for port in &grouping.others {
                let reason = classify(port).not_tested_reason().unwrap_or_default();
                text.push_str(&format!("  {} not tested: {reason}\n", safe(&port.name)));
            }
            Err(text)
        }
        [board] => Ok(board.clone()),
        several => Err(board_list(several)),
    }
}

/// The `--port` value that picks `board` alone: the MAC when a native port carries it in its
/// serial number, else the primary port's USB serial number, else its name.
fn port_argument(board: &Board) -> String {
    if board
        .ports()
        .any(|port| native_mac(port) == Some(board.mac()))
    {
        return board.mac().to_string();
    }
    let primary = board.primary();
    match trusted_serial(primary) {
        Some(serial) if !serial.trim().is_empty() => safe(serial).to_string(),
        _ => safe(&primary.name).to_string(),
    }
}

/// The ambiguous-selection message (approved mockup, issue #24):
/// ```text
/// 2 boards found; choose one with --port:
///   ESP32-C6  e8:f6:0a:f7:ff:d4   --port e8:f6:0a:f7:ff:d4   (COM13, COM11)
/// ```
fn board_list(boards: &[Board]) -> String {
    let rows: Vec<(String, String, String, String)> = boards
        .iter()
        .map(|board| {
            let chip = board
                .chip()
                .map_or(UNKNOWN_CHIP, |chip| chip.caps().name)
                .to_owned();
            let ports: Vec<String> = board
                .ports()
                .map(|port| safe(&port.name).to_string())
                .collect();
            (
                chip,
                board.mac().to_string(),
                format!("--port {}", port_argument(board)),
                ports.join(", "),
            )
        })
        .collect();
    let chip_width = rows.iter().map(|row| row.0.len()).max().unwrap_or(0);
    let arg_width = rows.iter().map(|row| row.2.len()).max().unwrap_or(0);
    let mut text = format!("{} boards found; choose one with --port:\n", boards.len());
    for (chip, mac, arg, ports) in rows {
        text.push_str(&format!(
            "  {chip:<chip_width$}  {mac}   {arg:<arg_width$}   ({ports})\n"
        ));
    }
    text
}

// ---- the text ----

/// How the text is printed: colours, glyphs and width (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextStyle {
    /// ANSI colours and bold.
    pub(crate) color: bool,
    /// Unicode glyphs; ASCII when false.
    pub(crate) unicode: bool,
    /// Columns to lay the text out for.
    pub(crate) width: u16,
}

impl TextStyle {
    /// What a file or a pipe gets: no colour, ASCII, 80 columns.
    #[cfg(test)]
    pub(crate) fn plain() -> TextStyle {
        TextStyle {
            color: false,
            unicode: false,
            width: TEXT_WIDTH,
        }
    }

    /// The style for `stdout` as it is now.
    fn detect(stdout: &io::Stdout) -> TextStyle {
        let terminal = stdout.is_terminal();
        let columns = if terminal {
            ratatui::crossterm::terminal::size()
                .ok()
                .map(|(columns, _)| columns)
        } else {
            None
        };
        TextStyle::decide(
            terminal,
            std::env::var_os("NO_COLOR"),
            platform::terminal_is_utf8(),
            columns,
        )
    }

    /// The rule of the module docs, pure: colour only on a terminal without a non-empty
    /// `NO_COLOR`; Unicode only on a terminal that shows UTF-8; the terminal's width within
    /// [`MIN_WIDTH`]..=[`TEXT_WIDTH`], else [`TEXT_WIDTH`].
    pub(crate) fn decide(
        terminal: bool,
        no_color: Option<OsString>,
        utf8: bool,
        columns: Option<u16>,
    ) -> TextStyle {
        let no_color = no_color.is_some_and(|value| !value.is_empty());
        TextStyle {
            color: terminal && !no_color,
            unicode: terminal && utf8,
            width: columns
                .filter(|_| terminal)
                .map_or(TEXT_WIDTH, |columns| columns.clamp(MIN_WIDTH, TEXT_WIDTH)),
        }
    }
}

/// The lines of the text report (approved mockup, issue #24): the header of the `Done` screen,
/// the report body, the compact Board card and the summary, between rules.
pub(super) fn report_lines(report: &Report, board: &Board, width: u16) -> Vec<Line<'static>> {
    let chip = report.board().chip.as_ref().map(|info| info.chip);
    let verdict = if report.exit_code() == EXIT_PASS {
        Span::styled("PASS", green().add_modifier(Modifier::BOLD))
    } else {
        Span::styled("FAIL", red().add_modifier(Modifier::BOLD))
    };
    let right = vec![verdict];
    let title = board_title(board, chip);
    let header = if fits(&title, &right, width) {
        spread(title, right, width)
    } else {
        spread(board_title_short(board, chip), right, width)
    };
    let mut lines = vec![header, rule(width)];
    lines.extend(report_view::done_lines(report, width));
    lines.push(rule(width));
    lines.extend(board_view::compact_lines(report, width));
    lines.push(rule(width));
    lines.extend(report_view::summary_lines(report, width));
    lines
}

/// The text report as printed: one string, every line ending in `\n`.
pub(crate) fn text_report(report: &Report, board: &Board, style: TextStyle) -> String {
    report_lines(report, board, style.width)
        .iter()
        .map(|line| {
            let mut text = render_line(line, style);
            text.push('\n');
            text
        })
        .collect()
}

/// The ASCII stand-in of a glyph the layout uses; `None` keeps the character.
fn ascii_glyph(c: char) -> Option<&'static str> {
    Some(match c {
        '✓' => "+",
        '✕' => "x",
        '○' => "-",
        '─' => "-",
        '├' => "|",
        '└' => "\\",
        '·' => "|",
        '×' => "x",
        '—' => "-",
        '…' => "...",
        '▸' => ">",
        '●' => "*",
        '↑' => "^",
        '↓' => "v",
        ' '..='~' => return None,
        _ => "?",
    })
}

/// One [`Line`] as a string: its spans' text (with ASCII glyphs unless `style.unicode`) and, with
/// `style.color`, each span's colour and bold as an ANSI SGR sequence closed by a reset. Trailing
/// blanks are dropped.
///
/// An ASCII `...` is three columns where `…` was one: the two extra columns are taken from the
/// blanks right after it (a shortened value is always followed by its gap), so the line keeps
/// its width.
fn render_line(line: &Line, style: TextStyle) -> String {
    let mut segments: Vec<(Style, String)> = Vec::with_capacity(line.spans.len());
    let mut debt = 0usize;
    for span in &line.spans {
        let mut text = String::with_capacity(span.content.len());
        for c in span.content.chars() {
            if style.unicode {
                text.push(c);
                continue;
            }
            if c == ' ' && debt > 0 {
                debt -= 1;
                continue;
            }
            match ascii_glyph(c) {
                Some(glyph) => {
                    text.push_str(glyph);
                    debt += glyph.chars().count().saturating_sub(1);
                }
                None => text.push(c),
            }
        }
        segments.push((line.style.patch(span.style), text));
    }
    while let Some((_, text)) = segments.last_mut() {
        let kept = text.trim_end().len();
        if kept == 0 {
            segments.pop();
        } else {
            text.truncate(kept);
            break;
        }
    }
    let mut out = String::new();
    for (span_style, text) in segments {
        let sgr = if style.color { sgr(span_style) } else { None };
        match sgr {
            Some(sgr) if !text.trim().is_empty() => {
                out.push_str(&sgr);
                out.push_str(&text);
                out.push_str("\u{1b}[0m");
            }
            _ => out.push_str(&text),
        }
    }
    out
}

/// The ANSI SGR sequence of a style (bold and the foreground colours the layout uses); `None`
/// for the default style.
fn sgr(style: Style) -> Option<String> {
    let mut codes: Vec<&str> = Vec::new();
    if style.add_modifier.contains(Modifier::BOLD) {
        codes.push("1");
    }
    let color = match style.fg {
        Some(Color::Red) => Some("31"),
        Some(Color::Green) => Some("32"),
        Some(Color::Yellow) => Some("33"),
        Some(Color::Blue) => Some("34"),
        Some(Color::Magenta) => Some("35"),
        Some(Color::Cyan) => Some("36"),
        Some(Color::Gray) => Some("37"),
        Some(Color::DarkGray) => Some("90"),
        _ => None,
    };
    codes.extend(color);
    (!codes.is_empty()).then(|| format!("\u{1b}[{}m", codes.join(";")))
}

#[cfg(test)]
mod tests;
