//! Architecture guard: only `src/ui/` may mention `ratatui` or `crossterm`.

use std::fs;
use std::path::{Path, PathBuf};

const UI_ONLY_CRATES: [&str; 2] = ["ratatui", "crossterm"];

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

#[test]
fn only_ui_imports_ratatui_or_crossterm() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let ui = src.join("ui");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(
        !files.is_empty(),
        "no Rust files found under {}",
        src.display()
    );

    let offenders: Vec<String> = files
        .iter()
        .filter(|path| !path.starts_with(&ui))
        .filter_map(|path| {
            let text = fs::read_to_string(path).unwrap();
            let found: Vec<&str> = UI_ONLY_CRATES
                .into_iter()
                .filter(|name| {
                    text.lines()
                        .filter(|line| !line.trim_start().starts_with("//"))
                        .any(|line| line.contains(name))
                })
                .collect();
            (!found.is_empty()).then(|| {
                let rel = path.strip_prefix(&src).unwrap_or(path);
                format!("{} ({})", rel.display(), found.join(", "))
            })
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "ratatui/crossterm used outside src/ui/: {}",
        offenders.join("; ")
    );
}
