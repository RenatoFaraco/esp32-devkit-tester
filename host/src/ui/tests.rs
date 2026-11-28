//! `App` and screen tests: no terminal (ratatui's `TestBackend`) and no board (fake probes on the
//! runner's thread, through `runner::worker::testing`).

use std::sync::mpsc;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyEventState;

use super::*;
use crate::probe::Probe;
use crate::probe::fake::{FakeProbe, Step};
use crate::report::{BoardInfo, ChipInfo, HardReset, Outcome, Revision, Run, State};
use crate::runner::worker::testing::{
    C6_MAC, Log, TestTime, c6_enumerate, c6_probes, ch343, collect_until, gate, native, usb,
};
use crate::runner::{
    BOARD_TESTS, CHIP_IDENTITY, CRYSTAL, EMBEDDED_FLASH, FAIL_MAC_EFUSE_MISMATCH, FLASH_SIZE,
    JEDEC_ID, MAC_EFUSE_REG, PACKAGE, PSRAM, REVISION, SKIP_NO_USB_SERIAL_JTAG, UART_BRIDGE,
    USB_SERIAL_JTAG,
};

const C3_MAC: &str = "80:53:e0:0a:5d:ac";

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

fn ctrl_c() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
}

fn mac(text: &str) -> Mac {
    Mac::parse(text).unwrap()
}

fn chip(target: &str) -> Chip {
    Chip::from_target(target).unwrap()
}

fn c6_native() -> PortDesc {
    native("COM11", "E8:F6:0A:F7:FF:D4")
}

fn c3_native() -> PortDesc {
    native("COM6", "80:53:E0:0A:5D:AC")
}

fn ch340k(name: &str) -> PortDesc {
    usb(name, 0x1A86, 0x7522, "6")
}

fn cp210x(name: &str) -> PortDesc {
    usb(name, 0x10C4, 0xEA60, "0001")
}

fn boards_event(ports: &[PortDesc]) -> Event {
    Event::Boards(runner::pre_group(ports))
}

fn found(port: PortDesc, mac_text: &str, target: &str) -> Event {
    Event::Identified(Identified::Found {
        port,
        mac: mac(mac_text),
        chip: Some(chip(target)),
        reset: crate::report::HardReset::done(),
    })
}

/// The bench of the mockup: C6 (CH343 + native) and C3 (CH340K + native) identified, a CP210x
/// still not identified.
fn bench_app() -> App {
    let mut app = App::new();
    app.on_event(boards_event(&[
        ch343("COM13"),
        c6_native(),
        ch340k("COM10"),
        c3_native(),
        cp210x("COM7"),
    ]));
    app.on_event(found(ch343("COM13"), C6_MAC, "esp32c6"));
    app.on_event(found(ch340k("COM10"), C3_MAC, "esp32c3"));
    app
}

/// `enter` on the bench: decline the CP210x (`n`), then `enter` again starts the selected board.
fn start_run(app: &mut App) -> Board {
    assert_eq!(app.on_key(press(KeyCode::Enter)), Action::None);
    assert_eq!(app.on_key(press(KeyCode::Char('n'))), Action::None);
    let Action::Run(board) = app.on_key(press(KeyCode::Enter)) else {
        panic!("enter did not start a run");
    };
    board
}

fn screen_lines(app: &App) -> Vec<String> {
    screen_lines_sized(app, 80, 24)
}

/// The screen drawn on a `width` × `height` terminal, one trimmed string per row.
fn screen_lines_sized(app: &App, width: u16, height: u16) -> Vec<String> {
    buffer_rows(&screen_buffer(app, width, height))
}

fn screen_buffer(app: &App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| draw(frame, app)).unwrap();
    terminal.backend().buffer().clone()
}

fn buffer_rows(buffer: &ratatui::buffer::Buffer) -> Vec<String> {
    let width = usize::from(buffer.area.width);
    buffer
        .content()
        .chunks(width)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

/// Foreground colour of the first cell of `row` from column `column` that starts `text`.
fn fg_at(buffer: &ratatui::buffer::Buffer, row: usize, text: &str) -> Color {
    let rows = buffer_rows(buffer);
    let line = &rows[row];
    let byte = line
        .find(text)
        .unwrap_or_else(|| panic!("{text:?} not in {line:?}"));
    let column = line[..byte].chars().count();
    buffer[(u16::try_from(column).unwrap(), u16::try_from(row).unwrap())].fg
}

fn screen_text(app: &App) -> String {
    screen_lines(app).join("\n")
}

/// The line that starts with `prefix` (after trimming the right end), or a readable failure.
fn line_starting<'a>(lines: &'a [String], prefix: &str) -> &'a str {
    lines
        .iter()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("no line starts with {prefix:?}:\n{}", lines.join("\n")))
}

fn names(board: &Board) -> Vec<&str> {
    board.ports().map(|port| port.name.as_str()).collect()
}

// ---- keys ----

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
    assert_eq!(App::new().on_key(ctrl_c()), Action::Quit);
}

#[test]
fn key_release_is_ignored_so_windows_does_not_double_fire() {
    let mut app = App::new();
    app.on_event(boards_event(&[c6_native()]));
    let before = app.clone();
    assert_eq!(app.on_key(release(KeyCode::Char('q'))), Action::None);
    assert_eq!(app.on_key(release(KeyCode::Enter)), Action::None);
    assert_eq!(app, before);
}

#[test]
fn list_updates_on_hotplug_without_opening_ports() {
    let mut app = App::new();
    assert!(screen_text(&app).contains(NO_BOARDS_MESSAGE));

    // Plugged in: the native port alone already names the board (MAC from its USB serial).
    // `on_event` returns nothing: an event can never make the UI open a port.
    app.on_event(boards_event(&[c6_native()]));
    let lines = screen_lines(&app);
    assert_eq!(
        line_starting(&lines, " ▸ ●"),
        " ▸ ● ESP32-??   e8:f6:0a:f7:ff:d4  unknown chip"
    );
    assert_eq!(
        line_starting(&lines, "       └─"),
        "       └─ COM11  USB-Serial/JTAG  native   primary"
    );
    assert!(!screen_text(&app).contains(NO_BOARDS_MESSAGE));

    // A bridge appears: listed apart, nothing else changes.
    app.on_event(boards_event(&[c6_native(), ch343("COM13")]));
    assert_eq!(app.boards().len(), 1);
    assert_eq!(names(&app.boards()[0]), ["COM11"]);
    assert!(screen_text(&app).contains("? COM13  CH343            bridge"));

    // Unplugged: back to the empty screen.
    app.on_event(boards_event(&[]));
    assert!(app.boards().is_empty());
    assert!(screen_text(&app).contains(NO_BOARDS_MESSAGE));
}

#[test]
fn enter_with_unidentified_bridges_asks_first() {
    let mut app = App::new();
    app.on_event(boards_event(&[c6_native(), ch343("COM13")]));
    assert_eq!(app.on_key(press(KeyCode::Enter)), Action::None);
    assert_eq!(
        app.screen,
        Screen::ConfirmIdentify {
            ports: vec![ch343("COM13")]
        }
    );
    // Any other key keeps asking.
    assert_eq!(app.on_key(press(KeyCode::Enter)), Action::None);
    assert!(matches!(app.screen, Screen::ConfirmIdentify { .. }));

    assert_eq!(
        app.on_key(press(KeyCode::Char('y'))),
        Action::Identify(vec![ch343("COM13")])
    );
    assert!(matches!(app.screen, Screen::Identifying { .. }));
    // While identifying, `q` cancels instead of quitting.
    let mut cancelling = app.clone();
    assert_eq!(cancelling.on_key(press(KeyCode::Char('q'))), Action::Cancel);

    app.on_event(found(ch343("COM13"), C6_MAC, "esp32c6"));
    app.on_event(Event::IdentifyFinished);
    assert_eq!(app.screen, Screen::List);
    let boards = app.boards();
    assert_eq!(boards.len(), 1);
    assert_eq!(names(&boards[0]), ["COM13", "COM11"]);
    assert_eq!(boards[0].chip(), Some(chip("esp32c6")));

    // Now enter runs the whole board.
    assert_eq!(
        app.on_key(press(KeyCode::Enter)),
        Action::Run(boards[0].clone())
    );
}

#[test]
fn declining_keeps_bridges_out() {
    let mut app = App::new();
    app.on_event(boards_event(&[c6_native(), ch343("COM13")]));
    app.on_key(press(KeyCode::Enter));
    assert_eq!(app.on_key(press(KeyCode::Char('n'))), Action::None);
    assert_eq!(app.screen, Screen::List);

    let boards = app.boards();
    assert_eq!(names(&boards[0]), ["COM11"]);
    assert!(screen_text(&app).contains("? COM13"));

    // Not asked again on enter: the board runs with its native port only.
    assert_eq!(
        app.on_key(press(KeyCode::Enter)),
        Action::Run(boards[0].clone())
    );

    // `i` asks again on purpose.
    let mut app = App::new();
    app.on_event(boards_event(&[c6_native(), ch343("COM13")]));
    app.on_key(press(KeyCode::Enter));
    app.on_key(press(KeyCode::Esc));
    app.on_key(press(KeyCode::Char('i')));
    assert_eq!(
        app.screen,
        Screen::ConfirmIdentify {
            ports: vec![ch343("COM13")]
        }
    );
}

#[test]
fn single_board_runs_without_picking() {
    let mut app = App::new();
    app.on_event(boards_event(&[c6_native()]));
    let Action::Run(board) = app.on_key(press(KeyCode::Enter)) else {
        panic!("enter did not start a run");
    };
    assert_eq!(board.mac(), mac(C6_MAC));
    assert!(matches!(app.screen, Screen::Running(_)));
}

#[test]
fn arrows_select_board() {
    let mut app = App::new();
    app.on_event(boards_event(&[c6_native(), c3_native()]));
    // Up at the top stays; down moves; down at the bottom stays.
    app.on_key(press(KeyCode::Up));
    app.on_key(press(KeyCode::Down));
    app.on_key(press(KeyCode::Down));
    let lines = screen_lines(&app);
    assert!(line_starting(&lines, " ▸ ●").contains(C3_MAC));
    let Action::Run(board) = app.on_key(press(KeyCode::Enter)) else {
        panic!("enter did not start a run");
    };
    assert_eq!(board.mac(), mac(C3_MAC));

    // The selection follows the board, not the row, when another board is plugged in above.
    let mut app = App::new();
    app.on_event(boards_event(&[c6_native(), c3_native()]));
    app.on_key(press(KeyCode::Down));
    app.on_event(boards_event(&[
        native("COM20", "02:00:00:00:00:01"),
        c6_native(),
        c3_native(),
    ]));
    let Action::Run(board) = app.on_key(press(KeyCode::Enter)) else {
        panic!("enter did not start a run");
    };
    assert_eq!(board.mac(), mac(C3_MAC));
}

#[test]
fn q_during_run_requests_cancel_not_quit() {
    let mut app = App::new();
    app.on_event(boards_event(&[c6_native()]));
    app.on_key(press(KeyCode::Enter));
    assert_eq!(app.on_key(press(KeyCode::Char('q'))), Action::Cancel);
    assert!(matches!(&app.screen, Screen::Running(live) if live.cancelling));
    assert!(screen_text(&app).contains("cancelling"));
    // Ctrl+C while cancelling is the way out if the board never answers.
    let mut stuck = app.clone();
    assert_eq!(stuck.on_key(ctrl_c()), Action::Quit);

    app.on_event(Event::RunCancelled(HardReset::done()));
    assert_eq!(app.screen, Screen::List);
    assert_eq!(app.on_key(press(KeyCode::Char('q'))), Action::Quit);
}

#[test]
fn done_enter_goes_back_and_q_quits() {
    let mut app = App::new();
    app.on_event(boards_event(&[c6_native()]));
    app.on_key(press(KeyCode::Enter));
    app.on_event(Event::RunFinished(c6_report()));
    assert!(matches!(app.screen, Screen::Done { .. }));
    let mut back = app.clone();
    assert_eq!(back.on_key(press(KeyCode::Enter)), Action::None);
    assert_eq!(back.screen, Screen::List);
    // The chip learned in the run stays for the session.
    assert_eq!(back.boards()[0].chip(), Some(chip("esp32c6")));
    assert_eq!(app.on_key(press(KeyCode::Char('q'))), Action::Quit);
}

#[test]
fn identification_is_forgotten_when_the_bridge_is_unplugged() {
    let mut app = bench_app();
    assert_eq!(names(&app.boards()[0]), ["COM13", "COM11"]);
    app.on_event(boards_event(&[c6_native(), c3_native()]));
    app.on_event(boards_event(&[
        ch343("COM13"),
        c6_native(),
        ch340k("COM10"),
        c3_native(),
    ]));
    assert_eq!(names(&app.boards()[0]), ["COM11"]);
}

// ---- screens (TestBackend snapshots, by line content) ----

#[test]
fn list_screen_snapshot() {
    let lines = screen_lines(&bench_app());
    let header = &lines[0];
    assert!(header.starts_with(" esp32-devkit-tester "), "{header}");
    assert!(
        header.ends_with(&format!("v{}", env!("CARGO_PKG_VERSION"))),
        "{header}"
    );
    assert!(lines[1].starts_with(" ───"));
    let body: Vec<&str> = lines[2..15].iter().map(String::as_str).collect();
    assert_eq!(
        body,
        [
            " Boards",
            " ▸ ● ESP32-C6   e8:f6:0a:f7:ff:d4",
            "       ├─ COM13  CH343            bridge   primary",
            "       └─ COM11  USB-Serial/JTAG  native",
            "   ● ESP32-C3   80:53:e0:0a:5d:ac",
            "       ├─ COM10  CH340K           bridge   primary",
            "       └─ COM6   USB-Serial/JTAG  native",
            "",
            " Not identified yet",
            "   ? COM7   CP210x           bridge",
            "",
            " 2 boards · 5 ports · 1 not identified",
            "",
        ]
    );
    assert!(lines[22].starts_with(" ───"));
    assert_eq!(
        lines[23],
        " ↑↓ select   enter test   i identify bridges   q quit"
    );
}

/// An S3 whose app runs TinyUSB on the OTG controller (bench, 2026-11-26).
fn s3_otg(name: &str) -> PortDesc {
    let mut port = usb(name, 0x303A, 0x4001, "123456");
    if let Some(usb) = port.usb.as_mut() {
        usb.product = Some("Dispositivo Serial USB (COM14)".to_owned());
        usb.interface = Some(0);
    }
    port
}

#[test]
fn list_screen_snapshot_with_other_ports() {
    let mut app = App::new();
    app.on_event(boards_event(&[
        ch343("COM13"),
        c6_native(),
        s3_otg("COM14"),
        usb("COM5", 0x1234, 0x5678, "A1"),
    ]));
    app.on_event(found(ch343("COM13"), C6_MAC, "esp32c6"));
    let lines = screen_lines(&app);
    let body: Vec<&str> = lines[2..13].iter().map(String::as_str).collect();
    assert_eq!(
        body,
        [
            " Boards",
            " ▸ ● ESP32-C6   e8:f6:0a:f7:ff:d4",
            "       ├─ COM13  CH343            bridge   primary",
            "       └─ COM11  USB-Serial/JTAG  native",
            "",
            " Other ports (not tested)",
            "   · COM14  303A:4001        Espressif USB from the app firmware (USB OTG)",
            "   · COM5   1234:5678        unknown device",
            "",
            " 1 board · 2 ports · 0 not identified · 2 other",
            "",
        ]
    );
    // Listed only: the OTG port never joins a board and nothing asks to identify it.
    assert_eq!(names(&app.boards()[0]), ["COM13", "COM11"]);
    assert!(app.never_asked().is_empty());
    assert_eq!(lines[23], " ↑↓ select   enter test   q quit");
}

#[test]
fn other_ports_alone_are_listed_instead_of_the_empty_message() {
    let mut app = App::new();
    app.on_event(boards_event(&[s3_otg("COM14")]));
    let text = screen_text(&app);
    assert!(!text.contains(NO_BOARDS_MESSAGE), "{text}");
    assert!(text.contains(" Other ports (not tested)"), "{text}");
    assert!(
        text.contains(" 0 boards · 0 ports · 0 not identified · 1 other"),
        "{text}"
    );
    // Enter has nothing to test or identify.
    assert_eq!(app.on_key(press(KeyCode::Enter)), Action::None);
    assert_eq!(app.screen, Screen::List);
}

#[test]
fn confirm_screen_snapshot() {
    let mut app = App::new();
    app.on_event(boards_event(&[c6_native(), cp210x("COM7")]));
    app.on_key(press(KeyCode::Enter));
    let lines = screen_lines(&app);
    let top = lines
        .iter()
        .position(|line| line.contains("┌ Identify bridges "))
        .unwrap_or_else(|| panic!("no popup:\n{}", lines.join("\n")));
    let column = lines[top].chars().position(|c| c == '┌').unwrap();
    let inner: Vec<String> = lines[top..top + 7]
        .iter()
        .map(|line| line.chars().skip(column).collect())
        .collect();
    assert_eq!(
        inner,
        [
            "┌ Identify bridges ────────────────────────────────┐",
            "│ 1 bridge port is not identified yet.             │",
            "│ To read its MAC the board is reset into the ROM  │",
            "│ and then back to its app. Nothing is written.    │",
            "│                                                  │",
            "│           y identify          n skip             │",
            "└──────────────────────────────────────────────────┘",
        ]
    );
    assert_eq!(lines[23], " y identify   n skip");
}

/// A pass outcome with a read value, as the runner would announce it.
fn pass(id: TestId, ms: u64, read: &str) -> Event {
    Event::TestFinished(TestResult {
        id,
        outcome: Outcome::pass(ms, None, Some(SafeText::from_host(read))),
    })
}

#[test]
fn running_screen_snapshot() {
    let mut app = bench_app();
    start_run(&mut app);
    app.on_event(Event::TestStarted(UART_BRIDGE));
    app.on_event(pass(UART_BRIDGE, 350, C6_MAC));
    app.on_event(Event::TestStarted(CHIP_IDENTITY));
    app.on_event(pass(CHIP_IDENTITY, 4, "security info chip id 13"));
    app.on_event(Event::TestStarted(USB_SERIAL_JTAG));
    app.on_event(Event::Waiting { port: c6_native() });

    let lines = screen_lines(&app);
    let header = &lines[0];
    assert!(
        header.starts_with(" ESP32-C6  ·  e8:f6:0a:f7:ff:d4  ·  COM13 + COM11 "),
        "{header}"
    );
    assert!(header.ends_with("⠋ running"), "{header}");
    let body: Vec<&str> = lines[2..9].iter().map(String::as_str).collect();
    assert_eq!(body[0], "  port");
    assert!(
        body[1].starts_with("    ✓ uart-bridge          e8:f6:0a:f7:ff:d4 "),
        "{}",
        body[1]
    );
    assert!(body[1].ends_with(" 350 ms"), "{}", body[1]);
    assert_eq!(
        body[2],
        "    ⠋ usb-serial-jtag      waiting for COM11 to come back…"
    );
    assert_eq!(body[3], "  rom");
    assert!(
        body[4].starts_with("    ✓ chip-identity        security info chip id 13 "),
        "{}",
        body[4]
    );
    assert!(body[4].ends_with(" 4 ms"), "{}", body[4]);
    // Durations are flush right, in one column.
    assert_eq!(body[1].chars().count(), body[4].chars().count());
    assert_eq!(body[5], "    …");
    assert_eq!(body[6], "");
    // Everything fits: no summary block, no scroll hint, no `more` indicator.
    assert!(lines[22].starts_with(" ───"), "{}", lines[22]);
    assert!(lines[3..22].iter().all(|line| !line.starts_with(" ───")));
    assert_eq!(lines[23], " q cancel");

    // The spinner turns with the loop's tick.
    app.tick();
    assert!(screen_lines(&app)[0].ends_with("⠙ running"));
}

/// The report of a whole C6 run, made by the real runner over fake probes.
fn c6_report() -> Report {
    let log = Log::default();
    let time = TestTime::default();
    let mut enumerate = c6_enumerate();
    let board = crate::runner::worker::testing::c6_board();
    runner::run_board(
        &mut c6_probes(&log, None),
        &board,
        &time,
        &time,
        &mut enumerate,
        &mut |_| {},
    )
}

#[test]
fn done_screen_snapshot() {
    let mut app = bench_app();
    start_run(&mut app);
    app.on_event(Event::RunFinished(c6_report()));
    let lines = screen_lines(&app);
    // The verdict, then the tabs: the run opens on Report.
    assert!(lines[0].ends_with("PASS   [Report]  Board"), "{}", lines[0]);
    let text = lines.join("\n");
    for id in BOARD_TESTS {
        assert!(text.contains(&format!("✓ {:<20}", id.name)), "{id}\n{text}");
    }
    assert_eq!(lines[2], "  port");
    assert!(
        lines[3].starts_with("    ✓ uart-bridge          e8:f6:0a:f7:ff:d4 "),
        "{}",
        lines[3]
    );
    assert!(lines[3].ends_with(" 0 ms"), "{}", lines[3]);
    // A plain pass has no detail lines: suite headers and one line per test, nothing else.
    assert_eq!(lines[17], "");
    // The summary is fixed under the body, between two rules, and the body fits: no scroll.
    assert!(lines[19].starts_with(" ───"), "{}", lines[19]);
    assert_eq!(lines[20], " Tests: 11 passed, 11 total");
    assert_eq!(lines[21], " ✓ board back to its app");
    assert!(lines[22].starts_with(" ───"), "{}", lines[22]);
    assert!(lines.iter().all(|line| line.chars().count() < 80));
    assert_eq!(lines[23], " tab board   enter back to boards   q quit");
}

// ---- the loop never waits on the board ----

#[test]
fn ui_loop_never_blocks() {
    use crate::probe::fake::Step;
    use crate::runner::worker::Job;

    let log = Log::default();
    let (held, gate) = gate(Step::Revision);
    let (tx, events) = mpsc::channel();
    let worker = Worker::spawn(
        c6_probes(&log, Some(held)),
        TestTime::default(),
        c6_enumerate(),
        tx,
    )
    .unwrap();

    let mut app = App::new();
    app.on_event(boards_event(&[c6_native()]));
    let Action::Run(board) = app.on_key(press(KeyCode::Enter)) else {
        panic!("enter did not start a run");
    };
    assert!(worker.submit(Job::Run(board)));
    // The runner's thread is now stuck on the board (held at the revision read)...
    assert_eq!(
        gate.reached.recv_timeout(Duration::from_secs(10)),
        Ok(Step::Revision)
    );

    // ...and the UI side still takes keys, drains events and draws, without waiting.
    let start = Instant::now();
    for _ in 0..20 {
        app.on_key(press(KeyCode::Down));
        while let Ok(event) = events.try_recv() {
            app.on_event(event);
        }
        app.tick();
        let _ = screen_lines(&app);
    }
    assert_eq!(app.on_key(press(KeyCode::Char('q'))), Action::Cancel);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "UI side took {:?}",
        start.elapsed()
    );
    assert!(matches!(&app.screen, Screen::Running(live) if live.current == Some(BOARD_TESTS[1])));

    worker.cancel();
    gate.release.send(()).unwrap();
    for event in collect_until(&events, |event| matches!(event, Event::RunCancelled(_))) {
        app.on_event(event);
    }
    assert_eq!(app.screen, Screen::List);
    assert!(worker.shutdown(Duration::from_secs(5)));
}

#[test]
fn board_text_cannot_reach_the_terminal_raw() {
    // A native port whose name carries an escape sequence (a hostile udev rule, say).
    let mut app = App::new();
    app.on_event(boards_event(&[native(
        "\u{1b}[2Jevil",
        "E8:F6:0A:F7:FF:D4",
    )]));
    let text = screen_text(&app);
    assert!(!text.contains('\u{1b}'));
    assert!(text.contains("?[2Jevil"));
}

// ---- report details, wrapping and scroll (#22) ----

const ESP32_MAC: &str = "1c:c3:ab:d2:25:b4";

fn host(text: &str) -> Option<SafeText> {
    Some(SafeText::from_host(text))
}

/// A complete report for `target`, with `outcomes` finished in this order.
fn report_of(target: &str, outcomes: Vec<(TestId, Outcome)>, reset: HardReset) -> Report {
    let board = BoardInfo {
        chip: Some(ChipInfo {
            chip: chip(target),
            description: None,
            revision: Some(Revision { major: 3, minor: 1 }),
        }),
        mac: host(ESP32_MAC),
        ports: Vec::new(),
    };
    let plan = outcomes.iter().map(|(id, _)| *id).collect();
    let mut run = Run::new("0.1.0", board, plan);
    for (id, outcome) in outcomes {
        run.finish(id, outcome).unwrap();
    }
    run.complete(reset)
}

/// Spec example 8.4: the ESP32 of the bench with a 26 MHz crystal (a `variable` finding) and
/// the long SKIP that was cut at 80 columns on the bench.
fn esp32_report() -> Report {
    let esp32 = chip("esp32");
    let same = |ms, value| Outcome::pass(ms, host(value), host(value));
    let read = |ms, value| Outcome::pass(ms, None, host(value));
    report_of(
        "esp32",
        vec![
            (UART_BRIDGE, same(2310, ESP32_MAC)),
            (USB_SERIAL_JTAG, Outcome::skip(SKIP_NO_USB_SERIAL_JTAG)),
            (
                CHIP_IDENTITY,
                same(11, "magic register 0x40001000 = 0x00F01D83"),
            ),
            (REVISION, read(14, "v3.1")),
            (
                CRYSTAL,
                Outcome::from_comparison(esp32, "crystal_mhz", SafeText::from_board("26 MHz"), 3),
            ),
            (MAC_EFUSE_REG, same(9, ESP32_MAC)),
            (PACKAGE, read(6, "package 1")),
            (PSRAM, read(5, "none in package")),
            (EMBEDDED_FLASH, read(5, "none")),
            (JEDEC_ID, read(38, "0x5E4016")),
            (FLASH_SIZE, read(0, "4 MB")),
        ],
        HardReset::done(),
    )
}

/// An app showing `report` as the result of `board`, on a `width` × `height` terminal.
fn done_app(report: Report, width: u16, height: u16) -> App {
    let board = Board::new(mac(ESP32_MAC), Some(chip("esp32")), vec![cp210x("COM9")]).unwrap();
    let mut app = App::new();
    app.resize(width, height);
    app.screen = Screen::Done {
        board,
        report,
        tab: Tab::Report,
    };
    app.scroll = ScrollState::top();
    app
}

/// The rows of `lines` from the one equal to `first`, `count` of them.
fn block<'a>(lines: &'a [String], first: &str, count: usize) -> Vec<&'a str> {
    let start = lines
        .iter()
        .position(|line| line == first)
        .unwrap_or_else(|| panic!("no line {first:?}:\n{}", lines.join("\n")));
    lines[start..start + count]
        .iter()
        .map(String::as_str)
        .collect()
}

/// The test line that starts with `prefix`, checked to end with `duration` flush right.
fn test_row<'a>(lines: &'a [String], prefix: &str, duration: &str, width: usize) -> &'a str {
    let line = line_starting(lines, prefix);
    assert!(line.ends_with(&format!(" {duration}")), "{line}");
    assert_eq!(line.chars().count(), width - 1, "{line}");
    line
}

fn assert_fits(lines: &[String], width: usize) {
    for line in lines {
        assert!(line.chars().count() < width, "wider than {width}: {line:?}");
    }
}

#[test]
fn long_skip_reason_still_wraps() {
    let lines = screen_lines_sized(&done_app(esp32_report(), 80, 40), 80, 40);
    assert_fits(&lines, 80);
    assert_eq!(
        block(&lines, "  port", 5),
        [
            "  port",
            "    ✓ uart-bridge          1c:c3:ab:d2:25:b4                            2310 ms",
            "    ○ usb-serial-jtag",
            "                           └ chip has no USB-Serial/JTAG; JTAG needs an",
            "                             external probe (GPIO12-15)",
        ]
    );
    test_row(&lines, "    ✓ uart-bridge ", "2310 ms", 80);
    test_row(
        &lines,
        "    ✓ chip-identity        magic register 0x40001000 = 0x00F01D83 ",
        "11 ms",
        80,
    );

    // At 60 columns the reason still shows whole, the duration stays, the value gives way.
    let lines = screen_lines_sized(&done_app(esp32_report(), 60, 40), 60, 40);
    assert_fits(&lines, 60);
    let start = lines
        .iter()
        .position(|line| line == "    ○ usb-serial-jtag")
        .unwrap();
    let reason: Vec<&str> = lines[start + 1..]
        .iter()
        .take_while(|line| line.starts_with("                           "))
        .map(|line| line.trim_start().trim_start_matches("└ "))
        .collect();
    assert_eq!(reason.join(" "), SKIP_NO_USB_SERIAL_JTAG);
    assert_eq!(reason.len(), 3, "{reason:?}");
    assert_eq!(
        test_row(&lines, "    ✓ chip-identity ", "11 ms", 60),
        "    ✓ chip-identity        magic register 0x4000100…  11 ms"
    );
}

/// `board` run by the real runner, the session on each port answered by the probe of the same
/// name; the OS list always shows the board's ports (nothing re-enumerates in these cases).
fn run_ports(board: &Board, probes: Vec<(&str, FakeProbe)>) -> Report {
    let time = TestTime::default();
    let ports: Vec<PortDesc> = board.ports().cloned().collect();
    let mut enumerate = move || ports.clone();
    runner::run_board(
        &mut |port: &PortDesc| -> Box<dyn Probe> {
            let probe = probes
                .iter()
                .find(|(name, _)| *name == port.name)
                .map(|(_, probe)| probe.clone())
                .unwrap_or_else(|| panic!("no probe for {}", port.name));
            Box::new(probe)
        },
        board,
        &time,
        &time,
        &mut enumerate,
        &mut |_| {},
    )
}

/// The Done screen of `board`'s `report`, its durations masked (`# ms`, same width): the syncs
/// are timed by the real clock.
fn done_screen(board: Board, report: Report, width: u16, height: u16) -> Vec<String> {
    let mut app = App::new();
    app.resize(width, height);
    app.screen = Screen::Done {
        board,
        report,
        tab: Tab::Report,
    };
    app.scroll = ScrollState::top();
    screen_lines_sized(&app, width, height)
        .into_iter()
        .map(|line| match line.strip_suffix(" ms") {
            Some(head) => {
                let digits = head.len() - head.trim_end_matches(|c: char| c.is_ascii_digit()).len();
                format!("{}{} ms", &head[..head.len() - digits], "#".repeat(digits))
            }
            None => line,
        })
        .collect()
}

/// #67: a SKIP's reason that fits in the value column stays on the test line, in gray; one that
/// does not fit still goes under it (see `long_skip_reason_still_wraps`).
#[test]
fn skip_reason_is_inline_when_it_fits() {
    // Bench, 2026-11-28: esp32-logcat holds the C3's bridge; the native port runs the suites.
    // (The fixture's C3 is the one whose MAC ends in 5d:70.)
    let c3_mac = "80:53:e0:0a:5d:70";
    let c3 = || {
        Board::new(
            mac(c3_mac),
            Some(chip("esp32c3")),
            vec![ch340k("COM5"), native("COM6", "80:53:E0:0A:5D:70")],
        )
        .unwrap()
    };
    let report = run_ports(
        &c3(),
        vec![
            (
                "COM5",
                fixture("esp32c3").fail_at(Step::Connect, crate::probe::ProbeError::Busy),
            ),
            ("COM6", fixture("esp32c3")),
        ],
    );
    let lines = done_screen(c3(), report, 80, 30);
    assert_fits(&lines, 80);
    assert_eq!(
        block(&lines, "  port", 20),
        [
            "  port",
            "    ✕ uart-bridge          80:53:e0:0a:5d:70                               # ms",
            "                           └ sync failed: port busy: another program (e.g. a",
            "                             serial monitor) has it open",
            "    ✓ usb-serial-jtag      80:53:e0:0a:5d:70                               # ms",
            "  rom",
            "    ✓ chip-identity        security info chip id 5                         # ms",
            "    ✓ revision             v0.4                                            # ms",
            "    ✓ crystal              40 MHz                                          # ms",
            "    ✓ mac-efuse-reg        80:53:e0:0a:5d:70                               # ms",
            "  efuse",
            "    ✓ package              package 0                                       # ms",
            "    ✓ psram                none in package                                 # ms",
            "    ✓ embedded-flash       4 MB (XMC)                                      # ms",
            "  flash",
            "    ✓ jedec-id             0x464016                                        # ms",
            "    ✓ size                 4 MB                                            # ms",
            "",
            "",
            "",
        ]
    );
    assert!(
        lines
            .iter()
            .any(|line| line == " Tests: 1 failed, 10 passed, 11 total"),
        "{}",
        lines.join(
            "
"
        )
    );

    // The C6 through its native port alone, held by a serial monitor: nothing synced, each
    // `not run` takes one line.
    let c6 = || Board::new(mac(C6_MAC), Some(chip("esp32c6")), vec![c6_native()]).unwrap();
    let busy = fixture("esp32c6").fail_at(Step::Connect, crate::probe::ProbeError::Busy);
    let report = run_ports(&c6(), vec![("COM11", busy.clone())]);
    let buffer = {
        let mut app = App::new();
        app.resize(80, 30);
        app.screen = Screen::Done {
            board: c6(),
            report: report.clone(),
            tab: Tab::Report,
        };
        app.scroll = ScrollState::top();
        screen_buffer(&app, 80, 30)
    };
    let lines = done_screen(c6(), report, 80, 30);
    assert_fits(&lines, 80);
    let not_run = "not run: port/usb-serial-jtag failed";
    assert_eq!(
        block(&lines, "  rom", 3),
        [
            "  rom",
            &format!("    ○ chip-identity        {not_run}"),
            &format!("    ○ revision             {not_run}"),
        ]
    );
    for id in BOARD_TESTS {
        let row = format!("    ○ {:<20} {not_run}", id.name);
        let at = lines
            .iter()
            .position(|line| *line == row)
            .unwrap_or_else(|| panic!("{row}\n{}", lines.join("\n")));
        // No detail line under it.
        assert!(!lines[at + 1].contains('└'), "{}", lines[at + 1]);
        assert_eq!(fg_at(&buffer, at, not_run), Color::DarkGray);
        assert_eq!(fg_at(&buffer, at, "○"), Color::Yellow);
    }

    // At 60 columns the same reason no longer fits after the name: it goes under, as before.
    let report = run_ports(&c6(), vec![("COM11", busy)]);
    let lines = done_screen(c6(), report, 60, 40);
    assert_fits(&lines, 60);
    assert_eq!(
        block(&lines, "    ○ chip-identity", 3),
        [
            "    ○ chip-identity",
            "                           └ not run: port/usb-serial-jtag",
            "                             failed",
        ]
    );
}

#[test]
fn fail_shows_expected_read_and_reason() {
    let c3 = "80:53:e0:0a:5d:ac";
    let efuse = "80:53:e0:0a:5d:ad";
    let report = report_of(
        "esp32c3",
        vec![
            (
                UART_BRIDGE,
                Outcome::fail("MAC differs between ports", 2, host(c3), host(efuse)),
            ),
            (
                USB_SERIAL_JTAG,
                Outcome::fail(
                    "port busy: another program (e.g. a serial monitor) has it open",
                    5,
                    None,
                    None,
                ),
            ),
            (
                MAC_EFUSE_REG,
                Outcome::fail(FAIL_MAC_EFUSE_MISMATCH, 17, host(c3), host(efuse)),
            ),
        ],
        HardReset::done(),
    );
    let app = done_app(report, 80, 30);
    let buffer = screen_buffer(&app, 80, 30);
    let lines = buffer_rows(&buffer);
    assert_fits(&lines, 80);
    assert!(lines[0].ends_with("FAIL   [Report]  Board"), "{}", lines[0]);

    test_row(
        &lines,
        "    ✕ mac-efuse-reg        80:53:e0:0a:5d:ad ",
        "17 ms",
        80,
    );
    assert_eq!(
        block(
            &lines,
            "                           └ expected 80:53:e0:0a:5d:ac (ROM)",
            4
        ),
        [
            "                           └ expected 80:53:e0:0a:5d:ac (ROM)",
            "                             read     80:53:e0:0a:5d:ad (eFuse)",
            "                             MAC decoded from the eFuse words differs from the",
            "                             ROM MAC",
        ]
    );
    // No value: the reason leaves the value column and wraps underneath.
    assert_eq!(
        test_row(&lines, "    ✕ usb-serial-jtag", "5 ms", 80)
            .trim_end_matches(" 5 ms")
            .trim_end(),
        "    ✕ usb-serial-jtag"
    );
    // Exactly the room (50 columns at 80) still fits on the row; the rest wraps.
    assert_eq!(
        block(
            &lines,
            "                           └ port busy: another program (e.g. a serial monitor)",
            2
        )[1],
        "                             has it open"
    );
    // `(ROM)`/`(eFuse)` belong to `rom/mac-efuse-reg` only.
    let uart = block(&lines, &lines[3].clone(), 3);
    assert_eq!(
        uart,
        [
            lines[3].as_str(),
            "                           └ expected 80:53:e0:0a:5d:ac",
            "                             read     80:53:e0:0a:5d:ad",
        ]
    );
    assert!(lines[3].starts_with("    ✕ uart-bridge "), "{}", lines[3]);

    // Labels and the reason in gray, the values in the default colour.
    let row = lines
        .iter()
        .position(|line| line.ends_with("80:53:e0:0a:5d:ac (ROM)"))
        .unwrap();
    assert_eq!(fg_at(&buffer, row, "└ expected"), Color::DarkGray);
    assert_eq!(fg_at(&buffer, row, "expected"), Color::DarkGray);
    assert_eq!(fg_at(&buffer, row, "80:53"), Color::Reset);
    assert_eq!(fg_at(&buffer, row, "(ROM)"), Color::DarkGray);
    assert_eq!(fg_at(&buffer, row + 1, "read"), Color::DarkGray);
    assert_eq!(fg_at(&buffer, row + 1, "80:53"), Color::Reset);
    assert_eq!(fg_at(&buffer, row + 2, "MAC decoded"), Color::DarkGray);
}

#[test]
fn finding_shows_table_read_kind_and_source() {
    let app = done_app(esp32_report(), 80, 40);
    let buffer = screen_buffer(&app, 80, 40);
    let lines = buffer_rows(&buffer);
    let line = test_row(&lines, "    ✓ crystal              26 MHz ! ", "3 ms", 80);
    assert_eq!(
        block(&lines, line, 4)[1..],
        [
            "                           └ table 40 MHz · read 26 MHz",
            "                             variable (kept as pass) · ESP32 Series Datasheet",
            "                             v5.3, Features; ESP32 Series Comparison",
        ]
    );
    let row = lines.iter().position(|l| l == line).unwrap();
    assert_eq!(fg_at(&buffer, row, "!"), Color::Yellow);
    assert_eq!(fg_at(&buffer, row + 1, "table"), Color::DarkGray);
    assert_eq!(fg_at(&buffer, row + 1, "40 MHz"), Color::Reset);
    assert_eq!(fg_at(&buffer, row + 1, "·"), Color::DarkGray);
    assert_eq!(fg_at(&buffer, row + 2, "variable"), Color::DarkGray);
    assert_eq!(
        line_starting(&lines, " Tests:"),
        " Tests: 10 passed, 1 skipped, 11 total"
    );

    // An `invariant` finding fails; the finding says why, so the reason is not repeated.
    let s3 = chip("esp32s3");
    let identity = Outcome::from_comparison(
        s3,
        "identity",
        SafeText::from_board("security info chip id 13"),
        12,
    );
    assert_eq!(identity.state(), State::Fail);
    let report = report_of(
        "esp32s3",
        vec![(CHIP_IDENTITY, identity)],
        HardReset::done(),
    );
    let lines = screen_lines_sized(&done_app(report, 60, 30), 60, 30);
    assert_fits(&lines, 60);
    let text = lines.join("\n");
    // Too long for one row at 60: `table` and `read` take a row each, values lined up.
    assert_eq!(
        block(
            &lines,
            "                           └ table security info chip id 9",
            2
        )[1],
        "                             read  security info chip id 13",
        "{text}"
    );
    // The value gives way (no space left before `…`); `!` and the duration stay.
    test_row(
        &lines,
        "    ✕ chip-identity        security info chip id… ! ",
        "12 ms",
        60,
    );
    assert!(text.contains("invariant (failed) ·"), "{text}");
    assert!(
        !text.contains("differs from the capability table"),
        "{text}"
    );
}

/// Footer and first body row of an app at 80 × `height`.
fn view(app: &App, height: u16) -> (String, String) {
    let lines = screen_lines_sized(app, 80, height);
    (lines[usize::from(height) - 1].clone(), lines[2].clone())
}

#[test]
fn scroll_keys_move_and_clamp() {
    // 16 rows: header, rule, 9 body rows, rule, 2 summary rows, rule, footer.
    let report = esp32_report();
    let body: Vec<String> = report_view::done_lines(&report, 80)
        .iter()
        .map(|line| line.to_string().trim_end().to_owned())
        .collect();
    let max = body.len() - 9;
    assert!(max > 9, "the report must be taller than two pages");
    let mut app = done_app(report, 80, 16);
    let first = |app: &App| view(app, 16).1;

    for (key, expected) in [
        (KeyCode::Up, 0),
        (KeyCode::Down, 1),
        (KeyCode::PageDown, 9),
        (KeyCode::End, max),
        (KeyCode::Down, max),
        (KeyCode::PageDown, max),
        (KeyCode::PageUp, max - 8),
        (KeyCode::Home, 0),
        (KeyCode::Char('j'), 1),
        (KeyCode::Char('k'), 0),
    ] {
        assert_eq!(app.on_key(press(key)), Action::None, "{key:?}");
        assert_eq!(first(&app), body[expected], "after {key:?}");
    }
    // Releases do not scroll (Windows reports them too).
    app.on_key(release(KeyCode::Down));
    assert_eq!(first(&app), body[0]);
    // A taller terminal shows everything; the keys then have nothing to move.
    app.resize(80, 40);
    app.on_key(press(KeyCode::End));
    assert_eq!(view(&app, 40).1, body[0]);
    // Enter still goes back to the list.
    app.on_key(press(KeyCode::Enter));
    assert_eq!(app.screen, Screen::List);
}

#[test]
fn running_view_follows_unless_user_scrolled() {
    // 12 rows: header, rule, 8 body rows, rule, footer.
    let mut app = bench_app();
    app.resize(80, 12);
    start_run(&mut app);
    let last_row = |app: &App| screen_lines_sized(app, 80, 12)[9].clone();

    app.on_event(pass(UART_BRIDGE, 350, C6_MAC));
    app.on_event(Event::TestFinished(TestResult {
        id: USB_SERIAL_JTAG,
        outcome: Outcome::skip(SKIP_NO_USB_SERIAL_JTAG),
    }));
    for id in &BOARD_TESTS[..5] {
        app.on_event(pass(*id, 4, "ok"));
    }
    // Following: the newest line is at the bottom, with what scrolled away counted above.
    assert_eq!(last_row(&app), "    …");
    let (footer, _) = view(&app, 12);
    assert!(footer.starts_with(" ↑↓ scroll   q cancel "), "{footer}");
    assert!(footer.ends_with("↑ 5 more"), "{footer}");

    // The user scrolls up: new results no longer move the view.
    app.on_key(press(KeyCode::Up));
    let first = view(&app, 12).1;
    app.on_event(pass(BOARD_TESTS[5], 4, "ok"));
    app.on_event(pass(BOARD_TESTS[6], 4, "ok"));
    assert_eq!(view(&app, 12).1, first);
    assert_ne!(last_row(&app), "    …");
    assert!(
        view(&app, 12).0.ends_with("↓ 3 more"),
        "{}",
        view(&app, 12).0
    );

    // Back to the bottom (End, or ↓ until the end) follows again.
    app.on_key(press(KeyCode::End));
    app.on_event(pass(BOARD_TESTS[7], 38, "0xEF4016"));
    assert_eq!(last_row(&app), "    …");
    app.on_key(press(KeyCode::Up));
    app.on_key(press(KeyCode::Down));
    app.on_event(Event::TestStarted(BOARD_TESTS[8]));
    assert!(
        last_row(&app).starts_with("    ⠋ size"),
        "{}",
        last_row(&app)
    );
    // Scrolling never cancels: `q` still does.
    assert_eq!(app.on_key(press(KeyCode::Char('q'))), Action::Cancel);
}

#[test]
fn more_indicator_counts_hidden_lines() {
    let report = esp32_report();
    let hidden = report_view::done_lines(&report, 80).len() - 9;
    let mut app = done_app(report, 80, 16);
    let footer = |app: &App| view(app, 16).0;

    let first = footer(&app);
    assert!(
        first.starts_with(" tab board   ↑↓ scroll   enter back to boards   q quit "),
        "{first}"
    );
    assert!(first.ends_with(&format!(" ↓ {hidden} more")), "{first}");
    assert_eq!(first.chars().count(), 79, "flush right: {first}");

    app.on_key(press(KeyCode::Down));
    app.on_key(press(KeyCode::Down));
    assert!(
        footer(&app).ends_with(&format!(" ↑ 2  ↓ {} more", hidden - 2)),
        "{}",
        footer(&app)
    );
    app.on_key(press(KeyCode::End));
    assert!(
        footer(&app).ends_with(&format!(" ↑ {hidden} more")),
        "{}",
        footer(&app)
    );

    // Everything fits: no indicator and no scroll hint.
    let tall = done_app(esp32_report(), 80, 40);
    assert_eq!(
        view(&tall, 40).0,
        " tab board   enter back to boards   q quit"
    );
}

#[test]
fn summary_and_footer_stay_visible_when_scrolled() {
    let mut app = done_app(esp32_report(), 80, 16);
    for key in [KeyCode::Home, KeyCode::PageDown, KeyCode::End] {
        app.on_key(press(key));
        let lines = screen_lines_sized(&app, 80, 16);
        assert!(
            lines[0].starts_with(" ESP32  ·  1c:c3:ab:d2:25:b4  ·  COM9 "),
            "{key:?}"
        );
        assert!(lines[0].ends_with("PASS   [Report]  Board"), "{key:?}");
        assert!(lines[1].starts_with(" ───"), "{key:?}");
        assert!(lines[11].starts_with(" ───"), "{key:?}");
        assert_eq!(
            lines[12], " Tests: 10 passed, 1 skipped, 11 total",
            "{key:?}"
        );
        assert_eq!(lines[13], " ✓ board back to its app", "{key:?}");
        assert!(lines[14].starts_with(" ───"), "{key:?}");
        assert!(
            lines[15].starts_with(" tab board   ↑↓ scroll   enter back to boards"),
            "{key:?}"
        );
        assert_fits(&lines, 80);
    }

    // A missed reset with a long reason wraps in the fixed block instead of being cut.
    let reason = "port disconnected while waiting for the ROM to answer the reset sequence";
    let report = report_of(
        "esp32",
        vec![(UART_BRIDGE, Outcome::fail("no sync", 3, None, None))],
        HardReset::not_done(reason),
    );
    let lines = screen_lines_sized(&done_app(report, 60, 16), 60, 16);
    assert_fits(&lines, 60);
    let start = lines
        .iter()
        .position(|line| line.starts_with(" ✕ board not back to its app"))
        .unwrap_or_else(|| panic!("{}", lines.join("\n")));
    let text: Vec<&str> = lines[start..start + 2]
        .iter()
        .map(|line| line.trim())
        .collect();
    assert_eq!(
        text.join(" "),
        format!("✕ board not back to its app — {reason}")
    );
    assert!(lines[start + 2].starts_with(" ───"));
}

// ---- Board panel (#23) ----

const S3_MAC: &str = "10:51:db:84:14:0c";

fn ch9102(name: &str) -> PortDesc {
    usb(name, 0x1A86, 0x55D4, "5B53026573")
}

/// `board` run by the real runner, every session answered by `probe` (a fixture, maybe with a
/// fact changed or failing); the OS list always shows the board's ports.
fn run_fixture(board: &Board, probe: &FakeProbe) -> Report {
    let time = TestTime::default();
    let ports: Vec<PortDesc> = board.ports().cloned().collect();
    let mut enumerate = move || ports.clone();
    runner::run_board(
        &mut |_: &PortDesc| -> Box<dyn Probe> { Box::new(probe.clone()) },
        board,
        &time,
        &time,
        &mut enumerate,
        &mut |_| {},
    )
}

fn fixture(target: &str) -> FakeProbe {
    FakeProbe::from_fixture(target).unwrap()
}

/// The S3 of the bench through its CH343 (the native port is not wired on the capture).
fn s3_board() -> Board {
    Board::new(mac(S3_MAC), Some(chip("esp32s3")), vec![ch343("COM12")]).unwrap()
}

/// The ESP32 DevKit V1 of the bench through its CH9102.
fn esp32_board() -> Board {
    Board::new(mac(ESP32_MAC), Some(chip("esp32")), vec![ch9102("COM9")]).unwrap()
}

/// An app on the Board tab of `board`'s result, on a `width` × `height` terminal.
fn board_app(board: Board, report: Report, width: u16, height: u16) -> App {
    let mut app = App::new();
    app.resize(width, height);
    app.screen = Screen::Done {
        board,
        report,
        tab: Tab::Board,
    };
    app
}

/// The body rows of a board panel, from `  Chip` to the last port, as drawn.
fn panel(lines: &[String]) -> Vec<&str> {
    let start = lines
        .iter()
        .position(|line| line == "  Chip")
        .unwrap_or_else(|| panic!("no Board panel:\n{}", lines.join("\n")));
    let end = lines
        .iter()
        .rposition(|line| line.starts_with("    ├─") || line.starts_with("    └─"))
        .unwrap_or_else(|| panic!("no ports:\n{}", lines.join("\n")));
    lines[start..=end].iter().map(String::as_str).collect()
}

#[test]
fn board_panel_merges_table_and_read_values() {
    // S3: datasheet rows (CPU, clock, radios, USB) next to what was read through the CH343.
    let board = s3_board();
    let report = run_fixture(&board, &fixture("esp32s3"));
    let app = board_app(board, report, 80, 30);
    let buffer = screen_buffer(&app, 80, 30);
    let lines = buffer_rows(&buffer);
    assert_fits(&lines, 80);
    assert_eq!(
        lines[0],
        " ESP32-S3  ·  10:51:db:84:14:0c  ·  COM12                PASS   Report  [Board]"
    );
    assert_eq!(
        panel(&lines),
        [
            "  Chip",
            "    Family           ESP32-S3 · package 0                                  read",
            "    Revision         v0.2                                                  read",
            "    CPU              Xtensa LX7 × 2 · ULP-RISC-V + ULP-FSM            datasheet",
            "    Max clock        240 MHz                                          datasheet",
            "    Crystal          40 MHz                                                read",
            "  Memory",
            "    Flash            16 MB external · JEDEC 0x684018                       read",
            "    PSRAM            8 MB in package (AP_3v3)                              read",
            "  Radios",
            "    Wireless         Wi-Fi 4 (2.4 GHz) · BLE                          datasheet",
            "  USB",
            "    USB-Serial/JTAG  yes · not connected on this board                datasheet",
            "    USB OTG          yes                                              datasheet",
            "  Identity",
            "    MAC              10:51:db:84:14:0c                             read (eFuse)",
            "    ROM id           security info chip id 9                               read",
            "  Ports",
            "    └─ COM12  CH343            bridge   primary",
        ]
    );
    // Labels, separators and sources gray; values in the default colour.
    let row = lines
        .iter()
        .position(|line| line.starts_with("    Flash"))
        .unwrap();
    assert_eq!(fg_at(&buffer, row, "Flash"), Color::DarkGray);
    assert_eq!(fg_at(&buffer, row, "16 MB"), Color::Reset);
    assert_eq!(fg_at(&buffer, row, "·"), Color::DarkGray);
    assert_eq!(fg_at(&buffer, row, "0x684018"), Color::Reset);
    assert_eq!(fg_at(&buffer, row, "read"), Color::DarkGray);
    let row = lines
        .iter()
        .position(|line| line.starts_with("    USB-Serial"))
        .unwrap();
    assert_eq!(fg_at(&buffer, row, "not connected"), Color::DarkGray);
    // The summary stays under the panel, the footer names the other tab.
    assert_eq!(
        line_starting(&lines, " Tests:"),
        " Tests: 10 passed, 1 skipped, 11 total"
    );
    assert_eq!(lines[29], " tab report   enter back to boards   q quit");

    // C6: both ports tested; the native one confirms the table's USB-Serial/JTAG.
    let report = c6_report();
    let app = board_app(crate::runner::worker::testing::c6_board(), report, 80, 30);
    let buffer = screen_buffer(&app, 80, 30);
    let lines = buffer_rows(&buffer);
    assert_fits(&lines, 80);
    assert_eq!(
        panel(&lines),
        [
            "  Chip",
            "    Family           ESP32-C6 · package 0                                  read",
            "    Revision         v0.2                                                  read",
            "    CPU              RISC-V × 1 · LP RISC-V                           datasheet",
            "    Max clock        160 MHz                                          datasheet",
            "    Crystal          40 MHz                                                read",
            "  Memory",
            "    Flash            16 MB external · JEDEC 0x684018                       read",
            "    PSRAM            none in package                                       read",
            "  Radios",
            "    Wireless         Wi-Fi 6 (2.4 GHz) · BLE · IEEE 802.15.4          datasheet",
            "  USB",
            "    USB-Serial/JTAG  yes · tested ✓                                        read",
            "    USB OTG          no                                               datasheet",
            "  Identity",
            "    MAC              e8:f6:0a:f7:ff:d4                             read (eFuse)",
            "    ROM id           security info chip id 13                              read",
            "  Ports",
            "    ├─ COM13  CH343            bridge   primary",
            "    └─ COM11  USB-Serial/JTAG  native",
        ]
    );
    let row = lines
        .iter()
        .position(|line| line.starts_with("    USB-Serial"))
        .unwrap();
    assert_eq!(fg_at(&buffer, row, "✓"), Color::Green);
    // Port names are on screen only: the report itself has none.
    assert!(
        format!("{:?}", app_report(&app).board().ports)
            .find("COM1")
            .is_none()
    );
}

/// The report shown by an app on the `Done` screen.
fn app_report(app: &App) -> &Report {
    match &app.screen {
        Screen::Done { report, .. } => report,
        _ => panic!("not on the Done screen"),
    }
}

#[test]
fn read_value_wins_and_divergence_is_marked() {
    // Spec 8.4: a 26 MHz crystal is a `variable` finding: the read value, `!`, the table value.
    let board = s3_board();
    let report = run_fixture(&board, &fixture("esp32s3").with_crystal_mhz(26));
    let app = board_app(board.clone(), report, 80, 30);
    let buffer = screen_buffer(&app, 80, 30);
    let lines = buffer_rows(&buffer);
    assert_fits(&lines, 80);
    assert_eq!(
        block(
            &lines,
            "    Crystal          26 MHz !                                              read",
            2
        )[1],
        "                     └ table 40 MHz (variable)"
    );
    let row = lines
        .iter()
        .position(|line| line.starts_with("    Crystal"))
        .unwrap();
    assert_eq!(fg_at(&buffer, row, "26 MHz"), Color::Reset);
    assert_eq!(fg_at(&buffer, row, "!"), Color::Yellow);
    assert_eq!(fg_at(&buffer, row + 1, "└ table"), Color::DarkGray);
    assert_eq!(fg_at(&buffer, row + 1, "40 MHz"), Color::Reset);
    assert_eq!(fg_at(&buffer, row + 1, "(variable)"), Color::DarkGray);

    // An `invariant` finding (the ROM says another chip id) is shown the same way.
    let identity = crate::chips::ChipIdentity::SecurityChipId(13);
    let report = run_fixture(&board, &fixture("esp32s3").with_identity(identity));
    let lines = screen_lines_sized(&board_app(board, report, 80, 30), 80, 30);
    assert_eq!(
        block(
            &lines,
            "    ROM id           security info chip id 13 !                            read",
            2
        )[1],
        "                     └ table security info chip id 9 (invariant)"
    );

    // No finding, but the test expected another value: the eFuse MAC against the ROM's.
    let report = report_of(
        "esp32c3",
        vec![(
            MAC_EFUSE_REG,
            Outcome::fail(
                FAIL_MAC_EFUSE_MISMATCH,
                17,
                host("80:53:e0:0a:5d:ac"),
                host("80:53:e0:0a:5d:ad"),
            ),
        )],
        HardReset::done(),
    );
    let board = Board::new(mac(C3_MAC), Some(chip("esp32c3")), vec![c3_native()]).unwrap();
    let lines = screen_lines_sized(&board_app(board, report, 80, 40), 80, 40);
    assert_fits(&lines, 80);
    assert_eq!(
        block(
            &lines,
            "    MAC              80:53:e0:0a:5d:ad !                           read (eFuse)",
            2
        )[1],
        "                     └ ROM 80:53:e0:0a:5d:ac"
    );
}

#[test]
fn missing_read_shows_dash_with_reason() {
    let board = s3_board();
    let protocol = || crate::probe::ProbeError::Protocol("bad reply".to_owned());
    let probe = fixture("esp32s3")
        .fail_at(Step::FlashId, protocol())
        .fail_at(Step::Revision, protocol());
    let report = run_fixture(&board, &probe);
    let app = board_app(board.clone(), report, 80, 30);
    let buffer = screen_buffer(&app, 80, 30);
    let lines = buffer_rows(&buffer);
    assert_fits(&lines, 80);
    // No source: nothing was read, and the table is not used in its place.
    assert_eq!(
        line_starting(&lines, "    Flash"),
        "    Flash            — not read (flash/jedec-id failed)"
    );
    assert_eq!(
        line_starting(&lines, "    Revision"),
        "    Revision         — not read (rom/revision failed)"
    );
    let row = lines
        .iter()
        .position(|line| line.starts_with("    Flash"))
        .unwrap();
    assert_eq!(fg_at(&buffer, row, "—"), Color::Reset);
    assert_eq!(fg_at(&buffer, row, "not read"), Color::DarkGray);
    // The rest of the panel is still there.
    assert!(line_starting(&lines, "    PSRAM").contains("8 MB in package"));

    // A blank bus answered RDID: the FAIL read `0xFFFFFF`, which is no flash at all.
    let blank = crate::probe::FlashId {
        manufacturer: 0xFF,
        memory_type: 0xFF,
        capacity: 0xFF,
    };
    let report = run_fixture(&board, &fixture("esp32s3").with_flash_id(blank));
    let lines = screen_lines_sized(&board_app(board.clone(), report, 80, 30), 80, 30);
    assert_eq!(
        line_starting(&lines, "    Flash"),
        "    Flash            — not read (flash/jedec-id failed)"
    );

    // The bridge never synced: every read row points at the port test; the MAC is the one the
    // ROM gave when the bridge was identified.
    let report = run_fixture(
        &board,
        &fixture("esp32s3").fail_at(Step::Connect, crate::probe::ProbeError::Timeout),
    );
    let lines = screen_lines_sized(&board_app(board, report, 80, 30), 80, 30);
    assert_fits(&lines, 80);
    for label in ["Revision", "Crystal", "Flash", "PSRAM", "ROM id"] {
        let line = line_starting(&lines, &format!("    {label} "));
        assert!(
            line.ends_with("— not read (port/uart-bridge failed)"),
            "{line}"
        );
    }
    assert_eq!(
        line_starting(&lines, "    Family"),
        "    Family           ESP32-S3                                              read"
    );
    assert!(
        line_starting(&lines, "    MAC").ends_with(&format!(
            "{S3_MAC}                               read (ROM)"
        )),
        "{}",
        line_starting(&lines, "    MAC")
    );
    assert!(
        line_starting(&lines, "    USB-Serial/JTAG")
            .starts_with("    USB-Serial/JTAG  yes · not connected on this board"),
    );
}

#[test]
fn esp32_shows_no_usb_serial_jtag() {
    let board = esp32_board();
    let report = run_fixture(&board, &fixture("esp32"));
    let lines = screen_lines_sized(&board_app(board, report, 80, 30), 80, 30);
    assert_fits(&lines, 80);
    assert_eq!(
        block(&lines, "  USB", 3),
        [
            "  USB",
            "    USB-Serial/JTAG  no                                               datasheet",
            "    USB OTG          no                                               datasheet",
        ]
    );
    assert_eq!(
        line_starting(&lines, "    CPU"),
        "    CPU              Xtensa LX6 × 2 · ULP-FSM                         datasheet"
    );
    assert_eq!(
        line_starting(&lines, "    Wireless"),
        "    Wireless         Wi-Fi 4 (2.4 GHz) · BT Classic · BLE             datasheet"
    );
    assert_eq!(
        line_starting(&lines, "    └─"),
        "    └─ COM9   CH9102           bridge   primary"
    );
}

#[test]
fn tab_switches_report_and_board() {
    let mut app = bench_app();
    app.resize(80, 16);
    start_run(&mut app);
    // `tab` does nothing while the run is in flight.
    let running = app.clone();
    assert_eq!(app.on_key(press(KeyCode::Tab)), Action::None);
    assert_eq!(app, running);

    app.on_event(Event::RunFinished(c6_report()));
    let lines = screen_lines_sized(&app, 80, 16);
    assert!(lines[0].ends_with("PASS   [Report]  Board"), "{}", lines[0]);
    assert_eq!(lines[2], "  port");
    assert!(
        lines[15].starts_with(" tab board   ↑↓ scroll   enter back to boards   q quit "),
        "{}",
        lines[15]
    );
    // The report scrolls; then to the Board tab, which starts at its own top.
    app.on_key(press(KeyCode::Down));
    app.on_key(press(KeyCode::Down));
    let report_view_row = screen_lines_sized(&app, 80, 16)[2].clone();
    assert_ne!(report_view_row, "  port");
    // A release is not a press (Windows reports both).
    app.on_key(release(KeyCode::Tab));
    assert_eq!(screen_lines_sized(&app, 80, 16)[2], report_view_row);

    assert_eq!(app.on_key(press(KeyCode::Tab)), Action::None);
    let lines = screen_lines_sized(&app, 80, 16);
    assert!(lines[0].ends_with("PASS   Report  [Board]"), "{}", lines[0]);
    assert_eq!(lines[2], "  Chip");
    assert!(
        lines[15].starts_with(" tab report   ↑↓ scroll   enter back to boards   q quit "),
        "{}",
        lines[15]
    );
    // The header names the active tab in bold, the other in gray.
    let buffer = screen_buffer(&app, 80, 16);
    assert!(
        buffer[(73, 0)].modifier.contains(Modifier::BOLD),
        "{:?}",
        buffer[(73, 0)]
    );
    assert_eq!(fg_at(&buffer, 0, "Report"), Color::DarkGray);
    app.on_key(press(KeyCode::Down));
    assert_eq!(screen_lines_sized(&app, 80, 16)[2], lines[3]);

    // Back to Report: where it was left.
    app.on_key(press(KeyCode::Tab));
    assert_eq!(screen_lines_sized(&app, 80, 16)[2], report_view_row);
    app.on_key(press(KeyCode::Tab));
    assert_eq!(screen_lines_sized(&app, 80, 16)[2], lines[3]);

    // Enter still goes back to the list; the next run opens on Report, at the top.
    app.on_key(press(KeyCode::Enter));
    assert_eq!(app.screen, Screen::List);
    assert!(matches!(app.on_key(press(KeyCode::Enter)), Action::Run(_)));
    app.on_event(Event::RunFinished(c6_report()));
    let lines = screen_lines_sized(&app, 80, 16);
    assert!(lines[0].ends_with("[Report]  Board"), "{}", lines[0]);
    assert_eq!(lines[2], "  port");
    assert_eq!(app.on_key(press(KeyCode::Char('q'))), Action::Quit);
}

#[test]
fn board_panel_scrolls_and_wraps_at_60_columns() {
    let board = s3_board();
    let report = run_fixture(&board, &fixture("esp32s3"));
    let lines = screen_lines_sized(&board_app(board.clone(), report.clone(), 60, 40), 60, 40);
    assert_fits(&lines, 60);
    // Too narrow for the port names in the header: they are in the panel.
    assert_eq!(
        lines[0],
        " ESP32-S3  ·  10:51:db:84:14:0c      PASS   Report  [Board]"
    );
    // A long value wraps under itself; the source stays on its first row, flush right.
    assert_eq!(
        block(
            &lines,
            "    CPU              Xtensa LX7 × 2 · ULP-RISC-V  datasheet",
            2
        )[1],
        "                     + ULP-FSM"
    );
    assert_eq!(
        block(
            &lines,
            "    USB-Serial/JTAG  yes · not connected on this  datasheet",
            2
        )[1],
        "                     board"
    );
    assert_eq!(
        line_starting(&lines, "    Flash"),
        "    Flash            16 MB external · JEDEC 0x684018   read"
    );
    // A divergence's detail wraps under `└` too.
    let report26 = run_fixture(&board, &fixture("esp32s3").with_crystal_mhz(26));
    let lines = screen_lines_sized(&board_app(board.clone(), report26, 60, 40), 60, 40);
    assert_fits(&lines, 60);
    assert_eq!(
        block(
            &lines,
            "    Crystal          26 MHz !                          read",
            2
        )[1],
        "                     └ table 40 MHz (variable)"
    );

    // 16 rows: header, rule, 9 body rows, rule, 2 summary rows, rule, footer.
    let body: Vec<String> = board_view::board_lines(&report, &board, 60)
        .iter()
        .map(|line| line.to_string().trim_end().to_owned())
        .collect();
    let mut app = board_app(board, report, 60, 16);
    let lines = screen_lines_sized(&app, 60, 16);
    assert_fits(&lines, 60);
    assert_eq!(lines[2], "  Chip");
    // `enter back to boards` gives way so that the hints and the indicator fit.
    let hidden = body.len() - 9;
    assert!(
        lines[15].starts_with(" tab report   ↑↓ scroll   enter back   q quit "),
        "{}",
        lines[15]
    );
    assert!(
        lines[15].ends_with(&format!(" ↓ {hidden} more")),
        "{}",
        lines[15]
    );
    assert_eq!(lines[15].chars().count(), 59, "flush right: {}", lines[15]);
    app.on_key(press(KeyCode::End));
    let lines = screen_lines_sized(&app, 60, 16);
    assert_fits(&lines, 60);
    assert_eq!(lines[10], "    └─ COM12  CH343            bridge   primary");
    assert!(
        lines[15].ends_with(&format!(" ↑ {hidden} more")),
        "{}",
        lines[15]
    );
    assert_eq!(lines[12], " Tests: 10 passed, 1 skipped, 11 total");
    app.on_key(press(KeyCode::PageUp));
    assert_eq!(screen_lines_sized(&app, 60, 16)[2], body[hidden - 8]);
}

// ---- #25: a board unplugged mid-run ----

#[test]
fn unplug_mid_run_gives_fail_then_not_run() {
    use crate::runner::worker::Job;

    // The C6's CH343 goes away at the flash ID read (its hard reset fails too); the native port
    // still answers.
    let c6 = fixture("esp32c6");
    let probes = move |port: &PortDesc| -> Box<dyn Probe> {
        if port.name == "COM13" {
            Box::new(
                c6.clone()
                    .fail_at(Step::FlashId, crate::probe::ProbeError::Disconnected)
                    .fail_at(Step::HardReset, crate::probe::ProbeError::Disconnected),
            )
        } else {
            Box::new(c6.clone())
        }
    };
    let (tx, events) = mpsc::channel();
    let worker = Worker::spawn(probes, TestTime::default(), c6_enumerate(), tx).unwrap();
    let mut app = App::new();
    let board = crate::runner::worker::testing::c6_board();
    app.screen = Screen::Running(Live::new(board.clone()));
    assert!(worker.submit(Job::Run(board)));

    // The UI keeps taking keys and drawing while the run goes on, without waiting on it.
    let start = Instant::now();
    let mut keys = 0;
    while !matches!(app.screen, Screen::Done { .. }) {
        assert!(start.elapsed() < Duration::from_secs(10), "no report");
        let step = Instant::now();
        assert_eq!(app.on_key(press(KeyCode::Down)), Action::None);
        while let Ok(event) = events.try_recv() {
            app.on_event(event);
        }
        app.tick();
        let _ = screen_lines(&app);
        keys += 1;
        assert!(step.elapsed() < Duration::from_secs(1));
    }
    assert!(keys > 0);
    let Screen::Done { report, .. } = &app.screen else {
        unreachable!();
    };
    let jedec = &report
        .tests()
        .iter()
        .find(|t| t.id == JEDEC_ID)
        .unwrap()
        .outcome;
    assert_eq!(jedec.state(), State::Fail);
    assert_eq!(
        jedec.reason().map(SafeText::as_str),
        Some("port disconnected during the flash ID read")
    );
    let size = &report
        .tests()
        .iter()
        .find(|t| t.id == FLASH_SIZE)
        .unwrap()
        .outcome;
    assert_eq!(
        size.reason().map(SafeText::as_str),
        Some("not run: flash/jedec-id failed")
    );
    assert_eq!(report.exit_code(), 1);
    let lines = screen_lines(&app);
    assert!(lines[0].contains("FAIL"), "{}", lines[0]);
    // Still responsive on Done: q quits.
    assert_eq!(app.on_key(press(KeyCode::Char('q'))), Action::Quit);
    assert!(worker.shutdown(Duration::from_secs(5)));
}
