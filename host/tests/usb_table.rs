//! The VID:PID table in `transport` must match the table "Identificação das portas" in
//! `docs/context/hardware.md`, so document and code cannot drift apart silently.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use esp32_devkit_tester::transport::KNOWN_USB;

const HEADING: &str = "## Identificação das portas";

/// Every `` `XXXX:XXXX` `` (four hex digits each side) in a line, upper-cased.
fn vid_pids_in(line: &str) -> Vec<(u16, u16)> {
    line.split('`')
        .skip(1)
        .step_by(2)
        .filter_map(|token| {
            let (vid, pid) = token.split_once(':')?;
            let hex4 = |s: &str| s.len() == 4 && s.chars().all(|c| c.is_ascii_hexdigit());
            if !hex4(vid) || !hex4(pid) {
                return None;
            }
            Some((
                u16::from_str_radix(vid, 16).ok()?,
                u16::from_str_radix(pid, 16).ok()?,
            ))
        })
        .collect()
}

/// VID:PIDs in the first column of the table under [`HEADING`].
fn hardware_md_vid_pids(text: &str) -> BTreeSet<(u16, u16)> {
    let mut lines = text
        .lines()
        .skip_while(|line| !line.starts_with(HEADING))
        .skip(1)
        .skip_while(|line| !line.trim_start().starts_with('|'));
    let mut found = BTreeSet::new();
    for line in lines.by_ref() {
        let line = line.trim();
        if !line.starts_with('|') {
            break;
        }
        let first_cell = line.split('|').nth(1).unwrap_or("");
        found.extend(vid_pids_in(first_cell));
    }
    found
}

#[test]
fn parser_reads_single_and_paired_rows() {
    assert_eq!(vid_pids_in("| `303A:1001` |"), vec![(0x303A, 0x1001)]);
    assert_eq!(
        vid_pids_in(" `1A86:55D3` / `1A86:55D4` "),
        vec![(0x1A86, 0x55D3), (0x1A86, 0x55D4)]
    );
    assert!(vid_pids_in("`AA:BB:CC:DD:EE:FF` `MI_00`").is_empty());
}

#[test]
fn classifies_every_vid_pid_in_hardware_md() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("docs")
        .join("context")
        .join("hardware.md");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));

    let documented = hardware_md_vid_pids(&text);
    assert!(
        documented.len() >= 7,
        "table under {HEADING:?} not found or too short in {}: {documented:x?}",
        path.display()
    );

    let in_code: BTreeSet<(u16, u16)> = KNOWN_USB.iter().map(|e| (e.vid, e.pid)).collect();
    assert_eq!(
        in_code.len(),
        KNOWN_USB.len(),
        "duplicate VID:PID in KNOWN_USB"
    );

    let fmt = |set: &BTreeSet<(u16, u16)>| {
        set.iter()
            .map(|(vid, pid)| format!("{vid:04X}:{pid:04X}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let missing_in_code: BTreeSet<_> = documented.difference(&in_code).copied().collect();
    let missing_in_doc: BTreeSet<_> = in_code.difference(&documented).copied().collect();
    assert!(
        missing_in_code.is_empty() && missing_in_doc.is_empty(),
        "hardware.md and transport::KNOWN_USB differ; only in hardware.md: [{}]; only in code: [{}]",
        fmt(&missing_in_code),
        fmt(&missing_in_doc)
    );
}
