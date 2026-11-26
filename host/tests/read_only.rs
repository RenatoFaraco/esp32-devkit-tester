//! Read-only guard for v0.1 (ADR-003, ADR-009, issue #45).
//!
//! The ROM bootloader would happily erase the flash, burn eFuses or run code from RAM if asked.
//! v0.1 must never ask, so this test turns the promise into a failing build:
//!
//! - no file under `src/` may mention an espflash API that writes flash, burns eFuses, loads code
//!   into RAM, changes the baud rate or disables the watchdog;
//! - `write_reg` (volatile SPI1 register writes for RDID, allowed by ADR-009 §2) may appear only in
//!   `src/probe/rdid.rs`;
//! - the connection is built with `use_stub = false` and no baud change after the sync.
//!
//! Names come from espflash 4.5.0 (`flasher/mod.rs`, `target/mod.rs`, `command.rs`). When espflash
//! is upgraded, review this list against its new API in the same PR.

use std::fs;
use std::path::{Path, PathBuf};

/// espflash calls the real `Probe` is allowed to make (ADR-009 §2). Documentation for reviewers:
/// anything new in `src/probe/espflash.rs` or `src/probe/rdid.rs` must be added here, with the
/// reason, in the same PR.
#[allow(dead_code)]
const ALLOWED_ESPFLASH_CALLS: &[(&str, &str)] = &[
    ("Connection::new", "wraps the opened port; no I/O"),
    (
        "Flasher::try_connect",
        "reset + sync + detect + SPI_ATTACH/SPI_SET_PARAMS (volatile)",
    ),
    ("Connection::detect_chip", "GET_SECURITY_INFO / READ_REG"),
    ("Connection::security_info", "GET_SECURITY_INFO"),
    ("Connection::read_reg", "READ_REG"),
    (
        "Connection::write_reg",
        "only in rdid.rs: SPI1 RDID, restored",
    ),
    (
        "Connection::reset / reset_after",
        "hard reset back to the app",
    ),
    (
        "Chip::revision / xtal_frequency / mac_address / read_efuse_le / spi_registers",
        "eFuse and register reads",
    ),
];

/// espflash APIs that change the board's non-volatile state, load code or leave the board in a
/// different state. None of them may appear in `src/`.
const FORBIDDEN: &[&str] = &[
    // Flash writes and erases.
    "write_bin_to_flash",
    "write_bins_to_flash",
    "load_image_to_flash",
    "erase_region",
    "erase_flash",
    "erase_parts",
    "FlashBegin",
    "FlashData",
    "FlashEnd",
    "FlashDeflBegin",
    "FlashDeflData",
    "EraseFlash",
    "EraseRegion",
    // eFuse burns (irreversible).
    "write_efuse",
    // Code in RAM (the stub or anything else).
    "load_elf_to_ram",
    "load_stub",
    "MemBegin",
    "MemData",
    "MemEnd",
    "RunUserCode",
    // State changes outside the ADR-009 list.
    "change_baud",
    "ChangeBaudrate",
    "set_baud",
    "disable_watchdog",
    "rtc_wdt_reset",
];

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

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Non-comment lines of every `.rs` file under `src/`, as `(relative path, line number, text)`.
fn code_lines() -> Vec<(PathBuf, usize, String)> {
    let src = src_dir();
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(!files.is_empty(), "no Rust files under {}", src.display());
    let mut lines = Vec::new();
    for path in files {
        let text = fs::read_to_string(&path).unwrap();
        let rel = path.strip_prefix(&src).unwrap_or(&path).to_path_buf();
        for (index, line) in text.lines().enumerate() {
            if !line.trim_start().starts_with("//") {
                lines.push((rel.clone(), index + 1, line.to_owned()));
            }
        }
    }
    lines
}

/// Returns the forbidden names found in `line`, matched as whole identifiers.
fn forbidden_in(line: &str) -> Vec<&'static str> {
    FORBIDDEN
        .iter()
        .copied()
        .filter(|name| contains_identifier(line, name))
        .collect()
}

fn contains_identifier(line: &str, name: &str) -> bool {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    line.match_indices(name).any(|(start, _)| {
        let before = line[..start].chars().next_back();
        let after = line[start + name.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

#[test]
fn no_flash_efuse_ram_or_baud_writes_in_src() {
    let offenders: Vec<String> = code_lines()
        .into_iter()
        .filter_map(|(path, line_no, line)| {
            let found = forbidden_in(&line);
            (!found.is_empty())
                .then(|| format!("{}:{} ({})", path.display(), line_no, found.join(", ")))
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "v0.1 is read-only (ADR-003/009) but src/ mentions: {}",
        offenders.join("; ")
    );
}

#[test]
fn write_reg_only_in_rdid() {
    let rdid = Path::new("probe").join("rdid.rs");
    let offenders: Vec<String> = code_lines()
        .into_iter()
        .filter(|(path, _, line)| *path != rdid && contains_identifier(line, "write_reg"))
        .map(|(path, line_no, _)| format!("{}:{}", path.display(), line_no))
        .collect();
    assert!(
        offenders.is_empty(),
        "write_reg outside src/probe/rdid.rs (ADR-009 §2 allows it only for the SPI1 RDID): {}",
        offenders.join("; ")
    );
}

#[test]
fn connection_uses_rom_only_and_keeps_115200() {
    let path = src_dir().join("probe").join("espflash.rs");
    let text = fs::read_to_string(&path).unwrap();
    let code: String = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        code.contains("const USE_STUB: bool = false;"),
        "the RAM stub must stay off (ADR-009 §1)"
    );
    assert!(
        code.contains("const BAUD_AFTER_SYNC: Option<u32> = None;"),
        "the baud rate must not change after the sync (ADR-009 §2)"
    );
    assert!(
        code.contains("const BAUD: u32 = 115_200;"),
        "the ROM is spoken to at 115200 baud"
    );
    let connect = code
        .lines()
        .find(|line| line.contains("Flasher::try_connect("))
        .expect("Flasher::try_connect call not found in probe/espflash.rs");
    assert!(
        connect.contains("USE_STUB") && connect.contains("BAUD_AFTER_SYNC"),
        "Flasher::try_connect must take USE_STUB and BAUD_AFTER_SYNC, not literals: {connect}"
    );
    assert!(
        !contains_identifier(connect, "true"),
        "Flasher::try_connect must not be called with a literal true: {connect}"
    );
}

#[test]
fn guard_matches_whole_identifiers_only() {
    assert_eq!(forbidden_in("flasher.erase_flash()"), vec!["erase_flash"]);
    assert_eq!(forbidden_in("Command::FlashBegin {"), vec!["FlashBegin"]);
    assert!(forbidden_in("let not_erase_flash_here = 1;").is_empty());
    assert!(forbidden_in("fn my_load_stub_helper()").is_empty());
    assert!(contains_identifier(
        "conn.write_reg(a, b, None)",
        "write_reg"
    ));
    assert!(!contains_identifier("fn write_registers()", "write_reg"));
}
