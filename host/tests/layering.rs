//! Architecture guards:
//! - only `src/ui/` may mention `ratatui` or `crossterm`;
//! - only `src/platform.rs` / `src/platform/` may use OS-specific `cfg` (ADR-007).

use std::fs;
use std::path::{Path, PathBuf};

const UI_ONLY_CRATES: [&str; 2] = ["ratatui", "crossterm"];
const OS_CFG_TERMS: [&str; 4] = ["windows", "unix", "target_os", "target_family"];

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
