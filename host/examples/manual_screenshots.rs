//! Redraws the user manual's images from the recorded sessions (issue #64): every
//! `docs/project/manual/sessions/<name>.json` becomes `docs/project/manual/img/<name>_<screen>.svg`
//! (screens and rules in `esp32_devkit_tester::ui::manual`).
//!
//! ```powershell
//! cargo run --manifest-path host/Cargo.toml --example manual_screenshots
//! cargo run --manifest-path host/Cargo.toml --example manual_screenshots -- --out "$env:TEMP/img" host/tests/fixtures/manual_session_c6.json
//! ```
//!
//! No board, no port: only the session files. Deterministic: a file whose content would not
//! change is not rewritten, so running it twice changes nothing. Exit code 0, or 2 when a session
//! cannot be read or an image cannot be written.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use esp32_devkit_tester::ui::manual::{self, Session};

const USAGE: &str = "usage: manual_screenshots [--out DIR] [SESSION.json ...]";

fn manual_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("docs")
        .join("project")
        .join("manual")
}

/// Every `*.json` in `dir`, sorted by name (the order of the output).
fn sessions_in(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    Ok(paths)
}

fn main() -> ExitCode {
    let mut out = manual_dir().join("img");
    let mut paths = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => match args.next() {
                Some(dir) => out = PathBuf::from(dir),
                None => {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            },
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    if paths.is_empty() {
        let dir = manual_dir().join("sessions");
        paths = match sessions_in(&dir) {
            Ok(paths) => paths,
            Err(error) => {
                eprintln!("manual_screenshots: {}: {error}", dir.display());
                return ExitCode::from(2);
            }
        };
    }
    if let Err(error) = std::fs::create_dir_all(&out) {
        eprintln!("manual_screenshots: {}: {error}", out.display());
        return ExitCode::from(2);
    }

    let (mut written, mut unchanged) = (0, 0);
    for path in &paths {
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let images = std::fs::read_to_string(path)
            .map_err(|error| error.to_string())
            .and_then(|text| Session::from_json(&text).map_err(|error| error.to_string()))
            .and_then(|session| manual::images(&name, &session).map_err(|e| e.to_string()));
        let images = match images {
            Ok(images) => images,
            Err(error) => {
                eprintln!("manual_screenshots: {}: {error}", path.display());
                return ExitCode::from(2);
            }
        };
        for image in images {
            let target = out.join(&image.file_name);
            if std::fs::read_to_string(&target).is_ok_and(|old| old == image.svg) {
                unchanged += 1;
                continue;
            }
            if let Err(error) = std::fs::write(&target, &image.svg) {
                eprintln!("manual_screenshots: {}: {error}", target.display());
                return ExitCode::from(2);
            }
            eprintln!("wrote {}", target.display());
            written += 1;
        }
    }
    eprintln!(
        "{} session(s): {written} image(s) written, {unchanged} unchanged",
        paths.len()
    );
    ExitCode::SUCCESS
}
