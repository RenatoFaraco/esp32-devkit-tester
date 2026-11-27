//! The user manual's images (issue #64):
//! - every `manual/img/*.svg` that `docs/project/MANUAL*.md` cites exists (no manual yet: nothing
//!   to check);
//! - every SVG in `docs/project/manual/img/` was drawn from a session that exists
//!   (`<session>_<screen>.svg` ↔ `sessions/<session>.json`);
//! - drawing is deterministic: the synthetic session `tests/fixtures/manual_session_c6.json`
//!   (recorded from the C6 fixture with fake probes, no bench file) gives the same bytes twice.

use std::fs;
use std::path::{Path, PathBuf};

use esp32_devkit_tester::ui::manual::{self, SCREENS, Session};

fn docs_project() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("docs")
        .join("project")
}

/// The `manual/img/<file>.svg` references in `text`, as file names.
fn cited_images(text: &str) -> Vec<String> {
    const MARK: &str = "manual/img/";
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(MARK) {
        let tail = &rest[start + MARK.len()..];
        let end = tail
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'))
            .unwrap_or(tail.len());
        let name = &tail[..end];
        if name.ends_with(".svg") {
            found.push(name.to_owned());
        }
        rest = tail;
    }
    found
}

/// The manuals, `MANUAL.md` and its translations.
fn manuals() -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(docs_project()) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("MANUAL") && name.ends_with(".md"))
        })
        .collect();
    paths.sort();
    paths
}

#[test]
fn every_cited_image_exists() {
    let img = docs_project().join("manual").join("img");
    let mut missing = Vec::new();
    for manual in manuals() {
        let text = fs::read_to_string(&manual).unwrap();
        for name in cited_images(&text) {
            if !img.join(&name).is_file() {
                missing.push(format!("{} cites {name}", manual.display()));
            }
        }
    }
    assert!(missing.is_empty(), "missing images: {}", missing.join("; "));
}

/// The session a file name `<session>_<screen>.svg` comes from.
fn session_of(file_name: &str) -> Option<&str> {
    let stem = file_name.strip_suffix(".svg")?;
    SCREENS
        .iter()
        .find_map(|screen| stem.strip_suffix(&format!("_{screen}")))
}

#[test]
fn every_image_comes_from_a_session() {
    let manual = docs_project().join("manual");
    let Ok(entries) = fs::read_dir(manual.join("img")) else {
        return;
    };
    let mut orphans = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".svg") {
            continue;
        }
        let from_session = session_of(&name).is_some_and(|session| {
            manual
                .join("sessions")
                .join(format!("{session}.json"))
                .is_file()
        });
        if !from_session {
            orphans.push(name);
        }
    }
    assert!(
        orphans.is_empty(),
        "images without a session (run examples/manual_screenshots.rs or delete them): {}",
        orphans.join(", ")
    );
}

#[test]
fn the_guards_read_names_right() {
    assert_eq!(
        cited_images("![x](manual/img/c6_both_list.svg) and manual/img/a-b.svg)."),
        ["c6_both_list.svg", "a-b.svg"]
    );
    assert!(cited_images("manual/img/photo.png").is_empty());
    assert_eq!(session_of("c6_both_list.svg"), Some("c6_both"));
    assert_eq!(session_of("two_boards_text.svg"), Some("two_boards"));
    assert_eq!(session_of("notes.svg"), None);
}

#[test]
fn drawing_a_session_is_deterministic() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("manual_session_c6.json");
    let text = fs::read_to_string(&path).unwrap();
    let first = manual::images("c6", &Session::from_json(&text).unwrap()).unwrap();
    let second = manual::images("c6", &Session::from_json(&text).unwrap()).unwrap();
    assert_eq!(first, second);
    let names: Vec<&str> = first.iter().map(|image| image.file_name.as_str()).collect();
    assert_eq!(
        names,
        [
            "c6_confirm.svg",
            "c6_list.svg",
            "c6_running.svg",
            "c6_report.svg",
            "c6_board.svg",
            "c6_text.svg"
        ]
    );
    for image in &first {
        assert!(image.svg.starts_with("<svg "), "{}", image.file_name);
        assert!(image.svg.ends_with("</svg>\n"), "{}", image.file_name);
        assert!(
            !image.svg.chars().any(|c| c.is_control() && c != '\n'),
            "{}",
            image.file_name
        );
    }
}
