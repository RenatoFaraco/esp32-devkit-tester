//! `App` and screen tests: no terminal (ratatui's `TestBackend`) and no board (fake probes on the
//! runner's thread, through `runner::worker::testing`).

use std::sync::mpsc;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyEventState;

use super::*;
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
    assert!(lines[0].ends_with("PASS"), "{}", lines[0]);
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
    app.screen = Screen::Done { board, report };
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
fn long_skip_reason_wraps_instead_of_being_cut() {
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
    assert!(lines[0].ends_with("FAIL"), "{}", lines[0]);

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
        first.starts_with(" ↑↓ scroll   enter back to boards   q quit "),
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
    assert_eq!(view(&tall, 40).0, " enter back to boards   q quit");
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
        assert!(lines[0].ends_with("PASS"), "{key:?}");
        assert!(lines[1].starts_with(" ───"), "{key:?}");
        assert!(lines[11].starts_with(" ───"), "{key:?}");
        assert_eq!(
            lines[12], " Tests: 10 passed, 1 skipped, 11 total",
            "{key:?}"
        );
        assert_eq!(lines[13], " ✓ board back to its app", "{key:?}");
        assert!(lines[14].starts_with(" ───"), "{key:?}");
        assert!(
            lines[15].starts_with(" ↑↓ scroll   enter back to boards"),
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
