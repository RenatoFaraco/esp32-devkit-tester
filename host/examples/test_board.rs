//! Bench tool: lists the boards behind the serial ports, or runs the v0.1 test catalog on one
//! board (all its ports) and prints a Jest-style report.
//!
//! ```powershell
//! cargo run --manifest-path host/Cargo.toml --example test_board                      # list
//! cargo run --manifest-path host/Cargo.toml --example test_board -- e8:f6:0a:f7:ff:d4  # by MAC
//! cargo run --manifest-path host/Cargo.toml --example test_board -- COM13              # by port
//! ```
//!
//! Provisional: the TUI (#22) and `--no-tui` (#24) replace it. Read-only (ADR-009). Running it is
//! the user's explicit action (ADR-008 §4), so every USB-UART bridge is opened once to read the
//! MAC and group it with its native port (each one suffers a reset back to its application).
//! Unknown, non-USB and JTAG-channel ports are listed and never opened.
//!
//! Every text from the board, the OS or the command line is printed through `SafeText`
//! (printable ASCII only). Colors only when stdout is a terminal and `NO_COLOR` is not set;
//! Unicode glyphs only on a terminal (ASCII `+ x - * |- \-` otherwise). Exit code:
//! `report.exit_code()` (0 or 1) after a run, 0 after a list, 2 for bad usage, a board not found
//! or no port list.

use std::io::IsTerminal;
use std::process::ExitCode;

use esp32_devkit_tester::probe::{EspflashProbe, Probe};
use esp32_devkit_tester::report::{Outcome, Report, SafeText, State};
use esp32_devkit_tester::runner::{self, Board, Identified, SystemClock};
use esp32_devkit_tester::transport::{self, Mac, PortClass, PortDesc};
use esp32_devkit_tester::ui;

const USAGE: &str = "usage: test_board [--list | <MAC> | <port>]   \
                     (e.g. e8:f6:0a:f7:ff:d4, COM13, /dev/ttyACM0)";

/// Width of the port-name column (`COM13` + padding); longer names widen it.
const PORT_COLUMN: usize = 6;
/// Width of the bridge-model column; `USB-Serial/JTAG` overflows it, as in the mockup.
const MODEL_COLUMN: usize = 12;
/// Width of the chip-name column in the board list.
const CHIP_COLUMN: usize = 10;
/// Width of the test-name column in the report.
const TEST_COLUMN: usize = 20;
/// Minimum width of the value column in the report.
const VALUE_COLUMN: usize = 26;

enum Target {
    Mac(Mac),
    Port(String),
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let target = match args.as_slice() {
        [] => None,
        [flag] if flag == "--list" => None,
        [flag] if flag == "-h" || flag == "--help" => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        [arg] if !arg.starts_with('-') => Some(match Mac::parse(arg) {
            Ok(mac) => Target::Mac(mac),
            Err(_) => Target::Port(arg.clone()),
        }),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let style = Style::detect();

    let ports = match transport::enumerate() {
        Ok(ports) => ports,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let grouping = runner::pre_group(&ports);
    if !grouping.unidentified.is_empty() {
        eprintln!(
            "identifying {} port(s); each one is reset back to its app...",
            grouping.unidentified.len()
        );
    }
    let mut probe = |_: &PortDesc| -> Box<dyn Probe> { Box::new(EspflashProbe::new()) };
    let identified = runner::identify(&mut probe, &grouping.unidentified);
    for result in &identified {
        if let Identified::Found { port, reset, .. } = result
            && let Some(reason) = reset.reason()
        {
            eprintln!(
                "warning: {} was not reset back to its app: {reason}",
                safe(&port.name)
            );
        }
    }
    let boards = runner::merge(&grouping, &identified);

    let Some(target) = target else {
        print_boards(&style, &boards, &identified, &grouping.others);
        return ExitCode::SUCCESS;
    };
    let Some(board) = boards.iter().find(|board| target.matches(board)) else {
        let what = match &target {
            Target::Mac(mac) => mac.to_string(),
            Target::Port(name) => safe(name).to_string(),
        };
        eprintln!("no identified board with {what}; boards found:");
        for board in &boards {
            let names: Vec<String> = board
                .ports()
                .map(|port| safe(&port.name).to_string())
                .collect();
            eprintln!("  {}  {}", board.mac(), names.join(" + "));
        }
        return ExitCode::from(2);
    };

    let clock = SystemClock::new();
    let mut enumerate = || transport::enumerate().unwrap_or_default();
    let report = runner::run_board(
        &mut probe,
        board,
        &clock,
        &clock,
        &mut enumerate,
        &mut |_| {},
    );
    print_report(&style, board, &report);
    ExitCode::from(report.exit_code())
}

impl Target {
    fn matches(&self, board: &Board) -> bool {
        match self {
            Target::Mac(mac) => board.mac() == *mac,
            Target::Port(name) => {
                board.ports().any(|port| port.name == *name)
                    || board
                        .ports()
                        .any(|port| port.name.eq_ignore_ascii_case(name))
            }
        }
    }
}

/// OS, board or user text, sanitized before printing.
fn safe(text: &str) -> SafeText {
    SafeText::from_board(text)
}

/// How to draw: ANSI colors and Unicode glyphs, or plain ASCII.
struct Style {
    color: bool,
    unicode: bool,
}

impl Style {
    fn detect() -> Style {
        let terminal = std::io::stdout().is_terminal();
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
        let color = terminal && !no_color && ui::prepare_ansi_colors();
        Style {
            color,
            unicode: terminal,
        }
    }

    fn paint(&self, sgr: &str, text: &str) -> String {
        if self.color {
            format!("\u{1b}[{sgr}m{text}\u{1b}[0m")
        } else {
            text.to_owned()
        }
    }

    fn bold(&self, text: &str) -> String {
        self.paint("1", text)
    }

    fn green(&self, text: &str) -> String {
        self.paint("32", text)
    }

    fn red(&self, text: &str) -> String {
        self.paint("31", text)
    }

    fn yellow(&self, text: &str) -> String {
        self.paint("33", text)
    }

    fn cyan(&self, text: &str) -> String {
        self.paint("36", text)
    }

    fn gray(&self, text: &str) -> String {
        self.paint("90", text)
    }

    fn glyph(&self, unicode: &'static str, ascii: &'static str) -> &'static str {
        if self.unicode { unicode } else { ascii }
    }

    fn pass(&self) -> String {
        self.green(self.glyph("✓", "+"))
    }

    fn fail(&self) -> String {
        self.red(self.glyph("✕", "x"))
    }

    fn skip(&self) -> String {
        self.yellow(self.glyph("○", "-"))
    }

    fn dot(&self) -> &'static str {
        self.glyph("·", "-")
    }

    fn branch(&self, last: bool) -> String {
        let glyph = if last {
            self.glyph("└─", "\\-")
        } else {
            self.glyph("├─", "|-")
        };
        self.gray(glyph)
    }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// The board list: one entry per board with its ports as a tree, then the ports that are not in
/// any board, then the counts.
fn print_boards(style: &Style, boards: &[Board], identified: &[Identified], others: &[PortDesc]) {
    let failed: Vec<(&PortDesc, &str)> = identified
        .iter()
        .filter_map(|result| match result {
            Identified::Failed { port, reason } => Some((port, reason.as_str())),
            Identified::Found { .. } => None,
        })
        .collect();
    let width = boards
        .iter()
        .flat_map(Board::ports)
        .chain(failed.iter().map(|(port, _)| *port))
        .chain(others)
        .map(|port| safe(&port.name).as_str().len())
        .fold(PORT_COLUMN, usize::max);

    println!(" {} {} boards", style.bold("devkit-tester"), style.dot());
    println!();
    for board in boards {
        let chip = board.chip().map_or("unknown chip", |chip| chip.caps().name);
        println!(
            " {} {} {}",
            style.cyan(style.glyph("●", "*")),
            style.bold(&format!("{chip:<CHIP_COLUMN$}")),
            board.mac()
        );
        let count = 1 + board.others().len();
        for (index, port) in board.ports().enumerate() {
            let class = transport::classify(port);
            let (model, kind) = match class {
                PortClass::Native => ("USB-Serial/JTAG", "native"),
                _ => (class.bridge_model().unwrap_or("bridge"), "bridge"),
            };
            let tail = if index == 0 {
                format!("{kind:<8} {}", style.bold("primary"))
            } else {
                kind.to_owned()
            };
            println!(
                "     {} {:<width$} {model:<MODEL_COLUMN$} {tail}",
                style.branch(index + 1 == count),
                safe(&port.name).as_str(),
            );
        }
    }

    if !failed.is_empty() {
        println!(
            " {} {}",
            style.skip(),
            style.bold("unidentified (not tested)")
        );
        for (index, (port, reason)) in failed.iter().enumerate() {
            let model = transport::classify(port).bridge_model().unwrap_or("");
            println!(
                "     {} {:<width$} {model:<MODEL_COLUMN$} {}",
                style.branch(index + 1 == failed.len()),
                safe(&port.name).as_str(),
                style.gray(safe(reason).as_str()),
            );
        }
    }
    if !others.is_empty() {
        println!(
            " {} {}",
            style.gray(style.glyph("○", "-")),
            style.bold("other ports (never opened)")
        );
        for (index, port) in others.iter().enumerate() {
            println!(
                "     {} {:<width$} {}",
                style.branch(index + 1 == others.len()),
                safe(&port.name).as_str(),
                style.gray(&other_label(transport::classify(port))),
            );
        }
    }

    let ports: usize = boards.iter().map(|board| 1 + board.others().len()).sum();
    let dot = style.dot();
    let mut summary = format!(
        " {} {dot} {} {dot} {} unidentified",
        plural(boards.len(), "board", "boards"),
        plural(ports, "port", "ports"),
        failed.len()
    );
    if !others.is_empty() {
        summary.push_str(&format!(" {dot} {} other", others.len()));
    }
    println!();
    println!("{summary}");
}

/// Why a port is never opened, from its class (host text).
fn other_label(class: PortClass) -> String {
    match class {
        PortClass::NotUsb => "not USB".to_owned(),
        PortClass::Unknown { vid, pid } => format!("unknown USB {vid:04X}:{pid:04X}"),
        PortClass::EspressifApp { pid } => {
            format!("Espressif USB 303A:{pid:04X} from the app firmware (USB OTG)")
        }
        PortClass::JtagInterface { model } => format!("{model} JTAG channel"),
        PortClass::Bridge { model, .. } => format!("{model} channel unknown"),
        PortClass::Native => "native".to_owned(),
    }
}

/// The report of one board: header, the tests grouped by suite, the summary and the reset.
fn print_report(style: &Style, board: &Board, report: &Report) {
    let info = report.board();
    let chip = info
        .chip
        .as_ref()
        .map_or("unknown chip", |chip| chip.chip.caps().name);
    let dot = style.dot();
    let mut header = format!(" {}", style.bold(chip));
    if let Some(revision) = report
        .tests()
        .iter()
        .find(|test| test.id == runner::REVISION && test.outcome.state() == State::Pass)
        .and_then(|test| test.outcome.read())
    {
        header.push_str(&format!("  rev {revision}"));
    }
    if let Some(mac) = &info.mac {
        header.push_str(&format!("  {dot}  {mac}"));
    }
    let names: Vec<String> = board
        .ports()
        .map(|port| safe(&port.name).to_string())
        .collect();
    header.push_str(&format!("  {dot}  {}", names.join(" + ")));
    println!("{header}");
    println!();

    let width = report
        .tests()
        .iter()
        .map(|test| value_of(&test.outcome).len() + 2)
        .fold(VALUE_COLUMN, usize::max);
    let mut suite = None;
    for test in report.tests() {
        if suite != Some(test.id.suite) {
            suite = Some(test.id.suite);
            println!("  {}", style.bold(test.id.suite.as_str()));
        }
        let outcome = &test.outcome;
        let glyph = match outcome.state() {
            State::Pass => style.pass(),
            State::Fail => style.fail(),
            State::Skip => style.skip(),
        };
        let line = match outcome.state() {
            State::Skip => format!("{:<TEST_COLUMN$} {}", test.id.name, value_of(outcome)),
            State::Pass | State::Fail => format!(
                "{:<TEST_COLUMN$} {:<width$}{:>6} ms",
                test.id.name,
                value_of(outcome),
                outcome.duration_ms()
            ),
        };
        println!("    {glyph} {}", line.trim_end());
        print_details(style, outcome);
    }

    println!();
    let summary = report.summary();
    let mut counts = Vec::new();
    if summary.failed > 0 {
        counts.push(style.red(&format!("{} failed", summary.failed)));
    }
    if summary.passed > 0 {
        counts.push(style.green(&format!("{} passed", summary.passed)));
    }
    if summary.skipped > 0 {
        counts.push(style.yellow(&format!("{} skipped", summary.skipped)));
    }
    counts.push(format!("{} total", summary.total));
    let reset = match report.hard_reset().reason() {
        None => format!("board back to its app {}", style.pass()),
        Some(reason) => format!(
            "{} {} {}",
            style.red("board not back to its app"),
            style.fail(),
            style.gray(&format!("({reason})"))
        ),
    };
    println!(
        " {} {}   {dot}   {reset}",
        style.bold("Tests:"),
        counts.join(", ")
    );
}

/// The value column: what was read (or, failing that, what was expected), plus `!` when the
/// outcome carries a finding.
fn value_of(outcome: &Outcome) -> String {
    let value = outcome
        .read()
        .or(outcome.expected())
        .map_or("", SafeText::as_str);
    match outcome.finding() {
        Some(_) if value.is_empty() => "!".to_owned(),
        Some(_) => format!("{value} !"),
        None => value.to_owned(),
    }
}

/// The lines under a test: the reason in gray, `expected` × `read` on a FAIL, and the finding.
fn print_details(style: &Style, outcome: &Outcome) {
    const INDENT: &str = "      ";
    if let Some(reason) = outcome.reason() {
        println!("{INDENT}{}", style.gray(reason.as_str()));
    }
    if outcome.state() == State::Fail && (outcome.expected().is_some() || outcome.read().is_some())
    {
        let text = |value: Option<&SafeText>| value.map_or("-", SafeText::as_str).to_owned();
        println!(
            "{INDENT}{} {} {} {} {}",
            style.gray("expected"),
            text(outcome.expected()),
            style.gray(style.glyph("×", "vs")),
            style.gray("read"),
            text(outcome.read())
        );
    }
    if let Some(finding) = outcome.finding() {
        println!(
            "{INDENT}{}",
            style.yellow(&format!(
                "! {} ({}): table {}, read {}",
                finding.field(),
                finding.kind(),
                finding.table(),
                finding.read()
            ))
        );
        println!(
            "{INDENT}  {}",
            style.gray(&format!("source: {}", finding.source()))
        );
    }
}
