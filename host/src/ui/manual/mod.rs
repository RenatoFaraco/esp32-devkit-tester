//! The user manual's images, redrawn from recorded sessions by the tool's own drawing code
//! (issue #64). Used only by `examples/record_session.rs` and `examples/manual_screenshots.rs`;
//! the tester itself never calls it.
//!
//! 1. [`capture`] records a real run on the bench as a [`Session`] (`session.rs`).
//! 2. [`images`] replays a session into the TUI's [`App`] with the same [`Event`]s and keys the
//!    runner and the user would send, draws each screen with [`draw`](super::draw) on a
//!    `TestBackend` 80 columns wide, and renders the `--no-tui`/`--json` transcript with the
//!    same lines and messages as [`cli`](super::cli). Each one becomes an SVG ([`svg`]).
//!
//! # Images of a session `<name>`
//! | file                  | when                          | shows                                   |
//! |-----------------------|-------------------------------|-----------------------------------------|
//! | `<name>_confirm.svg`  | no `--port`, bridges to ask   | the list with the "Identify bridges" box |
//! | `<name>_list.svg`     | no `--port`                   | the list after the identification       |
//! | `<name>_running.svg`  | no `--port`, a report         | mid-run (see below)                     |
//! | `<name>_report.svg`   | no `--port`, a report         | `Done`, Report tab                      |
//! | `<name>_board.svg`    | no `--port`, a report         | `Done`, Board tab                       |
//! | `<name>_text.svg`     | a report, or no board to run  | `devkit-tester --no-tui`: stderr, stdout and `$LASTEXITCODE` |
//! | `<name>_json.svg`     | `--port` and a report         | `devkit-tester --json --port X`, the line wrapped as a terminal does |
//!
//! The running screen is the replay of the report's tests in the runner's order (primary port,
//! the board suites, then the other ports), stopped where a user would have looked: at the wait
//! for the native port to come back (`waiting for COM11…`) when the board has one, else on the
//! first `fail` (in flight), else on the last suite test (in flight).
//!
//! A screen is as tall as its content (no scrolling), at most [`MAX_ROWS`] rows; a longer one
//! keeps the app's own `↓ N more` footer, a longer transcript ends in a `…` line. The
//! confirmation box is the exception: it is drawn on the 80×24 terminal the app assumes, since
//! it is centred over the list.

mod session;
pub mod svg;

pub use session::{
    Bench, Capture, Captured, IdentifiedRecord, PortRecord, ResetRecord, SESSION_FORMAT, Session,
    SessionError, UsbRecord, capture, iso8601_utc,
};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use super::cli::{
    board_of_identified, failure_lines, identifying_message, identifying_port_message, native_mac,
    pick_only_board, report_lines, select_port, testing_message,
};
use super::{Action, App, Areas, DEFAULT_SIZE, PAGE_WIDTH, draw, gray};
use crate::report::json::to_json_line;
use crate::report::{Report, State, Suite};
use crate::runner::worker::Event;
use crate::runner::{self, Board, Identified, UART_BRIDGE, USB_SERIAL_JTAG};
use crate::transport::{Mac, PortClass, PortDesc, classify};

/// The screen names, as in the file names `<session>_<screen>.svg`.
pub const SCREENS: [&str; 7] = [
    "confirm", "list", "running", "report", "board", "text", "json",
];

/// Width of every image, in columns (the TUI's page).
pub const COLUMNS: u16 = PAGE_WIDTH;

/// Tallest image, in rows.
pub const MAX_ROWS: u16 = 40;

/// Shortest TUI screen, in rows (header, rule, 4 rows of body, rule, footer).
const MIN_ROWS: u16 = 8;

/// Ellipsis on stderr: a Windows terminal shows UTF-8 (`platform::terminal_is_utf8`).
const ELLIPSIS: &str = "…";

/// The PowerShell prompt of the transcripts.
const PROMPT: &str = "PS> ";

/// One image of the manual.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// `<session>_<screen>.svg`.
    pub file_name: String,
    /// The SVG document.
    pub svg: String,
}

/// Every image of session `name` (table in the module docs), in that order.
pub fn images(name: &str, session: &Session) -> Result<Vec<Image>, SessionError> {
    let ports = session.ports()?;
    let identified = session.identified()?;
    let report = session.report()?;
    let comment = format!(
        "esp32-devkit-tester {}: session {name} ({}), drawn by examples/manual_screenshots.rs",
        session.captured.host_version, session.captured.date
    );
    let mut shots: Vec<(&str, &str, Buffer)> = Vec::new();
    if session.captured.port.is_none() {
        for (screen, buffer) in tui_screens(&ports, &identified, report.as_ref())? {
            shots.push((screen, "devkit-tester", buffer));
        }
    }
    let transcript = Transcript::of(session, &ports, &identified, report.as_ref());
    if let Some(text) = &transcript {
        shots.push(("text", "devkit-tester --no-tui", text.buffer(false)));
        if session.captured.port.is_some() && report.is_some() {
            shots.push(("json", "devkit-tester --json", text.buffer(true)));
        }
    }
    Ok(shots
        .into_iter()
        .map(|(screen, title, buffer)| Image {
            file_name: format!("{name}_{screen}.svg"),
            svg: svg::to_svg(&buffer, title, &comment),
        })
        .collect())
}

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// `app` drawn by [`draw`] on a `width` × `height` test terminal.
fn render(app: &App, width: u16, height: u16) -> Result<Buffer, SessionError> {
    let mut terminal = Terminal::new(TestBackend::new(width, height))
        .map_err(|error| SessionError::Invalid(format!("test terminal: {error}")))?;
    terminal
        .draw(|frame| draw(frame, app))
        .map_err(|error| SessionError::Invalid(format!("test terminal: {error}")))?;
    Ok(terminal.backend().buffer().clone())
}

/// Rows a screen needs to show its content without scrolling, within
/// [`MIN_ROWS`]..=[`MAX_ROWS`].
fn rows_for(app: &App) -> Result<u16, SessionError> {
    let needed = match app.report_body(COLUMNS) {
        Some((body, summary)) => {
            let summary = if summary.is_empty() {
                0
            } else {
                1 + summary.len()
            };
            4 + body.len() + summary
        }
        None => {
            // The list is top-aligned: draw it tall and count up to its last written row.
            let tall = render(app, COLUMNS, MAX_ROWS)?;
            let body = Areas::of(Rect::new(0, 0, COLUMNS, MAX_ROWS), 0).body;
            let last = (body.top()..body.bottom())
                .filter(|&row| {
                    (0..COLUMNS).any(|column| {
                        tall.cell((column, row))
                            .is_some_and(|cell| !cell.symbol().trim().is_empty())
                    })
                })
                .max()
                .unwrap_or(body.top());
            usize::from(last - body.top() + 1) + 4
        }
    };
    let needed = u16::try_from(needed).unwrap_or(MAX_ROWS);
    Ok(needed.clamp(MIN_ROWS, MAX_ROWS))
}

/// The current screen of `app`, sized to its content.
fn shot(app: &mut App) -> Result<Buffer, SessionError> {
    let rows = rows_for(app)?;
    app.resize(COLUMNS, rows);
    render(app, COLUMNS, rows)
}

/// The TUI screens of a session without `--port`: the identification, then the run of the
/// board in the report, through `on_key`/`on_event` as on the bench.
fn tui_screens(
    ports: &[PortDesc],
    identified: &[Identified],
    report: Option<&Report>,
) -> Result<Vec<(&'static str, Buffer)>, SessionError> {
    let mut shots = Vec::new();
    let mut app = App::new();
    app.on_event(Event::Boards(runner::pre_group(ports)));
    if !app.never_asked().is_empty() {
        app.on_key(press(KeyCode::Enter));
        // The box is centred over the list: drawn on the classic terminal the app assumes,
        // as a user first sees it (a screen cut to the list would hide the list under it).
        let (columns, rows) = DEFAULT_SIZE;
        app.resize(columns, rows);
        shots.push(("confirm", render(&app, columns, rows)?));
        if let Action::Identify(_) = app.on_key(press(KeyCode::Char('y'))) {
            for result in identified {
                app.on_event(Event::Identified(result.clone()));
            }
            app.on_event(Event::IdentifyFinished);
        }
    }
    shots.push(("list", shot(&mut app)?));

    let Some(report) = report else {
        return Ok(shots);
    };
    let Some(mac) = report
        .board()
        .mac
        .as_ref()
        .and_then(|mac| Mac::parse(mac.as_str()).ok())
    else {
        return Ok(shots);
    };
    if !app.boards().iter().any(|board| board.mac() == mac) {
        return Ok(shots);
    }
    app.selected = Some(mac);
    let Action::Run(board) = app.on_key(press(KeyCode::Enter)) else {
        return Ok(shots);
    };
    for event in running_events(&board, report) {
        app.on_event(event);
    }
    shots.push(("running", shot(&mut app)?));
    app.on_event(Event::RunFinished(report.clone()));
    shots.push(("report", shot(&mut app)?));
    app.on_key(press(KeyCode::Tab));
    shots.push(("board", shot(&mut app)?));
    Ok(shots)
}

/// The events of the run up to the moment the running screen shows (module docs).
fn running_events(board: &Board, report: &Report) -> Vec<Event> {
    let primary = match classify(board.primary()) {
        PortClass::Native => USB_SERIAL_JTAG,
        _ => UART_BRIDGE,
    };
    let tests = report.tests();
    let order = tests
        .iter()
        .filter(|test| test.id == primary)
        .chain(tests.iter().filter(|test| test.id.suite != Suite::Port))
        .chain(
            tests
                .iter()
                .filter(|test| test.id.suite == Suite::Port && test.id != primary),
        );
    let native = board
        .others()
        .iter()
        .find(|port| classify(port) == PortClass::Native);
    let last_suite_test = tests
        .iter()
        .filter(|test| test.id.suite != Suite::Port && test.outcome.state() != State::Skip)
        .map(|test| test.id)
        .next_back();

    let mut events = Vec::new();
    for test in order {
        if let Some(port) = native
            && test.id == USB_SERIAL_JTAG
        {
            events.push(Event::TestStarted(test.id));
            events.push(Event::Waiting { port: port.clone() });
            break;
        }
        let in_flight = test.outcome.state() == State::Fail
            || (native.is_none() && Some(test.id) == last_suite_test);
        if test.outcome.state() != State::Skip {
            events.push(Event::TestStarted(test.id));
        }
        if in_flight {
            break;
        }
        events.push(Event::TestFinished(test.clone()));
    }
    events
}

/// What a terminal shows for `devkit-tester --no-tui` (or `--json`) with the session's options.
struct Transcript {
    /// The command after the prompt, without `--no-tui`/`--json`.
    arguments: String,
    /// stderr, in order (the `cli` messages).
    stderr: Vec<String>,
    /// The report and its board, when one ran.
    run: Option<(Report, Board)>,
    /// `$LASTEXITCODE`.
    exit_code: u8,
}

impl Transcript {
    /// `None` when the run was not asked for and a board was found (nothing to show).
    fn of(
        session: &Session,
        ports: &[PortDesc],
        identified: &[Identified],
        report: Option<&Report>,
    ) -> Option<Transcript> {
        let mut stderr = String::new();
        let picked = match &session.captured.port {
            Some(wanted) => match select_port(ports, wanted) {
                Err(message) => Err(format!("devkit-tester: {message}\n")),
                Ok(port) => match native_mac(&port) {
                    Some(mac) => Board::new(mac, None, vec![port.clone()])
                        .ok_or_else(|| format!("devkit-tester: {wanted} cannot be tested\n")),
                    None => {
                        stderr.push_str(&identifying_port_message(&port, ELLIPSIS));
                        stderr.push('\n');
                        stderr.push_str(&failure_lines(identified));
                        board_of_identified(&port, wanted, identified.first())
                    }
                },
            },
            None => {
                let grouping = runner::pre_group(ports);
                if !grouping.unidentified.is_empty() {
                    stderr.push_str(&identifying_message(&grouping.unidentified, ELLIPSIS));
                    stderr.push('\n');
                    stderr.push_str(&failure_lines(identified));
                }
                pick_only_board(&grouping, identified)
            }
        };
        let (run, exit_code) = match picked {
            Err(message) => {
                stderr.push_str(&message);
                (None, 2)
            }
            Ok(board) => {
                let report = report?;
                stderr.push_str(&testing_message(&board, ELLIPSIS));
                stderr.push('\n');
                (Some((report.clone(), board)), report.exit_code())
            }
        };
        let arguments = session
            .captured
            .port
            .as_ref()
            .map(|port| format!(" --port {}", super::safe(port)))
            .unwrap_or_default();
        Some(Transcript {
            arguments,
            stderr: stderr.lines().map(str::to_owned).collect(),
            run,
            exit_code,
        })
    }

    /// The terminal: the prompt and command, stderr, the report (text, or the JSON line), and
    /// `$LASTEXITCODE`.
    fn buffer(&self, json: bool) -> Buffer {
        let prompt = Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD);
        let command = |text: String| {
            Line::from(vec![
                Span::styled(PROMPT, prompt),
                Span::styled(text, prompt),
            ])
        };
        let mode = if json { "--json" } else { "--no-tui" };
        let mut lines = vec![command(format!("devkit-tester {mode}{}", self.arguments))];
        for line in &self.stderr {
            lines.extend(hard_wrap(line).into_iter().map(Line::raw));
        }
        if let Some((report, board)) = &self.run {
            if json {
                let line = to_json_line(report).unwrap_or_default();
                lines.extend(hard_wrap(line.trim_end()).into_iter().map(Line::raw));
            } else {
                lines.extend(report_lines(report, board, COLUMNS));
            }
        }
        lines.push(command("$LASTEXITCODE".to_owned()));
        lines.push(Line::raw(self.exit_code.to_string()));

        let max = usize::from(MAX_ROWS);
        if lines.len() > max {
            lines.truncate(max - 1);
            lines.push(Line::styled("…", gray()));
        }
        let rows = u16::try_from(lines.len()).unwrap_or(MAX_ROWS);
        let area = Rect::new(0, 0, COLUMNS, rows);
        let mut buffer = Buffer::empty(area);
        Paragraph::new(lines).render(area, &mut buffer);
        buffer
    }
}

/// `text` cut every [`COLUMNS`] characters, as a terminal wraps a long line.
fn hard_wrap(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    chars
        .chunks(usize::from(COLUMNS))
        .map(|chunk| chunk.iter().collect())
        .collect()
}

#[cfg(test)]
mod tests;
