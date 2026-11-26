//! `App` and screen tests: no terminal (ratatui's `TestBackend`) and no board (fake probes on the
//! runner's thread, through `runner::worker::testing`).

use std::sync::mpsc;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyEventState;

use super::*;
use crate::runner::worker::testing::{
    C6_MAC, Log, TestTime, c6_enumerate, c6_probes, ch343, collect_until, gate, native, usb,
};
use crate::runner::{BOARD_TESTS, CHIP_IDENTITY, UART_BRIDGE, USB_SERIAL_JTAG};

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
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| draw(frame, app)).unwrap();
    let buffer = terminal.backend().buffer();
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

    app.on_event(Event::RunCancelled);
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
    assert!(lines[0].ends_with("PASS"), "{}", lines[0]);
    let text = lines.join("\n");
    for id in BOARD_TESTS {
        assert!(text.contains(&format!("✓ {:<20}", id.name)), "{id}\n{text}");
    }
    assert_eq!(
        line_starting(&lines, " Tests:"),
        " Tests: 11 passed, 11 total"
    );
    assert_eq!(
        line_starting(&lines, " ✓ board"),
        " ✓ board back to its app"
    );
    assert!(
        lines
            .iter()
            .all(|line| line.trim_end().chars().count() <= 80)
    );
    assert_eq!(lines[23], " enter back to boards   q quit");
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
    for event in collect_until(&events, |event| *event == Event::RunCancelled) {
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
