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
//! - `List`: the boards found by enumeration, live (hot-plug never opens a port, ADR-008 §3),
//!   then the bridges not identified yet and the "other" ports that are never tested (unknown
//!   devices, JTAG channels, an app's own USB on the OTG controller), each with its reason.
//!   `enter` tests the selected board (the only one needs no picking); if there are bridges
//!   nobody was asked about yet, it asks first (ADR-008 §4). `i` asks again for every bridge
//!   that is still not identified.
//! - `Running`: `q`/`Esc`/`Ctrl+C` ask to cancel (the board still gets its hard reset); a
//!   second `Ctrl+C` while cancelling quits at once.
//! - `Done`: `tab` switches between the **Report** (what passed or failed) and the **Board**
//!   panel (what board this is: the chip table merged with what was read, H7), named as tabs in
//!   the header; `enter` back to the list, `q` quits. Every run opens on Report. `tab` does
//!   nothing on `Running`: the panel is built from the finished report, and half a panel would
//!   show `—` for values that are only not read *yet*.
//!
//! On `Running` and `Done` the body scrolls (`↑↓`/`j k` a line, `PgUp`/`PgDn` a page,
//! `Home`/`End`) between the fixed header and footer; `Done` also keeps its summary fixed under
//! both tabs. Each tab of `Done` keeps its own scroll position, so going to Board and back
//! leaves the report where it was; a new run starts both at the top. The layouts live in
//! [`report_view`] and [`board_view`]. [`App::resize`] tells the state the terminal size so that
//! the keys clamp against what is actually on screen.
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
use crate::report::{Report, SafeText, TestId, TestResult};
use crate::runner::watch::Watcher;
use crate::runner::worker::{Event, Job, Worker, silence_shielded_panics};
use crate::runner::{self, Board, Grouping, Identified};
use crate::transport::{self, Mac, PortClass, PortDesc};

mod board_view;
mod report_view;

use report_view::{Scroll, ScrollState};

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

/// Terminal size assumed until the loop reports the real one (the classic 80×24).
const DEFAULT_SIZE: (u16, u16) = (80, 24);

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
    /// Scroll position of the report body (`Running`, and the Report tab of `Done`).
    scroll: ScrollState,
    /// Scroll position of the Board tab of `Done`.
    board_scroll: ScrollState,
    /// Terminal size (columns, rows) as last drawn; the scroll keys clamp against it.
    size: (u16, u16),
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
        tab: Tab,
    },
}

/// The two views of a finished run, switched with `tab`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    /// The Jest-style report.
    #[default]
    Report,
    /// The Board panel.
    Board,
}

impl Tab {
    /// The tab `tab` goes to.
    fn other(self) -> Tab {
        match self {
            Tab::Report => Tab::Board,
            Tab::Board => Tab::Report,
        }
    }

    /// The name in the header (`Report`, `Board`).
    fn name(self) -> &'static str {
        match self {
            Tab::Report => "Report",
            Tab::Board => "Board",
        }
    }

    /// The footer hint for going to this tab (`tab board`).
    fn hint(self) -> &'static str {
        match self {
            Tab::Report => "report",
            Tab::Board => "board",
        }
    }
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
            scroll: ScrollState::top(),
            board_scroll: ScrollState::top(),
            size: DEFAULT_SIZE,
        }
    }

    /// Advances the spinner by one frame.
    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    /// Records the terminal size (columns, rows); the loop calls it on every draw.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.size = (width, height);
    }

    /// The scrolling body of the current screen laid out for `width` (the report, or the Board
    /// panel on that tab of `Done`), and the fixed summary under it (`Done` only); `None` on the
    /// screens that do not show a run.
    fn report_body(&self, width: u16) -> Option<(Vec<Line<'static>>, Vec<Line<'static>>)> {
        match &self.screen {
            Screen::Running(live) => Some((
                report_view::running_lines(live, self.spinner(), width),
                Vec::new(),
            )),
            Screen::Done { board, report, tab } => {
                let body = match tab {
                    Tab::Report => report_view::done_lines(report, width),
                    Tab::Board => board_view::board_lines(report, board, width),
                };
                Some((body, report_view::summary_lines(report, width)))
            }
            _ => None,
        }
    }

    /// The scroll position of the body on screen.
    fn body_scroll(&self) -> &ScrollState {
        match &self.screen {
            Screen::Done {
                tab: Tab::Board, ..
            } => &self.board_scroll,
            _ => &self.scroll,
        }
    }

    /// Scrolls the body on screen by `step`, against the body as laid out for the last size.
    fn scroll_report(&mut self, step: Scroll) {
        let (width, height) = self.size;
        let page = Rect::new(0, 0, width.min(PAGE_WIDTH), height);
        if let Some((body, summary)) = self.report_body(page.width) {
            let viewport = usize::from(Areas::of(page, summary.len()).body.height);
            let scroll = match &self.screen {
                Screen::Done {
                    tab: Tab::Board, ..
                } => &mut self.board_scroll,
                _ => &mut self.scroll,
            };
            scroll.apply(step, body.len(), viewport);
        }
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
        if matches!(self.screen, Screen::Running(_) | Screen::Done { .. })
            && let Some(step) = scroll_step(key.code)
        {
            self.scroll_report(step);
            return Action::None;
        }
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
            Screen::Done { tab, .. } if key.code == KeyCode::Tab => {
                *tab = tab.other();
                Action::None
            }
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
                self.scroll = ScrollState::following();
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
                    self.screen = Screen::Done {
                        board,
                        report,
                        tab: Tab::Report,
                    };
                    self.scroll = ScrollState::top();
                    self.board_scroll = ScrollState::top();
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

/// The scroll step of a key on the report screens, if it is one.
fn scroll_step(code: KeyCode) -> Option<Scroll> {
    match code {
        KeyCode::Up | KeyCode::Char('k') => Some(Scroll::Up),
        KeyCode::Down | KeyCode::Char('j') => Some(Scroll::Down),
        KeyCode::PageUp => Some(Scroll::PageUp),
        KeyCode::PageDown => Some(Scroll::PageDown),
        KeyCode::Home => Some(Scroll::Home),
        KeyCode::End => Some(Scroll::End),
        _ => None,
    }
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

/// The rows of a page, top to bottom. The summary and the rule above it are zero rows high on
/// every screen but `Done`.
struct Areas {
    header: Rect,
    top_rule: Rect,
    body: Rect,
    summary_rule: Rect,
    summary: Rect,
    bottom_rule: Rect,
    footer: Rect,
}

impl Areas {
    /// Splits `page` for a fixed summary of `summary` lines. One function for drawing and for
    /// the scroll keys, so both agree on how many body lines are on screen.
    fn of(page: Rect, summary: usize) -> Areas {
        let summary = u16::try_from(summary).unwrap_or(u16::MAX);
        let summary_rule = u16::from(summary > 0);
        let [
            header,
            top_rule,
            body,
            summary_rule,
            summary,
            bottom_rule,
            footer,
        ] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(summary_rule),
            Constraint::Length(summary),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(page);
        Areas {
            header,
            top_rule,
            body,
            summary_rule,
            summary,
            bottom_rule,
            footer,
        }
    }
}

/// Draws the whole screen from `app`.
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let page = Rect {
        width: area.width.min(PAGE_WIDTH),
        ..area
    };
    let report = app.report_body(page.width);
    let summary_len = report.as_ref().map_or(0, |(_, summary)| summary.len());
    let areas = Areas::of(page, summary_len);

    let rule = Line::styled(
        format!(" {}", "─".repeat(usize::from(page.width.saturating_sub(2)))),
        gray(),
    );
    frame.render_widget(Paragraph::new(header_line(app, page.width)), areas.header);
    frame.render_widget(Paragraph::new(rule.clone()), areas.top_rule);
    frame.render_widget(Paragraph::new(rule.clone()), areas.bottom_rule);

    let mut hidden = (0, 0);
    match (&app.screen, report) {
        (_, Some((body, summary))) => {
            let viewport = usize::from(areas.body.height);
            let scroll = app.body_scroll();
            let offset = scroll.offset(body.len(), viewport);
            hidden = scroll.hidden(body.len(), viewport);
            let shown: Vec<Line> = body.into_iter().skip(offset).take(viewport).collect();
            frame.render_widget(Paragraph::new(shown), areas.body);
            if !summary.is_empty() {
                frame.render_widget(Paragraph::new(rule), areas.summary_rule);
                frame.render_widget(Paragraph::new(summary), areas.summary);
            }
        }
        (Screen::ConfirmIdentify { ports }, None) => {
            draw_list(frame, app, areas.body);
            draw_confirm(frame, ports.len(), areas.body);
        }
        (_, None) => draw_list(frame, app, areas.body),
    }
    frame.render_widget(
        Paragraph::new(footer_line(app, page.width, hidden)),
        areas.footer,
    );
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
        Screen::Done { board, report, tab } => {
            let chip = report.board().chip.as_ref().map(|info| info.chip);
            let verdict = if report.exit_code() == 0 {
                Span::styled("PASS", green().add_modifier(Modifier::BOLD))
            } else {
                Span::styled("FAIL", red().add_modifier(Modifier::BOLD))
            };
            let mut right = vec![verdict, Span::raw("   ")];
            right.extend(tab_spans(*tab));
            // Too narrow for everything: the port names go first (the Board panel lists them),
            // then the verdict (the summary under the body still says it).
            let title = board_title(board, chip);
            if fits(&title, &right, width) {
                return spread(title, right, width);
            }
            let short = board_title_short(board, chip);
            if fits(&short, &right, width) {
                return spread(short, right, width);
            }
            spread(short, tab_spans(*tab), width)
        }
    }
}

/// `[Report]  Board`: the active tab bold in brackets, the other one gray.
fn tab_spans(active: Tab) -> Vec<Span<'static>> {
    let tab = |tab: Tab| {
        if tab == active {
            Span::styled(format!("[{}]", tab.name()), bold())
        } else {
            Span::styled(tab.name(), gray())
        }
    };
    vec![tab(Tab::Report), Span::raw("  "), tab(Tab::Board)]
}

/// Whether `left` and `right` fit on one line of `width` cells as [`spread`] lays them out.
fn fits(left: &[Span], right: &[Span], width: u16) -> bool {
    let used: usize = left.iter().chain(right).map(Span::width).sum();
    used + 2 <= usize::from(width)
}

/// ` ESP32-C6  ·  e8:f6:0a:f7:ff:d4  ·  COM13 + COM11`
fn board_title(board: &Board, chip: Option<Chip>) -> Vec<Span<'static>> {
    let ports: Vec<String> = board
        .ports()
        .map(|port| safe(&port.name).to_string())
        .collect();
    let mut spans = board_title_short(board, chip);
    spans.push(Span::styled("  ·  ", gray()));
    spans.push(Span::raw(ports.join(" + ")));
    spans
}

/// ` ESP32-C6  ·  e8:f6:0a:f7:ff:d4`
fn board_title_short(board: &Board, chip: Option<Chip>) -> Vec<Span<'static>> {
    let name = chip.map_or(UNKNOWN_CHIP, |chip| chip.caps().name);
    vec![
        Span::styled(format!(" {name}"), bold()),
        Span::styled("  ·  ", gray()),
        Span::raw(board.mac().to_string()),
    ]
}

/// The key hints and, on the report screens, `↑ N`/`↓ N more` at the right end for the body
/// lines `hidden` above and below the viewport (`↑↓ scroll` is listed only then). On `Done` the
/// first hint is the tab `tab` goes to (`tab board`), then `↑↓ scroll`; when the line is too
/// narrow, `enter back to boards` becomes `enter back`.
fn footer_line(app: &App, width: u16, hidden: (usize, usize)) -> Line<'static> {
    let scrollable = hidden.0 + hidden.1 > 0;
    let more = if scrollable {
        report_view::more_indicator(hidden.0, hidden.1)
    } else {
        Vec::new()
    };
    let mut keys: Vec<(&str, &str)> = match &app.screen {
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
        Screen::Done { tab, .. } => {
            vec![
                ("tab", tab.other().hint()),
                ("enter", "back to boards"),
                ("q", "quit"),
            ]
        }
    };
    if scrollable {
        let at = usize::from(matches!(app.screen, Screen::Done { .. }));
        keys.insert(at, ("↑↓", "scroll"));
    }
    let mut spans = key_spans(&keys);
    if !fits(&spans, &more, width)
        && let Some(back) = keys.iter_mut().find(|(key, _)| *key == "enter")
    {
        back.1 = "back";
        spans = key_spans(&keys);
    }
    if !scrollable {
        return Line::from(spans);
    }
    spread(spans, more, width)
}

/// ` key label   key label`: keys bold, labels gray.
fn key_spans(keys: &[(&str, &str)]) -> Vec<Span<'static>> {
    let mut spans = vec![Span::raw(" ")];
    for (index, (key, label)) in keys.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("   "));
        }
        spans.push(Span::styled((*key).to_owned(), bold()));
        spans.push(Span::styled(format!(" {label}"), gray()));
    }
    spans
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
    let others = &app.grouping.others;
    if boards.is_empty() && waiting.is_empty() && others.is_empty() {
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
        .chain(others)
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

    if !others.is_empty() {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        lines.push(Line::styled(" Other ports (not tested)", bold()));
        for port in others {
            // Never opened: the class alone says why (host text, no board involved).
            let class = transport::classify(port);
            let ids = port
                .usb
                .as_ref()
                .map(|usb| format!("{:04X}:{:04X}", usb.vid, usb.pid))
                .unwrap_or_default();
            let reason = class.not_tested_reason().unwrap_or_default();
            lines.push(Line::from(vec![
                Span::raw("   "),
                Span::styled("·", gray()),
                Span::raw(format!(
                    " {:<name_width$} {ids:<MODEL_COLUMN$} ",
                    safe(&port.name).as_str()
                )),
                Span::styled(reason, gray()),
            ]));
        }
    }

    let ports: usize = boards
        .iter()
        .map(|board| 1 + board.others().len())
        .sum::<usize>()
        + waiting.len();
    let mut summary = format!(
        " {} · {} · {} not identified",
        plural(boards.len(), "board", "boards"),
        plural(ports, "port", "ports"),
        waiting.len()
    );
    if !others.is_empty() {
        summary.push_str(&format!(" · {} other", others.len()));
    }
    lines.push(Line::raw(""));
    lines.push(Line::raw(summary));
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
        terminal.draw(|frame| {
            let area = frame.area();
            app.resize(area.width, area.height);
            draw(frame, &app);
        })?;
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
