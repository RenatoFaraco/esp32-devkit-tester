//! Terminal UI (ratatui).
//!
//! The only layer allowed to import `ratatui`/`crossterm`. It renders from state (`App`), never
//! byte by byte; key handling is a pure function (`App::on_key`) so it can be tested without a
//! terminal.

use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};

/// Message shown before the user presses a key.
pub const IDLE_MESSAGE: &str = "Press any key to test the board";

/// Message shown after a key press while port detection does not exist yet (M0).
pub const NO_BOARDS_MESSAGE: &str = "No boards detected yet — port detection arrives in v0.1";

/// What the main loop should do after a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Leave the application.
    Quit,
    /// Start testing the connected boards.
    Start,
    /// Nothing to do.
    None,
}

/// UI state. The screen is drawn from this, never directly from incoming bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    message: String,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// Creates the initial (idle) state.
    pub fn new() -> Self {
        Self {
            message: IDLE_MESSAGE.to_owned(),
        }
    }

    /// Status message currently shown to the user.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Maps a key event to an action and updates the state. Pure: no I/O.
    ///
    /// Only `KeyEventKind::Press` counts: the Windows console also reports `Release`
    /// (and `Repeat`), which would otherwise trigger every action twice.
    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        if key.kind != KeyEventKind::Press {
            return Action::None;
        }
        let is_ctrl_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
        match key.code {
            _ if is_ctrl_c => Action::Quit,
            KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
            _ => {
                self.message = NO_BOARDS_MESSAGE.to_owned();
                Action::Start
            }
        }
    }
}

/// Draws the whole screen from `app`.
pub fn draw(frame: &mut Frame, app: &App) {
    let [header, status, boards, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    frame.render_widget(
        Paragraph::new(Line::from("esp32-devkit-tester").centered())
            .style(Style::new().add_modifier(Modifier::BOLD)),
        header,
    );
    frame.render_widget(Paragraph::new(Line::from(app.message()).centered()), status);
    frame.render_widget(Block::bordered().title(" Boards "), boards);
    frame.render_widget(
        Paragraph::new(Line::from("q quit")).style(Style::new().add_modifier(Modifier::DIM)),
        footer,
    );
}

/// Runs the TUI until the user quits.
///
/// `ratatui::init` enables raw mode, enters the alternate screen and installs a panic hook that
/// restores the terminal; `ratatui::restore` is called on both the success and error paths.
pub fn run() -> std::io::Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal) -> std::io::Result<()> {
    let mut app = App::new();
    loop {
        terminal.draw(|frame| draw(frame, &app))?;
        if let Event::Key(key) = event::read()?
            && app.on_key(key) == Action::Quit
        {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyEventState;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn release(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        }
    }

    fn screen_text(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        let width = buffer.area.width as usize;
        buffer
            .content()
            .chunks(width)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn q_quits() {
        assert_eq!(App::new().on_key(press(KeyCode::Char('q'))), Action::Quit);
    }

    #[test]
    fn esc_quits() {
        assert_eq!(App::new().on_key(press(KeyCode::Esc)), Action::Quit);
    }

    #[test]
    fn ctrl_c_quits_because_raw_mode_delivers_it_as_a_key() {
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(App::new().on_key(key), Action::Quit);
    }

    #[test]
    fn plain_c_starts_instead_of_quitting() {
        assert_eq!(App::new().on_key(press(KeyCode::Char('c'))), Action::Start);
    }

    #[test]
    fn any_other_key_starts_and_reports_no_boards_yet() {
        let mut app = App::new();
        assert_eq!(app.message(), IDLE_MESSAGE);
        assert_eq!(app.on_key(press(KeyCode::Enter)), Action::Start);
        assert_eq!(app.message(), NO_BOARDS_MESSAGE);
    }

    #[test]
    fn key_release_is_ignored_so_windows_does_not_double_fire() {
        let mut app = App::new();
        assert_eq!(app.on_key(release(KeyCode::Char('q'))), Action::None);
        assert_eq!(app.on_key(release(KeyCode::Enter)), Action::None);
        assert_eq!(app.message(), IDLE_MESSAGE);
    }

    #[test]
    fn idle_screen_shows_title_prompt_and_footer() {
        let text = screen_text(&App::new());
        assert!(text.contains("esp32-devkit-tester"));
        assert!(text.contains(IDLE_MESSAGE));
        assert!(text.contains("Boards"));
        assert!(text.contains("q quit"));
    }

    #[test]
    fn screen_shows_no_boards_message_after_start() {
        let mut app = App::new();
        app.on_key(press(KeyCode::Char(' ')));
        assert!(screen_text(&app).contains("No boards detected yet"));
    }
}
