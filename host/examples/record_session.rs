//! Bench tool for the user manual (issue #64): records one real session as
//! `docs/project/manual/sessions/<name>.json`, which `manual_screenshots` redraws as images.
//!
//! ```powershell
//! cargo run --manifest-path host/Cargo.toml --example record_session -- c6_both --note "C6, CH343 + native"
//! cargo run --manifest-path host/Cargo.toml --example record_session -- two_boards --no-run
//! cargo run --manifest-path host/Cargo.toml --example record_session -- json --port 80:53:e0:0a:5d:ac
//! ```
//!
//! Options: `--port X` (as `devkit-tester --port`: name, USB serial number or MAC), `--no-run`
//! (enumeration and identification only), `--note TEXT`, `--date ISO` (default: now, UTC),
//! `--out DIR` (default: `docs/project/manual/sessions`).
//!
//! Read-only, exactly as the tester: the same enumeration, identification (each bridge is reset
//! into its ROM and back to its app), board choice and run (ADR-009), through `EspflashProbe`.
//! Running it is the user's explicit action (ADR-008 §4). The session keeps the OS port names
//! (`COM13`): it is documentation material, never a fixture of the report format (spec D1).
//! Exit code: 0 recorded, 2 nothing recorded (bad usage, no port list, `--port` matches nothing).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use esp32_devkit_tester::probe::{self, EspflashProbe, Probe};
use esp32_devkit_tester::runner::SystemClock;
use esp32_devkit_tester::transport::{self, PortDesc};
use esp32_devkit_tester::ui::manual::{self, Bench, Capture};

const USAGE: &str = "usage: record_session <name> [--port X] [--no-run] [--note TEXT] \
                     [--date ISO] [--out DIR]   (name: lowercase letters, digits, _)";

struct Args {
    name: String,
    capture: Capture,
    out: PathBuf,
}

fn default_out() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("docs")
        .join("project")
        .join("manual")
        .join("sessions")
}

fn now_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    manual::iso8601_utc(secs)
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut name = None;
    let mut capture = Capture {
        date: String::new(),
        note: String::new(),
        port: None,
        run: true,
    };
    let mut date = None;
    let mut out = default_out();
    while let Some(arg) = args.next() {
        let mut value = |flag: &str| args.next().ok_or(format!("{flag} needs a value"));
        match arg.as_str() {
            "--port" => capture.port = Some(value("--port")?),
            "--note" => capture.note = value("--note")?,
            "--date" => date = Some(value("--date")?),
            "--out" => out = PathBuf::from(value("--out")?),
            "--no-run" => capture.run = false,
            "-h" | "--help" => return Err(String::new()),
            flag if flag.starts_with('-') => return Err(format!("unknown option {flag}")),
            _ if name.is_some() => return Err("one session name only".to_owned()),
            _ => name = Some(arg),
        }
    }
    let name = name.ok_or("a session name is needed")?;
    let valid = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if !valid {
        return Err(format!("bad session name {name:?}"));
    }
    capture.date = date.unwrap_or_else(now_utc);
    Ok(Args { name, capture, out })
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            if !message.is_empty() {
                eprintln!("record_session: {message}");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    probe::quiet_shielded_panics();
    let ports = match transport::enumerate() {
        Ok(ports) => ports,
        Err(error) => {
            eprintln!("record_session: {error}");
            return ExitCode::from(2);
        }
    };
    eprintln!(
        "recording {}: {} port(s); bridges are reset into the ROM and back to their app...",
        args.name,
        ports.len()
    );
    let clock = SystemClock::new();
    let mut probe = |_: &PortDesc| -> Box<dyn Probe> { Box::new(EspflashProbe::new()) };
    let mut enumerate = || transport::enumerate().unwrap_or_default();
    let session = match manual::capture(
        &args.capture,
        ports,
        Bench {
            probe: &mut probe,
            clock: &clock,
            sleeper: &clock,
            enumerate: &mut enumerate,
        },
    ) {
        Ok(session) => session,
        Err(message) => {
            eprintln!("record_session: {message}; nothing recorded");
            return ExitCode::from(2);
        }
    };
    let text = match session.to_json() {
        Ok(text) => text,
        Err(error) => {
            eprintln!("record_session: {error}");
            return ExitCode::from(2);
        }
    };
    let path = args.out.join(format!("{}.json", args.name));
    if let Err(error) =
        std::fs::create_dir_all(&args.out).and_then(|()| std::fs::write(&path, text))
    {
        eprintln!("record_session: {}: {error}", path.display());
        return ExitCode::from(2);
    }
    let found = session
        .identified
        .iter()
        .filter(|result| matches!(result, manual::IdentifiedRecord::Found { .. }))
        .count();
    let report = match &session.report {
        Some(report) => format!(
            "report: {} failed, {} total",
            report["summary"]["failed"], report["summary"]["total"]
        ),
        None => "no report".to_owned(),
    };
    eprintln!(
        "wrote {}: {} port(s), {found}/{} identified, {report}",
        path.display(),
        session.ports.len(),
        session.identified.len()
    );
    ExitCode::SUCCESS
}
