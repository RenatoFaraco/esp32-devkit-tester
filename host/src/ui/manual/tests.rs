//! The manual's pipeline with fake probes: recording, the session file, the screens.
//!
//! `host/tests/fixtures/manual_session_c6.json` is the synthetic session the integration test
//! `tests/manual_images.rs` draws. It is recorded here from the C6 fixture; after a change that
//! alters it on purpose, regenerate it with
//! `$env:DKT_BLESS_MANUAL = 1; cargo test --manifest-path host/Cargo.toml --locked manual`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::*;
use crate::probe::Probe;
use crate::probe::fake::FakeProbe;
use crate::runner::worker::testing::{c6_enumerate, ch343, native, usb};
use crate::runner::{Clock, Sleeper};

/// Every reading of the clock is 7 ms after the previous one; sleeping advances it exactly.
#[derive(Default)]
struct Ticking {
    now: AtomicU64,
}

impl Clock for Ticking {
    fn now_ms(&self) -> u64 {
        self.now.fetch_add(7, Ordering::SeqCst)
    }
}

impl Sleeper for Ticking {
    fn sleep(&self, duration: Duration) {
        let ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        self.now.fetch_add(ms, Ordering::SeqCst);
    }
}

/// Probes answering as the board of each fixture: ports named in `c3` are the C3, every other
/// port the C6.
fn probes(c3: &'static [&'static str]) -> impl FnMut(&PortDesc) -> Box<dyn Probe> {
    let c6 = FakeProbe::from_fixture("esp32c6").unwrap();
    let c3_probe = FakeProbe::from_fixture("esp32c3").unwrap();
    move |port: &PortDesc| -> Box<dyn Probe> {
        if c3.contains(&port.name.as_str()) {
            Box::new(c3_probe.clone())
        } else {
            Box::new(c6.clone())
        }
    }
}

fn record(
    options: &Capture,
    ports: Vec<PortDesc>,
    c3: &'static [&'static str],
    mut enumerate: impl FnMut() -> Vec<PortDesc>,
) -> Result<Session, String> {
    let time = Ticking::default();
    let mut probe = probes(c3);
    capture(
        options,
        ports,
        Bench {
            probe: &mut probe,
            clock: &time,
            sleeper: &time,
            enumerate: &mut enumerate,
        },
    )
}

fn options(port: Option<&str>, run: bool) -> Capture {
    Capture {
        date: "2026-11-27T00:00:00Z".to_owned(),
        note: "synthetic: the C6 fixture through FakeProbe (CH343 + native)".to_owned(),
        port: port.map(str::to_owned),
        run,
    }
}

fn c6_ports() -> Vec<PortDesc> {
    vec![ch343("COM13"), native("COM11", "E8:F6:0A:F7:FF:D4")]
}

/// The C6 with both ports, recorded through the fakes.
fn c6_session() -> Session {
    record(&options(None, true), c6_ports(), &[], c6_enumerate()).unwrap()
}

fn fixture_path() -> PathBuf {
    [
        env!("CARGO_MANIFEST_DIR"),
        "tests",
        "fixtures",
        "manual_session_c6.json",
    ]
    .iter()
    .collect()
}

#[test]
fn the_c6_fixture_is_what_the_recorder_writes() {
    let text = c6_session().to_json().unwrap();
    let path = fixture_path();
    if std::env::var_os("DKT_BLESS_MANUAL").is_some() {
        std::fs::write(&path, &text).unwrap();
    }
    let stored = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    // The host version changes at every release; everything else must not move.
    let unversioned = |text: &str| {
        let mut value: serde_json::Value = serde_json::from_str(text).unwrap();
        value["captured"]["host_version"] = "*".into();
        value["report"]["tool"]["version"] = "*".into();
        value
    };
    assert_eq!(
        unversioned(&stored),
        unversioned(&text),
        "the fixture is stale; see the module docs to regenerate it"
    );
}

#[test]
fn a_session_survives_its_file() {
    let session = c6_session();
    let again = Session::from_json(&session.to_json().unwrap()).unwrap();
    assert_eq!(again, session);
    assert_eq!(again.ports().unwrap(), c6_ports());
    let identified = again.identified().unwrap();
    assert!(matches!(
        identified.as_slice(),
        [Identified::Found { port, chip: Some(_), .. }] if port.name == "COM13"
    ));
    let report = again.report().unwrap().unwrap();
    assert_eq!(report.exit_code(), 0);
    assert_eq!(report.summary().total, 11);
}

#[test]
fn a_session_file_is_checked() {
    let mut value: serde_json::Value =
        serde_json::from_str(&c6_session().to_json().unwrap()).unwrap();
    value["format_version"] = 2.into();
    assert!(matches!(
        Session::from_json(&value.to_string()),
        Err(SessionError::UnsupportedFormat(2))
    ));
    value["format_version"] = 1.into();
    value["identified"][0]["mac"] = "not a mac".into();
    let session = Session::from_json(&value.to_string()).unwrap();
    assert!(matches!(
        session.identified(),
        Err(SessionError::Invalid(_))
    ));
    value["report"]["schema_version"] = 9.into();
    let session = Session::from_json(&value.to_string()).unwrap();
    assert!(matches!(session.report(), Err(SessionError::Report(_))));
}

fn names(images: &[Image]) -> Vec<&str> {
    images
        .iter()
        .map(|image| image.file_name.as_str())
        .collect()
}

fn find<'i>(images: &'i [Image], name: &str) -> &'i str {
    &images
        .iter()
        .find(|image| image.file_name == name)
        .unwrap_or_else(|| panic!("no {name}"))
        .svg
}

/// The visible text of an SVG, `&#160;` back to spaces (runs of one row are concatenated).
fn visible(svg: &str) -> String {
    svg.split("<text ")
        .skip(1)
        .filter_map(|part| part.split_once('>'))
        .filter_map(|(_, rest)| rest.split_once("</text>"))
        .map(|(text, _)| text.replace("&#160;", " "))
        .collect::<Vec<_>>()
        .join("|")
}

#[test]
fn the_c6_session_gives_every_screen() {
    let images = images("c6_both", &c6_session()).unwrap();
    assert_eq!(
        names(&images),
        [
            "c6_both_confirm.svg",
            "c6_both_list.svg",
            "c6_both_running.svg",
            "c6_both_report.svg",
            "c6_both_board.svg",
            "c6_both_text.svg",
        ]
    );
    let confirm = visible(find(&images, "c6_both_confirm.svg"));
    assert!(confirm.contains("Identify bridges"), "{confirm}");
    let list = visible(find(&images, "c6_both_list.svg"));
    assert!(list.contains("COM13") && list.contains("COM11"), "{list}");
    assert!(!list.contains("Not identified yet"), "{list}");
    let running = visible(find(&images, "c6_both_running.svg"));
    assert!(running.contains("COM11"), "{running}");
    let report = visible(find(&images, "c6_both_report.svg"));
    assert!(
        report.contains("PASS") && report.contains("[Report]"),
        "{report}"
    );
    let board = visible(find(&images, "c6_both_board.svg"));
    assert!(board.contains("[Board]"), "{board}");
    let text = visible(find(&images, "c6_both_text.svg"));
    assert!(text.contains("devkit-tester --no-tui"), "{text}");
    assert!(text.contains("identifying 1 bridge port"), "{text}");
    assert!(text.contains("$LASTEXITCODE|0"), "{text}");
    for image in &images {
        assert!(image.svg.starts_with("<svg "), "{}", image.file_name);
        assert!(!image.svg.contains('\u{1b}'), "{}", image.file_name);
    }
}

#[test]
fn images_are_deterministic() {
    let session = c6_session();
    assert_eq!(
        images("c6_both", &session).unwrap(),
        images("c6_both", &session).unwrap()
    );
}

#[test]
fn port_sessions_draw_only_the_transcripts() {
    let session = record(
        &options(Some("e8:f6:0a:f7:ff:d4"), true),
        c6_ports(),
        &[],
        c6_enumerate(),
    )
    .unwrap();
    assert!(
        session.identified.is_empty(),
        "the native port needs no identification"
    );
    let images = images("json", &session).unwrap();
    assert_eq!(names(&images), ["json_text.svg", "json_json.svg"]);
    let json = visible(find(&images, "json_json.svg"));
    assert!(
        json.contains("devkit-tester --json --port e8:f6:0a:f7:ff:d4"),
        "{json}"
    );
    assert!(json.contains("{&quot;schema_version&quot;:1,"), "{json}");

    let error = record(
        &options(Some("COM99"), true),
        c6_ports(),
        &[],
        c6_enumerate(),
    );
    assert!(error.is_err_and(|message| message.contains("no serial port matches")));
}

#[test]
fn two_boards_show_the_list_and_exit_2() {
    let ports = vec![
        ch343("COM13"),
        native("COM11", "E8:F6:0A:F7:FF:D4"),
        usb("COM10", 0x1A86, 0x7522, "6"),
        native("COM6", "80:53:E0:0A:5D:70"),
    ];
    let session = record(&options(None, false), ports, &["COM10"], Vec::new).unwrap();
    assert!(session.report.is_none());
    let images = images("two_boards", &session).unwrap();
    assert_eq!(
        names(&images),
        [
            "two_boards_confirm.svg",
            "two_boards_list.svg",
            "two_boards_text.svg"
        ]
    );
    let text = visible(find(&images, "two_boards_text.svg"));
    assert!(
        text.contains("2 boards found; choose one with --port:"),
        "{text}"
    );
    assert!(text.contains("$LASTEXITCODE|2"), "{text}");
    let list = visible(find(&images, "two_boards_list.svg"));
    assert!(
        list.contains("ESP32-C3") && list.contains("ESP32-C6"),
        "{list}"
    );
}

#[test]
fn a_no_run_session_with_one_board_has_no_transcript() {
    let session = record(&options(None, false), c6_ports(), &[], c6_enumerate()).unwrap();
    let images = images("one", &session).unwrap();
    assert_eq!(names(&images), ["one_confirm.svg", "one_list.svg"]);
}

#[test]
fn long_transcripts_are_cut() {
    assert_eq!(hard_wrap(""), [""]);
    assert_eq!(hard_wrap(&"x".repeat(81)).len(), 2);
}

#[test]
fn utc_dates() {
    assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
    assert_eq!(iso8601_utc(951_782_400), "2000-02-29T00:00:00Z");
    assert_eq!(iso8601_utc(1_700_000_000), "2023-11-14T22:13:20Z");
    assert_eq!(iso8601_utc(4_107_542_399), "2100-02-28T23:59:59Z");
}
