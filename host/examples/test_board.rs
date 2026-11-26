//! Bench tool: runs the v0.1 test catalog on the board behind one port and prints a Jest-style
//! report.
//!
//! ```powershell
//! cargo run --manifest-path host/Cargo.toml --example test_board -- COM13
//! ```
//!
//! Provisional: the TUI (#22) and `--no-tui` (#24) replace it. Read-only (ADR-009): it syncs
//! with the ROM, runs the `port`, `rom`, `efuse` and `flash` suites and always tries the hard
//! reset back to the application at the end. Only one port is tested, so a board with two
//! cables shows `port/usb-serial-jtag` as SKIP through the bridge until grouping (#18) lands.
//!
//! Every text printed from the report is a `SafeText` (printable ASCII only), so nothing the
//! board sends reaches the terminal raw. Exit code: `report.exit_code()` (0 or 1), or 2 when
//! nothing ran (bad usage, port not found, no sync with the ROM).

use std::process::ExitCode;

use esp32_devkit_tester::probe::{EspflashProbe, Probe};
use esp32_devkit_tester::report::{
    BoardInfo, ChipInfo, HardReset, PortInfo, PortKind, Report, Run, SafeText,
};
use esp32_devkit_tester::runner::{self, SystemClock};
use esp32_devkit_tester::transport::{self, PortClass, PortDesc};

fn main() -> ExitCode {
    let Some(name) = std::env::args().nth(1) else {
        eprintln!("usage: test_board <port>   (e.g. COM13, /dev/ttyACM0)");
        return ExitCode::from(2);
    };
    let ports = match transport::enumerate() {
        Ok(ports) => ports,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let Some(port) = find(&ports, &name) else {
        eprintln!("port {} not found; ports seen by the OS:", safe(&name));
        for port in &ports {
            eprintln!("  {} {:?}", safe(&port.name), transport::classify(port));
        }
        return ExitCode::from(2);
    };
    let class = transport::classify(port);
    if !class.is_probe_candidate() {
        eprintln!("port {} is not an ESP32 port: {class:?}", safe(&port.name));
        return ExitCode::from(2);
    }

    let mut probe = EspflashProbe::new();
    let clock = SystemClock::new();
    let sync = runner::sync_port(&mut probe, port);
    let chip = match &sync.mac {
        Ok(_) => probe.chip(),
        Err(error) => Err(error.clone()),
    };
    let chip = match chip {
        Ok(chip) => chip,
        Err(error) => {
            eprintln!("no run: could not identify the chip ({error})");
            if let Err(error) = probe.hard_reset() {
                eprintln!("hard reset not done: {error}");
            }
            return ExitCode::from(2);
        }
    };
    let caps = chip.caps();
    let board_mac = sync.mac.as_ref().ok().copied();
    let has_bridge = matches!(class, PortClass::Bridge { .. });

    let board = BoardInfo {
        chip: Some(ChipInfo {
            chip,
            description: None,
            // Read by `rom/revision`; this example does not copy it back into the board info.
            revision: None,
        }),
        mac: board_mac.map(|mac| SafeText::from_host(&mac.to_string())),
        ports: port_info(port, class).into_iter().collect(),
    };
    let mut run = Run::new(
        env!("CARGO_PKG_VERSION"),
        board,
        runner::catalog(caps, has_bridge),
    );
    for test in runner::port_tests(caps, board_mac, std::slice::from_ref(&sync)) {
        if let Err(error) = run.finish(test.id, test.outcome) {
            eprintln!("runner bug: {error}");
        }
    }
    if let Err(interrupted) = runner::run_suites(&mut probe, caps, &clock, &mut run) {
        eprintln!("{interrupted}");
    }
    let reset = match probe.hard_reset() {
        Ok(()) => HardReset::done(),
        Err(error) => HardReset::not_done(&error.to_string()),
    };
    let report = run.complete(reset);

    println!("{} on {}", caps.name, safe(&port.name));
    print_report(&report);
    ExitCode::from(report.exit_code())
}

fn find<'a>(ports: &'a [PortDesc], name: &str) -> Option<&'a PortDesc> {
    ports.iter().find(|port| port.name == name).or_else(|| {
        ports
            .iter()
            .find(|port| port.name.eq_ignore_ascii_case(name))
    })
}

/// OS or user text, sanitized before printing.
fn safe(text: &str) -> SafeText {
    SafeText::from_board(text)
}

/// The tested port for the report; `None` for a port without USB identity.
fn port_info(port: &PortDesc, class: PortClass) -> Option<PortInfo> {
    let usb = port.usb.as_ref()?;
    let kind = match class {
        PortClass::Native => PortKind::Native,
        _ => PortKind::Bridge,
    };
    Some(PortInfo {
        kind,
        vid: usb.vid,
        pid: usb.pid,
        bridge_model: class.bridge_model().map(SafeText::from_host),
        usb_serial: usb.serial.as_deref().map(SafeText::from_board),
        primary: true,
    })
}

/// One line per test, then the summary and the hard reset. Only `SafeText` and numbers.
fn print_report(report: &Report) {
    if let Some(mac) = &report.board().mac {
        println!("MAC {mac}");
    }
    println!();
    for test in report.tests() {
        let outcome = &test.outcome;
        let state = outcome.state().as_str().to_ascii_uppercase();
        println!("{state} {} ({} ms)", test.id, outcome.duration_ms());
        if let Some(expected) = outcome.expected() {
            println!("     expected: {expected}");
        }
        if let Some(read) = outcome.read() {
            println!("     read:     {read}");
        }
        if let Some(reason) = outcome.reason() {
            println!("     reason:   {reason}");
        }
        if let Some(finding) = outcome.finding() {
            println!(
                "     finding:  {} ({}) table {} vs read {}; source: {}",
                finding.field(),
                finding.kind(),
                finding.table(),
                finding.read(),
                finding.source()
            );
        }
    }
    println!();
    println!("{}", report.summary());
    match report.hard_reset().reason() {
        None => println!("Hard reset: done"),
        Some(reason) => println!("Hard reset: not done ({reason})"),
    }
}
