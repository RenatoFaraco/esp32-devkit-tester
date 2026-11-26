//! Architecture guards:
//! - only `src/ui/` may mention `ratatui` or `crossterm`;
//! - only `src/platform.rs` / `src/platform/` may use OS-specific `cfg` (ADR-007);
//! - only `src/chips.rs` may name a specific chip variant (ADR-004: a new chip is a new row);
//! - only `src/probe/espflash.rs` and `src/probe/rdid.rs` may use the `espflash` crate
//!   (ADR-011: its types and errors never cross the `Probe` seam).

use std::fs;
use std::path::{Path, PathBuf};

const UI_ONLY_CRATES: [&str; 2] = ["ratatui", "crossterm"];
const OS_CFG_TERMS: [&str; 4] = ["windows", "unix", "target_os", "target_family"];
const CHIP_VARIANT_TERMS: [&str; 2] = ["Chip::Esp", "Chip::*"];
/// Files allowed to name the `espflash` crate, relative to `src/`.
const ESPFLASH_FILES: [[&str; 2]; 2] = [["probe", "espflash.rs"], ["probe", "rdid.rs"]];
/// A `espflash::` preceded by one of these is our module `probe::espflash`, not the crate.
const OWN_MODULE_PREFIXES: [&str; 3] = ["self::", "super::", "probe::"];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Scans every `.rs` file under `src/` for which `scan` returns true, and reports
/// `relative/path:line (detail)` for each non-comment line where `matches` returns `Some`.
fn offending_lines(
    scan: impl Fn(&Path, &Path) -> bool,
    matches: impl Fn(&str) -> Option<String>,
) -> Vec<String> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(
        !files.is_empty(),
        "no Rust files found under {}",
        src.display()
    );

    let mut offenders = Vec::new();
    for path in files.iter().filter(|path| scan(&src, path)) {
        let text = fs::read_to_string(path).unwrap();
        let rel = path.strip_prefix(&src).unwrap_or(path);
        for (index, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            if let Some(detail) = matches(line) {
                offenders.push(format!("{}:{} ({detail})", rel.display(), index + 1));
            }
        }
    }
    offenders
}

#[test]
fn only_ui_imports_ratatui_or_crossterm() {
    let offenders = offending_lines(
        |src, path| !path.starts_with(src.join("ui")),
        |line| {
            let found: Vec<&str> = UI_ONLY_CRATES
                .into_iter()
                .filter(|name| line.contains(name))
                .collect();
            (!found.is_empty()).then(|| found.join(", "))
        },
    );

    assert!(
        offenders.is_empty(),
        "ratatui/crossterm used outside src/ui/: {}",
        offenders.join("; ")
    );
}

#[test]
fn os_cfg_only_in_platform() {
    let offenders = offending_lines(
        |src, path| path != src.join("platform.rs") && !path.starts_with(src.join("platform")),
        |line| {
            if !line.contains("cfg") {
                return None;
            }
            let found: Vec<&str> = OS_CFG_TERMS
                .into_iter()
                .filter(|term| line.contains(term))
                .collect();
            (!found.is_empty()).then(|| found.join(", "))
        },
    );

    assert!(
        offenders.is_empty(),
        "OS-specific cfg outside src/platform: {}",
        offenders.join("; ")
    );
}

#[test]
fn chip_variants_only_in_chips() {
    let offenders = offending_lines(
        |src, path| path != src.join("chips.rs"),
        |line| {
            let found: Vec<&str> = CHIP_VARIANT_TERMS
                .into_iter()
                .filter(|term| line.contains(term))
                .collect();
            (!found.is_empty()).then(|| found.join(", "))
        },
    );

    assert!(
        offenders.is_empty(),
        "chip variants named outside src/chips.rs (use the capability table): {}",
        offenders.join("; ")
    );
}

/// Uses of the `espflash` crate in a line: `use espflash`, `extern crate espflash` and every
/// `espflash::` path that is not our own `probe::espflash` module.
fn espflash_crate_uses(line: &str) -> Option<String> {
    let mut found = Vec::new();
    for term in ["use espflash", "extern crate espflash"] {
        if line.contains(term) {
            found.push(term.to_owned());
        }
    }
    for (index, _) in line.match_indices("espflash::") {
        let before = &line[..index];
        if !OWN_MODULE_PREFIXES
            .iter()
            .any(|prefix| before.ends_with(prefix))
        {
            found.push("espflash::".to_owned());
        }
    }
    found.dedup();
    (!found.is_empty()).then(|| found.join(", "))
}

#[test]
fn only_the_probe_backend_uses_espflash() {
    let offenders = offending_lines(
        |src, path| {
            !ESPFLASH_FILES
                .iter()
                .any(|parts| path == parts.iter().fold(src.to_path_buf(), |p, part| p.join(part)))
        },
        espflash_crate_uses,
    );

    assert!(
        offenders.is_empty(),
        "espflash used outside src/probe/espflash.rs and src/probe/rdid.rs: {}",
        offenders.join("; ")
    );
}

#[test]
fn espflash_guard_tells_the_crate_from_our_module() {
    assert!(espflash_crate_uses("use espflash::connection::Connection;").is_some());
    assert!(espflash_crate_uses("let e: espflash::Error = x;").is_some());
    assert!(espflash_crate_uses("use ::espflash::flasher::Flasher;").is_some());
    assert!(espflash_crate_uses("use espflash as rom;").is_some());
    assert!(espflash_crate_uses("pub use self::espflash::EspflashProbe;").is_none());
    assert!(espflash_crate_uses("use super::espflash::{map_err, shield};").is_none());
    assert!(espflash_crate_uses("use crate::probe::espflash::Stage;").is_none());
    assert!(espflash_crate_uses("mod espflash;").is_none());
    assert!(espflash_crate_uses("let probe = EspflashProbe::new();").is_none());
}
