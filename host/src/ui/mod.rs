//! Terminal UI (ratatui).
//!
//! The only layer allowed to import `ratatui`/`crossterm`. It never talks to a board: the
//! runner's threads ([`runner::watch`], [`runner::worker`]) do, and send [`Event`]s.
//!
//! # State machine (pure, tested without a terminal)
//! [`App`] holds the screen and the session's knowledge (ports, identified bridges, chips seen
//! in runs). [`App::on_key`] and [`App::on_event`] only change that state; what must happen
//! outside comes back as an [`Action`] for the loop to carry out. Screens:
//! `List` → `ConfirmIdentify` → `Identifying` → `List` → `Running` → `Done` → `List`.
//! - `List`: the boards found by enumeration, live (hot-plug never opens a port, ADR-008 §3).
//!   `enter` tests the selected board (the only one needs no picking); if there are bridges
//!   nobody was asked about yet, it asks first (ADR-008 §4). `i` asks again for every bridge
//!   that is still not identified.
//! - `Running`: `q`/`Esc`/`Ctrl+C` ask to cancel (the board still gets its hard reset); a
//!   second `Ctrl+C` while cancelling quits at once.
//! - `Done`: `enter` back to the list, `q` quits.
//!
//! # Loop
//! [`run`] starts the threads, then `crossterm::event::poll` with a short timeout, drains the
//! event channel with `try_recv` and redraws from state: the UI never waits on a board.

use std::io;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{
    self as term, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::chips::Chip;
use crate::report::{Outcome, Report, SafeText, State, TestId, TestResult};
use crate::runner::watch::Watcher;
use crate::runner::worker::{Event, Job, Worker, silence_shielded_panics};
use crate::runner::{self, Board, Grouping, Identified};
use crate::transport::{self, Mac, PortClass, PortDesc};

/// Shown in the middle of the list when no port of interest is connected.
pub const NO_BOARDS_MESSAGE: &str = "No boards detected — plug a board in";

/// How long the loop waits for a key before looking at the event channel again.
const POLL: Duration = Duration::from_millis(50);

/// One spinner frame per this much time.
const SPINNER_STEP: Duration = Duration::from_millis(80);

/// How long quitting waits for the background threads before leaving them to end on their own.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

/// Braille spinner, one frame per [`App::tick`].
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Widest the page is drawn; wider terminals keep it on the left (the columns stay together).
const PAGE_WIDTH: u16 = 80;

/// Width of the port-name column (`COM13` + padding); longer names widen it.
const PORT_COLUMN: usize = 6;
/// Width of the bridge-model column (`USB-Serial/JTAG` + padding).
const MODEL_COLUMN: usize = 16;
/// Width of the port-kind column.
const KIND_COLUMN: usize = 8;
/// Width of the chip-name column in the board list.
const CHIP_COLUMN: usize = 10;
/// Width of the test-name column in the report.
const TEST_COLUMN: usize = 20;

/// Chip name shown for a board whose chip is not known yet (native port only, never run).
const UNKNOWN_CHIP: &str = "ESP32-??";

/// What the loop must do after a key press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Leave the application.
    Quit,
    /// Open these bridges once to read their MAC (the user confirmed).
    Identify(Vec<PortDesc>),
    /// Test this board.
    Run(Board),
    /// Ask the job in hand to stop.
    Cancel,
    /// Nothing to do.
    None,
}

/// UI state. The screen is drawn from this, never directly from incoming bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    grouping: Grouping,
    /// Identification results of this session, kept while their port stays enumerated.
    identified: Vec<Identified>,
    /// Bridges the user chose not to identify; `enter` does not ask about them again.
    declined: Vec<PortDesc>,
    /// Chips learned from finished runs (a native-only board shows its chip after one run).
    chips: Vec<(Mac, Chip)>,
    selected: Option<Mac>,
    screen: Screen,
    tick: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Screen {
    List,
    ConfirmIdentify {
        ports: Vec<PortDesc>,
    },
    Identifying {
        ports: Vec<PortDesc>,
        done: Vec<PortDesc>,
        cancelling: bool,
    },
    Running(Live),
    Done {
        board: Board,
        report: Report,
    },
}

/// A run in progress, as far as the events told.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Live {
    board: Board,
    plan: Vec<TestId>,
    results: Vec<TestResult>,
    current: Option<TestId>,
    waiting: Option<PortDesc>,
    cancelling: bool,
}

impl Live {
    fn new(board: Board) -> Live {
        let plan = runner::catalog(board.chip().map(Chip::caps), board.has_bridge());
        Live {
            board,
            plan,
            results: Vec::new(),
            current: None,
            waiting: None,
            cancelling: false,
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// The start screen, with no port known yet (the hot-plug thread sends the first list).
    pub fn new() -> Self {
        Self {
            grouping: Grouping::default(),
            identified: Vec::new(),
            declined: Vec::new(),
            chips: Vec::new(),
            selected: None,
            screen: Screen::List,
            tick: 0,
        }
    }

    /// Advances the spinner by one frame.
    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    /// The boards as known now: groups from the enumeration joined with the bridges identified
    /// in this session (still enumerated), with the chips learned from runs.
    pub fn boards(&self) -> Vec<Board> {
        let present: Vec<Identified> = self
            .identified
            .iter()
            .filter(|result| self.grouping.unidentified.contains(result.port()))
            .cloned()
            .collect();
        runner::merge(&self.grouping, &present)
            .into_iter()
            .map(|board| match (board.chip(), self.known_chip(board.mac())) {
                (None, Some(chip)) => {
                    let ports = board.ports().cloned().collect();
                    Board::new(board.mac(), Some(chip), ports).unwrap_or(board)
                }
                _ => board,
            })
            .collect()
    }

    fn known_chip(&self, mac: Mac) -> Option<Chip> {
        self.chips
            .iter()
            .find(|(known, _)| *known == mac)
            .map(|(_, chip)| *chip)
    }

    /// Ports still waiting for identification, with the reason of a failed attempt.
    fn not_identified(&self) -> Vec<(&PortDesc, Option<&str>)> {
        self.grouping
            .unidentified
            .iter()
            .filter_map(
                |port| match self.identified.iter().find(|result| result.port() == port) {
                    Some(Identified::Found { .. }) => None,
                    Some(Identified::Failed { reason, .. }) => Some((port, Some(reason.as_str()))),
                    None => Some((port, None)),
                },
            )
            .collect()
    }

    /// Bridges `enter` asks about: not identified, never tried and never declined.
    fn never_asked(&self) -> Vec<PortDesc> {
        self.not_identified()
            .into_iter()
            .filter(|(port, failed)| failed.is_none() && !self.declined.contains(port))
            .map(|(port, _)| port.clone())
            .collect()
    }

    fn selected_index(&self, boards: &[Board]) -> Option<usize> {
        if boards.is_empty() {
            return None;
        }
        let found = self
            .selected
            .and_then(|mac| boards.iter().position(|board| board.mac() == mac));
        Some(found.unwrap_or(0))
    }

    /// Maps a key press to an action and updates the state. Pure: no I/O.
    ///
    /// Only `KeyEventKind::Press` counts: the Windows console also reports `Release`
    /// (and `Repeat`), which would otherwise trigger every action twice.
    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        if key.kind != KeyEventKind::Press {
            return Action::None;
        }
        let ctrl_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
        let leave = ctrl_c || matches!(key.code, KeyCode::Char('q') | KeyCode::Esc);
        match &mut self.screen {
            Screen::List if leave => Action::Quit,
            Screen::List => self.list_key(key.code),
            Screen::ConfirmIdentify { .. } if ctrl_c => Action::Quit,
            Screen::ConfirmIdentify { ports } => match key.code {
                KeyCode::Char('y' | 'Y') => {
                    let ports = std::mem::take(ports);
                    self.screen = Screen::Identifying {
                        ports: ports.clone(),
                        done: Vec::new(),
                        cancelling: false,
                    };
                    Action::Identify(ports)
                }
                KeyCode::Char('n' | 'N' | 'q') | KeyCode::Esc => {
                    let ports = std::mem::take(ports);
                    for port in ports {
                        if !self.declined.contains(&port) {
                            self.declined.push(port);
                        }
                    }
                    self.screen = Screen::List;
                    Action::None
                }
                _ => Action::None,
            },
            Screen::Identifying { cancelling, .. } => cancel_key(cancelling, leave, ctrl_c),
            Screen::Running(live) => cancel_key(&mut live.cancelling, leave, ctrl_c),
            Screen::Done { .. } if ctrl_c || key.code == KeyCode::Char('q') => Action::Quit,
            Screen::Done { .. } => {
                if matches!(key.code, KeyCode::Enter | KeyCode::Esc) {
                    self.screen = Screen::List;
                }
                Action::None
            }
        }
    }

    fn list_key(&mut self, code: KeyCode) -> Action {
        let boards = self.boards();
        match code {
            KeyCode::Up | KeyCode::Char('k') | KeyCode::Down | KeyCode::Char('j') => {
                if let Some(index) = self.selected_index(&boards) {
                    let up = matches!(code, KeyCode::Up | KeyCode::Char('k'));
                    let next = if up {
                        index.saturating_sub(1)
                    } else {
                        (index + 1).min(boards.len() - 1)
                    };
                    self.selected = boards.get(next).map(Board::mac);
                }
                Action::None
            }
            KeyCode::Enter => {
                let ask = self.never_asked();
                if !ask.is_empty() {
                    self.screen = Screen::ConfirmIdentify { ports: ask };
                    return Action::None;
                }
                let Some(board) = self
                    .selected_index(&boards)
                    .and_then(|index| boards.get(index))
                else {
                    return Action::None;
                };
                self.selected = Some(board.mac());
                self.screen = Screen::Running(Live::new(board.clone()));
                Action::Run(board.clone())
            }
            KeyCode::Char('i') => {
                let ports: Vec<PortDesc> = self
                    .not_identified()
                    .into_iter()
                    .map(|(port, _)| port.clone())
                    .collect();
                if !ports.is_empty() {
                    self.screen = Screen::ConfirmIdentify { ports };
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Updates the state with what a background thread reported. Pure: no I/O, and no port is
    /// ever opened because of an event.
    pub fn on_event(&mut self, event: Event) {
        match event {
            Event::Boards(grouping) => {
                self.identified
                    .retain(|result| grouping.unidentified.contains(result.port()));
                self.declined
                    .retain(|port| grouping.unidentified.contains(port));
                self.grouping = grouping;
            }
            Event::Identified(result) => {
                let port = result.port().clone();
                self.identified.retain(|known| known.port() != &port);
                self.identified.push(result);
                if let Screen::Identifying { done, .. } = &mut self.screen {
                    done.push(port);
                }
            }
            Event::IdentifyFinished => {
                if matches!(self.screen, Screen::Identifying { .. }) {
                    self.screen = Screen::List;
                }
            }
            Event::TestStarted(id) => {
                if let Screen::Running(live) = &mut self.screen {
                    live.current = Some(id);
                    live.waiting = None;
                }
            }
            Event::Waiting { port } => {
                if let Screen::Running(live) = &mut self.screen {
                    live.waiting = Some(port);
                }
            }
            Event::TestFinished(test) => {
                if let Screen::Running(live) = &mut self.screen {
                    if live.current == Some(test.id) {
                        live.current = None;
                        live.waiting = None;
                    }
                    live.results.retain(|known| known.id != test.id);
                    live.results.push(test);
                }
            }
            Event::RunFinished(report) => {
                if let Screen::Running(live) = &self.screen {
                    let board = live.board.clone();
                    if let Some(info) = &report.board().chip {
                        self.chips.retain(|(mac, _)| *mac != board.mac());
                        self.chips.push((board.mac(), info.chip));
                    }
                    self.screen = Screen::Done { board, report };
                }
            }
            Event::RunCancelled => {
                if matches!(self.screen, Screen::Running(_)) {
                    self.screen = Screen::List;
                }
            }
        }
    }

    fn spinner(&self) -> &'static str {
        SPINNER[self.tick % SPINNER.len()]
    }
}

/// `q`/`Esc`/`Ctrl+C` while a job runs: ask to cancel; `Ctrl+C` again while cancelling quits.
fn cancel_key(cancelling: &mut bool, leave: bool, ctrl_c: bool) -> Action {
    if !leave {
        return Action::None;
    }
    if *cancelling && ctrl_c {
        return Action::Quit;
    }
    *cancelling = true;
    Action::Cancel
}

// ---- drawing ----

fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

fn gray() -> Style {
    Style::new().fg(Color::DarkGray)
}

fn green() -> Style {
    Style::new().fg(Color::Green)
}

fn red() -> Style {
    Style::new().fg(Color::Red)
}

fn yellow() -> Style {
    Style::new().fg(Color::Yellow)
}

fn cyan() -> Style {
    Style::new().fg(Color::Cyan)
}

/// OS or board text, sanitized before it reaches the terminal.
fn safe(text: &str) -> SafeText {
    SafeText::from_board(text)
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// `left` and `right` on one line of `width` cells, `right` flush against the right edge (one
/// cell of margin); at least one space between them.
fn spread<'a>(mut left: Vec<Span<'a>>, right: Vec<Span<'a>>, width: u16) -> Line<'a> {
    let used: usize = left.iter().chain(&right).map(Span::width).sum();
    let gap = usize::from(width).saturating_sub(used + 1).max(1);
    left.push(Span::raw(" ".repeat(gap)));
    left.extend(right);
    Line::from(left)
}

/// Draws the whole screen from `app`.
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let page = Rect {
        width: area.width.min(PAGE_WIDTH),
        ..area
    };
    let [header, top_rule, body, bottom_rule, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(page);

    let rule = Line::styled(
        format!(" {}", "─".repeat(usize::from(page.width.saturating_sub(2)))),
        gray(),
    );
    frame.render_widget(Paragraph::new(header_line(app, page.width)), header);
    frame.render_widget(Paragraph::new(rule.clone()), top_rule);
    frame.render_widget(Paragraph::new(rule), bottom_rule);
    frame.render_widget(Paragraph::new(footer_line(app)), footer);

    match &app.screen {
        Screen::List | Screen::Identifying { .. } => draw_list(frame, app, body),
        Screen::ConfirmIdentify { ports } => {
            draw_list(frame, app, body);
            draw_confirm(frame, ports.len(), body);
        }
        Screen::Running(live) => {
            let lines = running_lines(app, live, body.width);
            frame.render_widget(Paragraph::new(lines), body);
        }
        Screen::Done { report, .. } => {
            let lines = done_lines(report, body.width);
            frame.render_widget(Paragraph::new(lines), body);
        }
    }
}

fn header_line(app: &App, width: u16) -> Line<'static> {
    match &app.screen {
        Screen::List | Screen::ConfirmIdentify { .. } => spread(
            vec![Span::styled(" esp32-devkit-tester", bold())],
            vec![Span::styled(
                format!("v{}", env!("CARGO_PKG_VERSION")),
                gray(),
            )],
            width,
        ),
        Screen::Identifying { cancelling, .. } => spread(
            vec![Span::styled(" esp32-devkit-tester", bold())],
            vec![
                Span::styled(app.spinner(), cyan()),
                Span::raw(if *cancelling {
                    " cancelling"
                } else {
                    " identifying"
                }),
            ],
            width,
        ),
        Screen::Running(live) => {
            let status = if live.cancelling {
                " cancelling"
            } else {
                " running"
            };
            spread(
                board_title(&live.board, live.board.chip()),
                vec![Span::styled(app.spinner(), cyan()), Span::raw(status)],
                width,
            )
        }
        Screen::Done { board, report } => {
            let chip = report.board().chip.as_ref().map(|info| info.chip);
            let verdict = if report.exit_code() == 0 {
                Span::styled("PASS", green().add_modifier(Modifier::BOLD))
            } else {
                Span::styled("FAIL", red().add_modifier(Modifier::BOLD))
            };
            spread(board_title(board, chip), vec![verdict], width)
        }
    }
}

/// ` ESP32-C6  ·  e8:f6:0a:f7:ff:d4  ·  COM13 + COM11`
fn board_title(board: &Board, chip: Option<Chip>) -> Vec<Span<'static>> {
    let name = chip.map_or(UNKNOWN_CHIP, |chip| chip.caps().name);
    let ports: Vec<String> = board
        .ports()
        .map(|port| safe(&port.name).to_string())
        .collect();
    vec![
        Span::styled(format!(" {name}"), bold()),
        Span::styled("  ·  ", gray()),
        Span::raw(board.mac().to_string()),
        Span::styled("  ·  ", gray()),
        Span::raw(ports.join(" + ")),
    ]
}

fn footer_line(app: &App) -> Line<'static> {
    let keys: Vec<(&str, &str)> = match &app.screen {
        Screen::List => {
            let mut keys = Vec::new();
            let has_boards = !app.boards().is_empty();
            if has_boards {
                keys.push(("↑↓", "select"));
            }
            if has_boards || !app.never_asked().is_empty() {
                keys.push(("enter", "test"));
            }
            if !app.not_identified().is_empty() {
                keys.push(("i", "identify bridges"));
            }
            keys.push(("q", "quit"));
            keys
        }
        Screen::ConfirmIdentify { .. } => vec![("y", "identify"), ("n", "skip")],
        Screen::Identifying { cancelling, .. } | Screen::Running(Live { cancelling, .. }) => {
            if *cancelling {
                return Line::styled(
                    " cancelling… the board still gets its reset back to its app \
                     (Ctrl+C again quits now)",
                    gray(),
                );
            }
            vec![("q", "cancel")]
        }
        Screen::Done { .. } => vec![("enter", "back to boards"), ("q", "quit")],
    };
    let mut spans = vec![Span::raw(" ")];
    for (index, (key, label)) in keys.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("   "));
        }
        spans.push(Span::styled(key, bold()));
        spans.push(Span::styled(format!(" {label}"), gray()));
    }
    Line::from(spans)
}

/// Model and kind of a port, as in the list (`CH343` / `bridge`, `USB-Serial/JTAG` / `native`).
fn port_label(port: &PortDesc) -> (&'static str, &'static str) {
    let class = transport::classify(port);
    match class {
        PortClass::Native => ("USB-Serial/JTAG", "native"),
        _ => (class.bridge_model().unwrap_or("bridge"), "bridge"),
    }
}

fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
    let boards = app.boards();
    let waiting = app.not_identified();
    if boards.is_empty() && waiting.is_empty() {
        let [_, middle, _] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(area);
        frame.render_widget(
            Paragraph::new(Line::from(NO_BOARDS_MESSAGE).centered()),
            middle,
        );
        return;
    }

    let names = boards
        .iter()
        .flat_map(Board::ports)
        .chain(waiting.iter().map(|(port, _)| *port))
        .map(|port| safe(&port.name).as_str().len());
    let name_width = names.fold(PORT_COLUMN, usize::max);
    let selected = app.selected_index(&boards);
    let mut lines: Vec<Line> = Vec::new();

    if !boards.is_empty() {
        lines.push(Line::styled(" Boards", bold()));
    }
    for (index, board) in boards.iter().enumerate() {
        let marker = if Some(index) == selected {
            " ▸ "
        } else {
            "   "
        };
        let chip = board.chip().map(|chip| chip.caps().name);
        let mut spans = vec![
            Span::styled(marker, cyan()),
            Span::styled("● ", cyan()),
            Span::styled(
                format!("{:<CHIP_COLUMN$}", chip.unwrap_or(UNKNOWN_CHIP)),
                bold(),
            ),
            Span::raw(format!(" {}", board.mac())),
        ];
        if chip.is_none() {
            spans.push(Span::styled("  unknown chip", gray()));
        }
        lines.push(Line::from(spans));
        let count = 1 + board.others().len();
        for (position, port) in board.ports().enumerate() {
            let (model, kind) = port_label(port);
            let branch = if position + 1 == count {
                "└─"
            } else {
                "├─"
            };
            let mut spans = vec![
                Span::styled(format!("       {branch} "), gray()),
                Span::raw(format!(
                    "{:<name_width$} {model:<MODEL_COLUMN$} ",
                    safe(&port.name).as_str()
                )),
            ];
            if position == 0 {
                spans.push(Span::raw(format!("{kind:<KIND_COLUMN$} ")));
                spans.push(Span::styled("primary", bold()));
            } else {
                spans.push(Span::raw(kind));
            }
            lines.push(Line::from(spans));
        }
    }

    if !waiting.is_empty() {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        lines.push(Line::styled(" Not identified yet", bold()));
        let in_flight = match &app.screen {
            Screen::Identifying { ports, done, .. } => {
                ports.iter().find(|port| !done.contains(port)).cloned()
            }
            _ => None,
        };
        for (port, failed) in &waiting {
            let (model, kind) = port_label(port);
            let glyph = if in_flight.as_ref() == Some(*port) {
                Span::styled(app.spinner(), cyan())
            } else if failed.is_some() {
                Span::styled("✕", red())
            } else {
                Span::styled("?", yellow())
            };
            let mut spans = vec![
                Span::raw("   "),
                glyph,
                Span::raw(format!(
                    " {:<name_width$} {model:<MODEL_COLUMN$} {kind}",
                    safe(&port.name).as_str()
                )),
            ];
            if let Some(reason) = failed {
                spans.push(Span::styled(format!("  {}", safe(reason)), gray()));
            }
            lines.push(Line::from(spans));
        }
    }

    let ports: usize = boards
        .iter()
        .map(|board| 1 + board.others().len())
        .sum::<usize>()
        + waiting.len();
    lines.push(Line::raw(""));
    lines.push(Line::raw(format!(
        " {} · {} · {} not identified",
        plural(boards.len(), "board", "boards"),
        plural(ports, "port", "ports"),
        waiting.len()
    )));
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_confirm(frame: &mut Frame, count: usize, area: Rect) {
    const WIDTH: u16 = 52;
    const HEIGHT: u16 = 7;
    let first = if count == 1 {
        "1 bridge port is not identified yet.".to_owned()
    } else {
        format!("{count} bridge ports are not identified yet.")
    };
    let second = if count == 1 {
        "To read its MAC the board is reset into the ROM"
    } else {
        "To read each MAC its board is reset into the ROM"
    };
    let popup = Rect {
        x: area.x + area.width.saturating_sub(WIDTH) / 2,
        y: area.y + area.height.saturating_sub(HEIGHT) / 2,
        width: WIDTH.min(area.width),
        height: HEIGHT.min(area.height),
    };
    let lines = vec![
        Line::raw(format!(" {first}")),
        Line::raw(format!(" {second}")),
        Line::raw(" and then back to its app. Nothing is written."),
        Line::raw(""),
        // As in the approved mockup: 11 columns in, a hair left of the exact centre.
        Line::from(vec![
            Span::raw("           "),
            Span::styled("y", bold()),
            Span::raw(" identify          "),
            Span::styled("n", bold()),
            Span::raw(" skip"),
        ]),
    ];
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Identify bridges ")),
        popup,
    );
}

/// The glyph of a finished test: ✓ green, ✕ red, ○ yellow.
fn glyph(state: State) -> Span<'static> {
    match state {
        State::Pass => Span::styled("✓", green()),
        State::Fail => Span::styled("✕", red()),
        State::Skip => Span::styled("○", yellow()),
    }
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

/// One finished test: glyph, name, value (or the reason, in gray, when there is no value) and
/// the duration flush right (none on a SKIP).
fn test_line(test: &TestResult, width: u16) -> Line<'static> {
    let outcome = &test.outcome;
    let mut left = vec![
        Span::raw("    "),
        glyph(outcome.state()),
        Span::raw(format!(" {:<TEST_COLUMN$} ", test.id.name)),
    ];
    let value = value_of(outcome);
    if value.is_empty() {
        if let Some(reason) = outcome.reason() {
            left.push(Span::styled(reason.to_string(), gray()));
        }
    } else {
        left.push(Span::raw(value));
    }
    match outcome.state() {
        State::Skip => Line::from(left),
        State::Pass | State::Fail => spread(
            left,
            vec![Span::raw(format!("{} ms", outcome.duration_ms()))],
            width,
        ),
    }
}

/// Suite headers and the lines under them, in catalog order; a suite appears with its first line.
fn suite_lines<'a>(tests: impl Iterator<Item = (TestId, Line<'a>)>) -> Vec<Line<'a>> {
    let mut lines = Vec::new();
    let mut suite = None;
    for (id, line) in tests {
        if suite != Some(id.suite) {
            suite = Some(id.suite);
            lines.push(Line::styled(format!("  {}", id.suite.as_str()), bold()));
        }
        lines.push(line);
    }
    lines
}

fn running_lines(app: &App, live: &Live, width: u16) -> Vec<Line<'static>> {
    let mut pending = false;
    let shown = live.plan.iter().filter_map(|id| {
        if let Some(test) = live.results.iter().find(|test| test.id == *id) {
            return Some((*id, test_line(test, width)));
        }
        if live.current == Some(*id) {
            let mut spans = vec![
                Span::raw("    "),
                Span::styled(app.spinner(), cyan()),
                Span::raw(format!(" {:<TEST_COLUMN$} ", id.name)),
            ];
            if let Some(port) = &live.waiting {
                spans.push(Span::styled(
                    format!("waiting for {} to come back…", safe(&port.name)),
                    gray(),
                ));
            }
            return Some((*id, Line::from(spans)));
        }
        pending = true;
        None
    });
    let mut lines = suite_lines(shown);
    if pending {
        lines.push(Line::styled("    …", gray()));
    }
    lines
}

fn done_lines(report: &Report, width: u16) -> Vec<Line<'static>> {
    let mut lines = suite_lines(
        report
            .tests()
            .iter()
            .map(|test| (test.id, test_line(test, width))),
    );
    lines.push(Line::raw(""));
    let summary = report.summary();
    let mut spans = vec![Span::styled(" Tests: ", bold())];
    let mut counts = Vec::new();
    if summary.failed > 0 {
        counts.push(Span::styled(format!("{} failed", summary.failed), red()));
    }
    if summary.passed > 0 {
        counts.push(Span::styled(format!("{} passed", summary.passed), green()));
    }
    if summary.skipped > 0 {
        counts.push(Span::styled(
            format!("{} skipped", summary.skipped),
            yellow(),
        ));
    }
    counts.push(Span::raw(format!("{} total", summary.total)));
    for (index, count) in counts.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(", "));
        }
        spans.push(count);
    }
    spans.push(Span::styled(" · ", gray()));
    match report.hard_reset().reason() {
        None => {
            spans.push(Span::raw("board back to its app "));
            spans.push(glyph(State::Pass));
        }
        Some(reason) => {
            spans.push(Span::styled("board not back to its app ", red()));
            spans.push(glyph(State::Fail));
            spans.push(Span::styled(format!(" ({reason})"), gray()));
        }
    }
    lines.push(Line::from(spans));
    lines
}

// ---- loop ----

/// Runs the TUI until the user quits.
///
/// Starts the hot-plug thread and the runner's thread (real board access: espflash, the system
/// clock, the OS port list), then the terminal. `ratatui::init` enables raw mode, enters the
/// alternate screen and installs a panic hook that restores the terminal; on top of it,
/// [`silence_shielded_panics`] keeps espflash panics that the probe catches from printing over
/// the screen. `ratatui::restore` runs on both the success and error paths, then the threads are
/// given [`SHUTDOWN_WAIT`] to end.
pub fn run() -> io::Result<()> {
    let (events_tx, events) = mpsc::channel();
    let watcher = Watcher::spawn_system(events_tx.clone())?;
    let worker = Worker::spawn_system(events_tx)?;
    let mut terminal = ratatui::init();
    silence_shielded_panics();
    let result = event_loop(&mut terminal, &events, &worker);
    ratatui::restore();
    worker.shutdown(SHUTDOWN_WAIT);
    watcher.stop(SHUTDOWN_WAIT);
    result
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    events: &Receiver<Event>,
    worker: &Worker,
) -> io::Result<()> {
    let mut app = App::new();
    let mut last_tick = Instant::now();
    loop {
        terminal.draw(|frame| draw(frame, &app))?;
        if term::poll(POLL)?
            && let TermEvent::Key(key) = term::read()?
        {
            let submitted = match app.on_key(key) {
                Action::Quit => return Ok(()),
                Action::Identify(ports) => worker.submit(Job::Identify(ports)),
                Action::Run(board) => worker.submit(Job::Run(board)),
                Action::Cancel => {
                    worker.cancel();
                    true
                }
                Action::None => true,
            };
            if !submitted {
                return Err(io::Error::other("the runner thread is gone"));
            }
        }
        loop {
            match events.try_recv() {
                Ok(event) => app.on_event(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return Err(io::Error::other("the background threads are gone"));
                }
            }
        }
        if worker.is_finished() {
            return Err(io::Error::other("the runner thread stopped"));
        }
        if last_tick.elapsed() >= SPINNER_STEP {
            app.tick();
            last_tick = Instant::now();
        }
    }
}

/// Gets stdout ready for ANSI colour sequences printed as plain text (bench examples now,
/// `--no-tui` later). On Windows, crossterm turns on virtual terminal processing for the console
/// the first time it styles output, so the sequences are interpreted instead of shown; elsewhere
/// this only writes a colour reset. Call it only when stdout is a terminal. Returns false if
/// stdout could not be written.
pub fn prepare_ansi_colors() -> bool {
    ratatui::crossterm::execute!(std::io::stdout(), ratatui::crossterm::style::ResetColor).is_ok()
}

#[cfg(test)]
mod tests;
