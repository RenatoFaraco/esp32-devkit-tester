//! Command-line tests: no terminal and no board (fake probes on the runner's thread).

use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::thread;

use serde_json::Value;

use super::*;
use crate::chips::Chip;
use crate::platform::Wsl;
use crate::probe::fake::{FakeProbe, Step};
use crate::probe::{Probe, ProbeError};
use crate::report::json::spec_examples;
use crate::runner::worker::testing::{
    C6_MAC, Log, TestTime, c6_board, c6_enumerate, c6_probes, ch343, gate, native, usb,
};

const ESP32_MAC: &str = "1c:c3:ab:d2:25:b4";
const C3_MAC: &str = "80:53:e0:0a:5d:ac";
const S3_MAC: &str = "10:51:db:84:14:0c";

fn args(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

fn mac(text: &str) -> Mac {
    Mac::parse(text).unwrap()
}

fn chip(target: &str) -> Chip {
    Chip::from_target(target).unwrap()
}

fn fixture(target: &str) -> FakeProbe {
    FakeProbe::from_fixture(target).unwrap()
}

fn c6_native() -> PortDesc {
    native("COM11", "E8:F6:0A:F7:FF:D4")
}

fn ch9102(name: &str) -> PortDesc {
    usb(name, 0x1A86, 0x55D4, "5B53026573")
}

fn ch340k(name: &str) -> PortDesc {
    usb(name, 0x1A86, 0x7522, "6")
}

/// The JTAG channel of an FT2232 (interface 0): listed, never opened.
fn ft2232_jtag(name: &str) -> PortDesc {
    let mut port = usb(name, 0x0403, 0x6010, "FT2232A");
    if let Some(usb) = port.usb.as_mut() {
        usb.interface = Some(0);
    }
    port
}

/// What one run printed and returned.
#[derive(Debug)]
struct Ran {
    code: u8,
    stdout: String,
    stderr: String,
}

/// [`execute`] over fake probes, with `ports` as the enumeration at start.
fn execute_with<P, E>(
    options: RunOptions,
    ports: Vec<PortDesc>,
    probes: P,
    enumerate: E,
    interrupted: &AtomicBool,
) -> Ran
where
    P: FnMut(&PortDesc) -> Box<dyn Probe> + Send + 'static,
    E: FnMut() -> Vec<PortDesc> + Send + 'static,
{
    let (tx, events) = mpsc::channel();
    let worker = Worker::spawn(probes, TestTime::default(), enumerate, tx).unwrap();
    let session = Session {
        ports,
        worker: &worker,
        events: &events,
        interrupted,
        stderr_unicode: true,
        prepare_colors: false,
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute(
        &options,
        &session,
        TextStyle::plain(),
        &mut stdout,
        &mut stderr,
    );
    assert!(worker.shutdown(Duration::from_secs(5)));
    Ran {
        code,
        stdout: String::from_utf8(stdout).unwrap(),
        stderr: String::from_utf8(stderr).unwrap(),
    }
}

fn json() -> RunOptions {
    RunOptions {
        json: true,
        port: None,
    }
}

fn text() -> RunOptions {
    RunOptions::default()
}

/// Every port answers as the fixture its name maps to (`COM13`/`COM11` the C6, `COM10` the C3
/// with the MAC of its native port's serial, anything else the C6).
fn bench_probes() -> impl FnMut(&PortDesc) -> Box<dyn Probe> + Send + 'static {
    let c6 = fixture("esp32c6");
    let c3 = fixture("esp32c3").with_mac(mac(C3_MAC));
    move |port: &PortDesc| -> Box<dyn Probe> {
        match port.name.as_str() {
            "COM10" | "COM6" => Box::new(c3.clone()),
            _ => Box::new(c6.clone()),
        }
    }
}

// ---- arguments ----

#[test]
fn parses_flags_and_port_forms() {
    assert_eq!(parse_args(args(&[])), Ok(Command::Tui));
    assert_eq!(parse_args(args(&["--help"])), Ok(Command::Help));
    assert_eq!(parse_args(args(&["-h"])), Ok(Command::Help));
    assert_eq!(parse_args(args(&["--version"])), Ok(Command::Version));
    assert_eq!(parse_args(args(&["-V"])), Ok(Command::Version));
    // Help wins over anything else that is valid.
    assert_eq!(parse_args(args(&["--no-tui", "--help"])), Ok(Command::Help));
    assert_eq!(
        parse_args(args(&["--no-tui"])),
        Ok(Command::Run(RunOptions {
            json: false,
            port: None
        }))
    );
    let port = |value: &str| {
        Ok(Command::Run(RunOptions {
            json: false,
            port: Some(value.to_owned()),
        }))
    };
    // A port name, a USB serial number or a MAC; separate or after `=`; any order.
    assert_eq!(
        parse_args(args(&["--no-tui", "--port", "COM13"])),
        port("COM13")
    );
    assert_eq!(
        parse_args(args(&["--port", "5D28088406", "--no-tui"])),
        port("5D28088406")
    );
    assert_eq!(
        parse_args(args(&["--no-tui", "--port=e8:f6:0a:f7:ff:d4"])),
        port("e8:f6:0a:f7:ff:d4")
    );
    assert_eq!(
        parse_args(args(&["--no-tui", "--port", "/dev/ttyACM0"])),
        port("/dev/ttyACM0")
    );
    // --port alone, twice, empty or without a value is a usage error.
    for bad in [
        &["--port", "COM13"][..],
        &["--no-tui", "--port"],
        &["--no-tui", "--port="],
        &["--no-tui", "--port", "COM1", "--port", "COM2"],
        &["--no-tui=yes"],
    ] {
        assert!(parse_args(args(bad)).is_err(), "{bad:?}");
    }
}

#[test]
fn json_implies_no_tui() {
    let expected = Ok(Command::Run(RunOptions {
        json: true,
        port: None,
    }));
    assert_eq!(parse_args(args(&["--json"])), expected);
    assert_eq!(parse_args(args(&["--no-tui", "--json"])), expected);
    assert_eq!(
        parse_args(args(&["--json", "--port", "COM13"])),
        Ok(Command::Run(RunOptions {
            json: true,
            port: Some("COM13".to_owned()),
        }))
    );
}

#[test]
fn unknown_flag_is_usage_error_exit_2() {
    assert_eq!(
        parse_args(args(&["--frobnicate"])),
        Err(UsageError("unknown argument '--frobnicate'".to_owned()))
    );
    assert!(parse_args(args(&["COM13"])).is_err());
    // An escape in the argument is quoted back sanitized.
    let Err(error) = parse_args(args(&["--\u{1b}[2J"])) else {
        panic!("accepted");
    };
    assert!(!error.0.contains('\u{1b}'), "{error}");
    // The whole program: code 2 before anything else happens.
    assert_eq!(main(args(&["--frobnicate"])), EXIT_NO_RUN);
}

// ---- choosing the board ----

#[test]
fn no_board_exits_2_with_empty_stdout() {
    let never = AtomicBool::new(false);
    let log = Log::default();
    let ran = execute_with(json(), Vec::new(), c6_probes(&log, None), Vec::new, &never);
    assert_eq!(ran.code, 2);
    assert_eq!(ran.stdout, "");
    assert!(
        ran.stderr.contains("no ESP32 board found"),
        "{}",
        ran.stderr
    );

    // A port that is never tested is named with its reason; still nothing to run.
    let ran = execute_with(
        text(),
        vec![ft2232_jtag("COM20")],
        c6_probes(&log, None),
        Vec::new,
        &never,
    );
    assert_eq!((ran.code, ran.stdout.as_str()), (2, ""));
    assert!(
        ran.stderr.contains("COM20 not tested: FTDI JTAG channel"),
        "{}",
        ran.stderr
    );

    // --port that matches nothing.
    let options = RunOptions {
        json: true,
        port: Some("COM99".to_owned()),
    };
    let ran = execute_with(
        options,
        vec![c6_native()],
        c6_probes(&log, None),
        Vec::new,
        &never,
    );
    assert_eq!((ran.code, ran.stdout.as_str()), (2, ""));
    assert!(
        ran.stderr.contains("no serial port matches 'COM99'"),
        "{}",
        ran.stderr
    );
    assert!(
        ran.stderr.contains("COM11  serial E8:F6:0A:F7:FF:D4"),
        "{}",
        ran.stderr
    );
    assert!(log.lock().unwrap().is_empty(), "a port was opened");
}

#[test]
fn several_boards_without_port_list_on_stderr_exit_2() {
    let never = AtomicBool::new(false);
    let ports = vec![
        ch343("COM13"),
        c6_native(),
        ch340k("COM10"),
        native("COM6", "80:53:E0:0A:5D:AC"),
    ];
    let ran = execute_with(json(), ports, bench_probes(), Vec::new, &never);
    assert_eq!(ran.code, 2);
    assert_eq!(ran.stdout, "");
    assert_eq!(
        ran.stderr,
        "identifying 2 bridge ports; each board is reset back to its app…\n\
         2 boards found; choose one with --port:\n  \
         ESP32-C6  e8:f6:0a:f7:ff:d4   --port e8:f6:0a:f7:ff:d4   (COM13, COM11)\n  \
         ESP32-C3  80:53:e0:0a:5d:ac   --port 80:53:e0:0a:5d:ac   (COM10, COM6)\n"
    );

    // A board with no native port is picked by its bridge's serial number (or its name).
    let boards = [
        Board::new(mac(ESP32_MAC), Some(chip("esp32")), vec![ch9102("COM9")]).unwrap(),
        Board::new(mac(C3_MAC), Some(chip("esp32c3")), vec![ch340k("COM10")]).unwrap(),
    ];
    assert_eq!(
        board_list(&boards),
        "2 boards found; choose one with --port:\n  \
         ESP32     1c:c3:ab:d2:25:b4   --port 5B53026573   (COM9)\n  \
         ESP32-C3  80:53:e0:0a:5d:ac   --port COM10        (COM10)\n"
    );
}

#[test]
fn port_selects_by_name_serial_or_mac() {
    let ports = vec![
        ch343("COM13"),
        c6_native(),
        ch340k("COM10"),
        ft2232_jtag("COM20"),
        usb("COM30", 0x10C4, 0xEA60, "0001"),
        usb("COM31", 0x10C4, 0xEA60, "0001"),
    ];
    let pick = |wanted: &str| select_port(&ports, wanted).map(|port| port.name);
    // The name, in any ASCII case.
    assert_eq!(pick("COM13"), Ok("COM13".to_owned()));
    assert_eq!(pick("com13"), Ok("COM13".to_owned()));
    // The USB serial number, in any case.
    assert_eq!(pick("5d28088406"), Ok("COM13".to_owned()));
    // The MAC, in any case and separator, against the native port's serial number.
    for wanted in [
        "e8:f6:0a:f7:ff:d4",
        "E8-F6-0A-F7-FF-D4",
        "e8f60af7ffd4",
        " E8:F6:0A:F7:FF:D4 ",
    ] {
        assert_eq!(pick(wanted), Ok("COM11".to_owned()), "{wanted}");
    }
    // A serial number the OS made up (CH340K) is not one.
    assert!(pick("6").is_err());
    assert_eq!(pick("COM10"), Ok("COM10".to_owned()));
    // Never tested, ambiguous, or nothing at all.
    let error = pick("COM20").unwrap_err();
    assert!(
        error.contains("COM20 is never tested: FTDI JTAG channel"),
        "{error}"
    );
    let error = pick("0001").unwrap_err();
    assert!(
        error.contains("matches several ports (COM30, COM31)"),
        "{error}"
    );
    let error = pick("80:53:e0:0a:5d:ac").unwrap_err();
    assert!(error.starts_with("no serial port matches"), "{error}");

    // --port <MAC> runs that native port alone, without identifying anything.
    let never = AtomicBool::new(false);
    let log = Log::default();
    let options = RunOptions {
        json: true,
        port: Some("E8-F6-0A-F7-FF-D4".to_owned()),
    };
    let ran = execute_with(
        options,
        vec![ch343("COM13"), c6_native()],
        c6_probes(&log, None),
        || vec![ch343("COM13"), native("COM11", "E8:F6:0A:F7:FF:D4")],
        &never,
    );
    assert_eq!(ran.code, 0, "{}", ran.stderr);
    assert_eq!(ran.stderr, "testing e8:f6:0a:f7:ff:d4 (COM11)…\n");
    let value: Value = serde_json::from_str(&ran.stdout).unwrap();
    assert_eq!(value["board"]["ports"].as_array().map(Vec::len), Some(1));
    assert_eq!(value["board"]["ports"][0]["kind"], "native");
    assert_eq!(value["tests"][0]["name"], "usb-serial-jtag");
    assert!(log.lock().unwrap().iter().all(|(name, _)| name == "COM11"));

    // --port <bridge> is identified first (one session), then run alone.
    let log = Log::default();
    let options = RunOptions {
        json: false,
        port: Some("5D28088406".to_owned()),
    };
    let ran = execute_with(
        options,
        vec![ch343("COM13"), c6_native()],
        c6_probes(&log, None),
        Vec::new,
        &never,
    );
    assert_eq!(ran.code, 0, "{}", ran.stderr);
    assert_eq!(
        ran.stderr,
        "identifying COM13; the board is reset back to its app…\n\
         testing ESP32-C6 e8:f6:0a:f7:ff:d4 (COM13)…\n"
    );
    assert!(
        ran.stdout
            .starts_with(" ESP32-C6  |  e8:f6:0a:f7:ff:d4  |  COM13 "),
        "{}",
        ran.stdout
    );
    assert!(log.lock().unwrap().iter().all(|(name, _)| name == "COM13"));
}

// ---- running ----

#[test]
fn json_run_prints_one_document_and_exits_0() {
    let never = AtomicBool::new(false);
    let log = Log::default();
    let ran = execute_with(
        json(),
        vec![ch343("COM13"), c6_native()],
        c6_probes(&log, None),
        c6_enumerate(),
        &never,
    );
    assert_eq!(ran.code, 0, "{}", ran.stderr);
    assert_eq!(
        ran.stderr,
        "identifying 1 bridge port; each board is reset back to its app…\n\
         testing ESP32-C6 e8:f6:0a:f7:ff:d4 (COM13 + COM11)…\n"
    );
    assert!(ran.stdout.ends_with('\n'));
    assert_eq!(ran.stdout.matches('\n').count(), 1);
    assert!(ran.stdout.is_ascii());
    let value: Value = serde_json::from_str(&ran.stdout).unwrap();
    assert_eq!(value["summary"]["total"], 11);
    assert_eq!(value["summary"]["failed"], 0);
    assert_eq!(value["board"]["mac"], C6_MAC);
    // No port name anywhere in the document (D1).
    assert!(!ran.stdout.contains("COM1"), "{}", ran.stdout);
}

/// The C6 through its CH343 and its native port; the CH343 session dies at the flash ID read.
fn unplugging_probes() -> impl FnMut(&PortDesc) -> Box<dyn Probe> + Send + 'static {
    let c6 = fixture("esp32c6");
    move |port: &PortDesc| -> Box<dyn Probe> {
        if port.name == "COM13" {
            Box::new(
                c6.clone()
                    .fail_at(Step::FlashId, ProbeError::Disconnected)
                    .fail_at(Step::HardReset, ProbeError::Disconnected),
            )
        } else {
            Box::new(c6.clone())
        }
    }
}

#[test]
fn unplug_mid_run_gives_fail_then_not_run() {
    let never = AtomicBool::new(false);
    let ran = execute_with(
        text(),
        vec![ch343("COM13"), c6_native()],
        unplugging_probes(),
        c6_enumerate(),
        &never,
    );
    assert_eq!(ran.code, 1, "{}", ran.stderr);
    let lines: Vec<&str> = ran.stdout.lines().collect();
    assert!(lines[0].ends_with("FAIL"), "{}", lines[0]);
    let at = lines
        .iter()
        .position(|line| line.starts_with("    x jedec-id"))
        .unwrap_or_else(|| panic!("{}", ran.stdout));
    assert_eq!(
        lines[at + 1].trim(),
        "\\ port disconnected during the flash ID read"
    );
    assert!(lines[at + 2].starts_with("    - size"), "{}", lines[at + 2]);
    assert_eq!(lines[at + 3].trim(), "\\ not run: flash/jedec-id failed");
    assert!(
        ran.stdout
            .contains(" Tests: 1 failed, 9 passed, 1 skipped, 11 total"),
        "{}",
        ran.stdout
    );

    // The same through --json: code 1 and `failed > 0` (spec §1 invariant).
    let ran = execute_with(
        json(),
        vec![ch343("COM13"), c6_native()],
        unplugging_probes(),
        c6_enumerate(),
        &never,
    );
    assert_eq!(ran.code, 1);
    let value: Value = serde_json::from_str(&ran.stdout).unwrap();
    assert_eq!(value["summary"]["failed"], 1);
    let size = value["tests"]
        .as_array()
        .and_then(|tests| tests.iter().find(|test| test["name"] == "size"))
        .unwrap();
    assert_eq!(size["reason"], "not run: flash/jedec-id failed");
}

#[test]
fn ctrl_c_mid_run_exits_2_with_empty_stdout_after_the_reset() {
    let interrupted = AtomicBool::new(false);
    let log = Log::default();
    let (held, gate) = gate(Step::Revision);
    let ran = thread::scope(|scope| {
        let running = scope.spawn(|| {
            execute_with(
                json(),
                vec![c6_native()],
                c6_probes(&log, Some(held)),
                Vec::new,
                &interrupted,
            )
        });
        assert_eq!(
            gate.reached.recv_timeout(Duration::from_secs(10)),
            Ok(Step::Revision)
        );
        // What the ctrlc handler does, and nothing more.
        interrupted.store(true, Ordering::SeqCst);
        // Let the loop see the flag (and cancel) before the probe goes on.
        thread::sleep(POLL * 4);
        gate.release.send(()).unwrap();
        running.join().unwrap()
    });
    assert_eq!(ran.code, 2);
    assert_eq!(ran.stdout, "");
    assert_eq!(
        ran.stderr,
        "testing e8:f6:0a:f7:ff:d4 (COM11)…\n\
         cancelling… the board still gets its reset back to its app\n\
         cancelled: board back to its app\n"
    );
    // The session got its hard reset; nothing was read after the cancel.
    let calls: Vec<Step> = log.lock().unwrap().iter().map(|(_, step)| *step).collect();
    assert_eq!(calls.last(), Some(&Step::HardReset));
    assert!(!calls.contains(&Step::CrystalMhz), "{calls:?}");
}

#[test]
fn wsl1_is_refused_before_any_port_is_opened() {
    let mut started = false;
    let mut stderr = Vec::new();
    let code = launch(
        &Command::Run(json()),
        Platform::Wsl(Wsl::V1),
        &mut stderr,
        &mut |_| {
            started = true;
            0
        },
    );
    assert_eq!(code, 2);
    assert!(!started, "ports were listed under WSL1");
    assert_eq!(
        String::from_utf8(stderr).unwrap(),
        "devkit-tester: WSL1 is not supported: it has no USB/serial devices. \
         Use Windows natively, or WSL2 with usbipd-win.\n"
    );
    // The TUI too; WSL2 goes on.
    let mut stderr = Vec::new();
    assert_eq!(
        launch(
            &Command::Tui,
            Platform::Wsl(Wsl::V1),
            &mut stderr,
            &mut |_| 0
        ),
        2
    );
    assert_eq!(
        launch(
            &Command::Tui,
            Platform::Wsl(Wsl::V2),
            &mut stderr,
            &mut |_| 7
        ),
        7
    );
}

// ---- JSON against the spec ----

/// Drops what a fake run cannot reproduce: durations, the binary's version and the chip
/// description (the probe has no description fact yet: always `null`).
fn comparable(mut document: Value) -> Value {
    if let Some(tests) = document["tests"].as_array_mut() {
        for test in tests {
            test["duration_ms"] = Value::Null;
        }
    }
    document["tool"]["version"] = Value::Null;
    document["board"]["chip"]["description"] = Value::Null;
    document
}

/// `board` run by the real runner, every session answered by `probe`.
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

fn esp32_board() -> Board {
    Board::new(mac(ESP32_MAC), Some(chip("esp32")), vec![ch9102("COM9")]).unwrap()
}

#[test]
fn report_from_fixture_matches_spec_shape() {
    let report = run_fixture(&esp32_board(), &fixture("esp32"));
    let ours: Value = serde_json::from_str(&to_json_line(&report).unwrap()).unwrap();
    assert_eq!(ours["tool"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(ours["board"]["chip"]["description"].is_null());
    assert_eq!(comparable(ours), comparable(spec_examples::example("skip")));

    // The finding example (8.4) is the same board with a 26 MHz crystal.
    let report = run_fixture(&esp32_board(), &fixture("esp32").with_crystal_mhz(26));
    let ours: Value = serde_json::from_str(&to_json_line(&report).unwrap()).unwrap();
    assert_eq!(
        comparable(ours),
        comparable(spec_examples::example("finding"))
    );
}

// ---- the text ----

fn c6_report() -> Report {
    let log = Log::default();
    let time = TestTime::default();
    let mut enumerate = c6_enumerate();
    runner::run_board(
        &mut c6_probes(&log, None),
        &c6_board(),
        &time,
        &time,
        &mut enumerate,
        &mut |_| {},
    )
}

fn s3_board() -> Board {
    Board::new(mac(S3_MAC), Some(chip("esp32s3")), vec![ch343("COM12")]).unwrap()
}

/// A pass, a finding and a cut session, each with its board.
fn reports() -> Vec<(Report, Board)> {
    let s3 = fixture("esp32s3")
        .fail_at(Step::FlashId, ProbeError::Disconnected)
        .fail_at(Step::HardReset, ProbeError::Disconnected);
    vec![
        (c6_report(), c6_board()),
        (
            run_fixture(&esp32_board(), &fixture("esp32").with_crystal_mhz(26)),
            esp32_board(),
        ),
        (run_fixture(&s3_board(), &s3), s3_board()),
    ]
}

fn unicode() -> TextStyle {
    TextStyle {
        color: false,
        unicode: true,
        width: 80,
    }
}

#[test]
fn text_report_matches_the_mockup() {
    let text = text_report(&c6_report(), &c6_board(), unicode());
    let lines: Vec<&str> = text.lines().collect();
    let rule = format!(" {}", "─".repeat(78));
    assert!(
        lines[0].starts_with(" ESP32-C6  ·  e8:f6:0a:f7:ff:d4  ·  COM13 + COM11 "),
        "{}",
        lines[0]
    );
    assert!(
        lines[0].ends_with(" PASS") && lines[0].chars().count() == 79,
        "{}",
        lines[0]
    );
    assert_eq!(lines[1], rule);
    assert_eq!(lines[2], "  port");
    assert!(lines[3].starts_with("    ✓ uart-bridge          e8:f6:0a:f7:ff:d4 "));
    assert!(lines[3].ends_with(" 0 ms"), "{}", lines[3]);
    let board = lines.iter().position(|line| *line == "  Board").unwrap();
    assert_eq!(lines[board - 1], rule);
    assert_eq!(
        lines[board + 1..board + 5],
        [
            "    Revision v0.2 · RISC-V × 1 + LP RISC-V · 160 MHz · crystal 40 MHz",
            "    Flash 16 MB external (0x684018) · PSRAM none in package",
            "    Wi-Fi 6 (2.4 GHz) · BLE · IEEE 802.15.4 · USB-Serial/JTAG tested ✓",
            rule.as_str(),
        ]
    );
    assert_eq!(
        lines[board + 5..],
        [" Tests: 11 passed, 11 total", " ✓ board back to its app"]
    );
    assert!(text.ends_with("app\n"));
}

#[test]
fn text_report_has_no_ansi_when_not_a_tty() {
    // Redirected: no colour, whatever NO_COLOR says.
    for no_color in [None, Some(OsString::new())] {
        let style = TextStyle::decide(false, no_color, true, Some(120));
        assert_eq!(style, TextStyle::plain());
    }
    for (report, board) in reports() {
        let text = text_report(&report, &board, TextStyle::plain());
        assert!(!text.contains('\u{1b}'), "{text}");
    }
    // On a terminal: colour unless NO_COLOR is set (an empty NO_COLOR does not count).
    assert!(TextStyle::decide(true, None, true, Some(80)).color);
    assert!(TextStyle::decide(true, Some(OsString::new()), true, Some(80)).color);
    assert!(!TextStyle::decide(true, Some(OsString::from("1")), true, Some(80)).color);
    let colored = TextStyle::decide(true, None, true, Some(80));
    let text = text_report(&c6_report(), &c6_board(), colored);
    assert!(text.contains("\u{1b}[1;32mPASS\u{1b}[0m"), "{text}");
    assert!(text.contains("\u{1b}[32m✓\u{1b}[0m"), "{text}");
    // Colour codes are the only escapes, and only bytes of the layout's own styles.
    let stripped = strip_ansi(&text);
    assert_eq!(stripped, text_report(&c6_report(), &c6_board(), unicode()));
}

/// `text` without its `ESC [ … m` sequences.
fn strip_ansi(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn text_report_uses_ascii_glyphs_when_not_utf8() {
    // A terminal whose locale is not UTF-8, and any redirection, get ASCII.
    assert!(!TextStyle::decide(true, None, false, Some(80)).unicode);
    assert!(!TextStyle::decide(false, None, true, None).unicode);
    assert!(TextStyle::decide(true, None, true, Some(80)).unicode);

    for (report, board) in reports() {
        let text = text_report(&report, &board, TextStyle::plain());
        assert!(text.is_ascii(), "{text}");
    }
    let text = text_report(&c6_report(), &c6_board(), TextStyle::plain());
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with(" ESP32-C6  |  e8:f6:0a:f7:ff:d4  |  COM13 + COM11 "));
    assert_eq!(lines[1], format!(" {}", "-".repeat(78)));
    assert!(lines[3].starts_with("    + uart-bridge          e8:f6:0a:f7:ff:d4 "));
    assert!(
        text.contains("    Revision v0.2 | RISC-V x 1 + LP RISC-V | 160 MHz | crystal 40 MHz\n")
    );
    assert!(text.contains("USB-Serial/JTAG tested +\n"));
    assert!(text.ends_with(" + board back to its app\n"));

    // The finding and the SKIP of the ESP32.
    let (report, board) = &reports()[1];
    let text = text_report(report, board, TextStyle::plain());
    assert!(text.contains("    - usb-serial-jtag"), "{text}");
    assert!(text.contains("\\ table 40 MHz | read 26 MHz"), "{text}");
    assert!(text.contains("crystal 26 MHz !"), "{text}");

    // `…` becomes `...` without widening the line: it takes the blanks after it.
    let line = Line::from(vec![Span::raw("abc…"), Span::raw("   "), Span::raw("4 ms")]);
    assert_eq!(render_line(&line, TextStyle::plain()), "abc... 4 ms");
}

#[test]
fn text_fits_80_columns() {
    for (report, board) in reports() {
        for style in [TextStyle::plain(), unicode()] {
            let text = text_report(&report, &board, style);
            for line in text.lines() {
                assert!(
                    line.chars().count() <= 80,
                    "{} columns: {line}",
                    line.chars().count()
                );
            }
        }
    }
    // A card line too long for the width breaks at a `·`, never inside a part.
    let (report, board) = &reports()[2];
    let text = text_report(report, board, unicode());
    assert!(
        text.contains(
            "\n    Revision v0.2 · Xtensa LX7 × 2 + ULP-RISC-V + ULP-FSM · 240 MHz\n      \
             crystal 40 MHz\n"
        ),
        "{text}"
    );
    // A wide terminal still gets the 80-column page; a narrow one its own width.
    assert_eq!(TextStyle::decide(true, None, true, Some(200)).width, 80);
    assert_eq!(TextStyle::decide(true, None, true, Some(60)).width, 60);
    assert_eq!(
        TextStyle::decide(true, None, true, Some(0)).width,
        MIN_WIDTH
    );
    let (report, board) = &reports()[2];
    for line in text_report(
        report,
        board,
        TextStyle {
            width: 60,
            ..unicode()
        },
    )
    .lines()
    {
        assert!(line.chars().count() <= 60, "{line}");
    }
}
