//! Layout of the Board panel, the `Board` tab of the `Done` screen: what board this is (PRD H7).
//!
//! Pure, like [`super::report_view`]: [`board_lines`] builds ratatui [`Line`]s from the model
//! and a width, with no I/O and no terminal.
//!
//! # Where each value comes from (nothing is read twice)
//! - **read**: the [`TestResult`](crate::report::TestResult)s of the [`Report`], by [`TestId`]
//!   (revision, crystal, PSRAM, embedded flash, JEDEC ID, flash size, ROM identity, eFuse MAC),
//!   and the report's `BoardInfo` (chip, revision, MAC, ports);
//! - **datasheet**: the chip row of the capability table, through `ChipCaps::fields()` (CPU,
//!   maximum clock, radios, USB-Serial/JTAG, USB OTG).
//!
//! The read value wins (ADR-004). When it disagrees with the table (the test carries a
//! `Finding`) or with what its test expected, it shows a yellow `!` and a detail line
//! `└ table 40 MHz (variable)` (`└ ROM <mac>` for the eFuse MAC). A value that was not read is
//! `—` and a short gray reason (`not read (flash/jedec-id failed)`): the panel never makes a
//! value up, nor falls back to the table for a value that should have been read.
//!
//! # Columns (approved mockup, issue #23)
//! ```text
//!   Memory
//!     Flash            16 MB external · JEDEC 0x684018               read
//!     Crystal          26 MHz !                                      read
//!                      └ table 40 MHz (variable)
//! ```
//! Labels and the source column in gray, values in the default colour. The source is flush
//! right; a value too long for its row wraps under itself (between words), never cut, so no
//! line is wider than the width it was laid out for, keeping one cell of margin on the right.
//! The Ports section lists the board's ports in the tree of the board list; their names come
//! from the [`Board`] that was run (the report leaves them out, spec D1) and are only shown on
//! screen.
//!
//! Every text shown here is a [`SafeText`] from the model, a port name through [`safe`] or a
//! host literal: nothing coming from the board reaches the terminal unsanitized.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::report_view::{fit, wrap_words};
use super::{KIND_COLUMN, MODEL_COLUMN, PORT_COLUMN, bold, gray, green, red, safe, spread, yellow};
use crate::chips::FieldView;
use crate::report::{Outcome, PortKind, Report, SafeText, State, TestId};
use crate::runner::{
    Board, CHIP_IDENTITY, CRYSTAL, EMBEDDED_FLASH, FLASH_SIZE, JEDEC_ID, MAC_EFUSE_REG,
    NO_EMBEDDED_FLASH, PACKAGE, PSRAM, REVISION, SKIP_NATIVE_NOT_CONNECTED, USB_SERIAL_JTAG,
};

/// Width of the label column (`USB-Serial/JTAG` and two spaces).
const LABEL_COLUMN: usize = 17;
/// Where the value starts: four cells of indent and the label column.
const VALUE_COLUMN: usize = 4 + LABEL_COLUMN;
/// Where detail text starts: after `└ ` under the value column.
const DETAIL_COLUMN: usize = VALUE_COLUMN + 2;
/// Cells kept free on the right edge (the same margin as [`spread`]).
const MARGIN: usize = 1;
/// Fewest spaces between a value and its source, so the two never run together.
const SOURCE_GAP: usize = 2;
/// Cells before a port name: indent, branch and one space (`    ├─ `).
const PORT_INDENT: usize = 7;
/// What a port row needs after its name: the model and kind columns and `primary`.
const PORT_TAIL: usize = 1 + MODEL_COLUMN + 1 + KIND_COLUMN + 1 + "primary".len();

/// Source of a value read from the board.
const READ: &str = "read";
/// Source of a value taken from the capability table.
const DATASHEET: &str = "datasheet";
/// Source of the MAC decoded from the eFuse words (`rom/mac-efuse-reg`).
const READ_EFUSE: &str = "read (eFuse)";
/// Source of the MAC the ROM reported, when the eFuse words were not read.
const READ_ROM: &str = "read (ROM)";

/// Styled words of a value, laid out by `wrap_words`.
type Words = Vec<(String, Style)>;

/// `text` split into words, all in `style`.
fn words(text: &str, style: Style) -> Words {
    text.split_whitespace()
        .map(|word| (word.to_owned(), style))
        .collect()
}

/// The gray `·` between the parts of a value.
fn dot() -> (String, Style) {
    ("·".to_owned(), gray())
}

/// One labelled row of the panel.
struct Row {
    label: &'static str,
    /// The value, word by word.
    value: Words,
    /// A yellow `!` after the value: the read value disagrees with the table or the test.
    flag: bool,
    /// The `└` line under the value (empty: none).
    detail: Words,
    /// `read`, `datasheet`... flush right; `None` when nothing was read.
    source: Option<&'static str>,
}

impl Row {
    fn new(label: &'static str, value: Words, source: &'static str) -> Row {
        Row {
            label,
            value,
            flag: false,
            detail: Vec::new(),
            source: Some(source),
        }
    }

    /// `—` and the reason in gray, with no source: nothing was read.
    fn missing(label: &'static str, reason: &str) -> Row {
        let mut value = vec![("—".to_owned(), Style::new())];
        value.extend(words(reason, gray()));
        Row {
            label,
            value,
            flag: false,
            detail: Vec::new(),
            source: None,
        }
    }

    /// A value read by a test, marked when it disagrees: with the table (the finding, `table
    /// <x> (<kind>)`) or, without a finding, with the value the test expected
    /// (`<expected_label> <x>`).
    fn read(
        label: &'static str,
        outcome: &Outcome,
        value: Words,
        source: &'static str,
        expected_label: &str,
    ) -> Row {
        let mut row = Row::new(label, value, source);
        if let Some(finding) = outcome.finding() {
            row.flag = true;
            row.detail = words("table", gray());
            row.detail
                .extend(words(finding.table().as_str(), Style::new()));
            row.detail.push((format!("({})", finding.kind()), gray()));
        } else if let (Some(expected), Some(read)) = (outcome.expected(), outcome.read())
            && expected != read
        {
            row.flag = true;
            row.detail = words(expected_label, gray());
            row.detail.extend(words(expected.as_str(), Style::new()));
        }
        row
    }
}

/// One section of the panel: a bold title and its rows.
struct Section {
    title: &'static str,
    rows: Vec<Row>,
}

/// The outcome of test `id` in the report, if it is there.
fn outcome_of(report: &Report, id: TestId) -> Option<&Outcome> {
    report
        .tests()
        .iter()
        .find(|test| test.id == id)
        .map(|test| &test.outcome)
}

/// What test `id` read, if the panel may show it: a PASS with a value, or a FAIL that still
/// carries a comparison (a finding or an expected value, shown with `!`). A FAIL without one
/// (a blank flash bus, say) read nothing worth showing. `Err` is the short reason for `—`.
fn read_of(report: &Report, id: TestId) -> Result<(&Outcome, &SafeText), String> {
    let usable = |outcome: &Outcome| match outcome.state() {
        State::Pass => true,
        State::Fail => outcome.finding().is_some() || outcome.expected().is_some(),
        State::Skip => false,
    };
    match outcome_of(report, id) {
        Some(outcome) if usable(outcome) => match outcome.read() {
            Some(read) => Ok((outcome, read)),
            None => Err(not_read(report, id)),
        },
        _ => Err(not_read(report, id)),
    }
}

/// Why test `id` gave nothing: `not read (<id> failed)`, `not read (<id> skipped)`, or the
/// cause a cascaded SKIP points at (`not read (flash/jedec-id failed)`).
fn not_read(report: &Report, id: TestId) -> String {
    let Some(outcome) = outcome_of(report, id) else {
        return format!("not read ({id} not run)");
    };
    match outcome.state() {
        State::Skip => outcome
            .reason()
            .and_then(|reason| reason.as_str().strip_prefix("not run: "))
            .map_or_else(
                || format!("not read ({id} skipped)"),
                |cause| format!("not read ({cause})"),
            ),
        State::Pass | State::Fail => format!("not read ({id} failed)"),
    }
}

/// The value of capability `name` in the chip's row.
fn field<'f>(fields: &'f [FieldView], name: &str) -> Option<&'f str> {
    fields
        .iter()
        .find(|view| view.name == name)
        .map(|view| view.value.as_str())
}

/// The whole Board panel for `report`, the result of running `board`, laid out for `width`.
pub(super) fn board_lines(report: &Report, board: &Board, width: u16) -> Vec<Line<'static>> {
    let fields = report
        .board()
        .chip
        .as_ref()
        .map(|info| info.chip.caps().fields())
        .unwrap_or_default();
    let sections = [
        Section {
            title: "Chip",
            rows: chip_rows(report, &fields),
        },
        Section {
            title: "Memory",
            rows: vec![flash_row(report), psram_row(report)],
        },
        Section {
            title: "Radios",
            rows: field(&fields, "radios")
                .map(|radios| {
                    // The table joins radios with `, `; the panel uses its own separator.
                    let mut value = Words::new();
                    for (index, radio) in radios.split(", ").enumerate() {
                        if index > 0 {
                            value.push(dot());
                        }
                        value.extend(words(radio, Style::new()));
                    }
                    Row::new("Wireless", value, DATASHEET)
                })
                .into_iter()
                .collect(),
        },
        Section {
            title: "USB",
            rows: usb_rows(report, &fields),
        },
        Section {
            title: "Identity",
            rows: vec![mac_row(report), rom_id_row(report)],
        },
    ];

    let mut lines = Vec::new();
    for section in sections {
        if section.rows.is_empty() {
            continue;
        }
        lines.push(Line::styled(format!("  {}", section.title), bold()));
        for row in &section.rows {
            lines.extend(row_lines(row, width));
        }
    }
    lines.push(Line::styled("  Ports", bold()));
    lines.extend(port_lines(report, board, width));
    lines
}

/// Family (name and package), revision, CPU, maximum clock and crystal.
fn chip_rows(report: &Report, fields: &[FieldView]) -> Vec<Row> {
    let Some(info) = report.board().chip.as_ref() else {
        return vec![Row::missing("Family", "chip not identified")];
    };
    let table = |name: &str| field(fields, name);
    let mut family = words(info.chip.caps().name, Style::new());
    if let Ok((_, package)) = read_of(report, PACKAGE) {
        family.push(dot());
        family.extend(words(package.as_str(), Style::new()));
    }
    let mut rows = vec![Row::new("Family", family, READ)];
    rows.push(match info.revision {
        Some(revision) => Row::new("Revision", words(&revision.to_string(), Style::new()), READ),
        None => Row::missing("Revision", &not_read(report, REVISION)),
    });
    if let (Some(arch), Some(cores)) = (table("arch"), table("cores")) {
        let mut cpu = words(arch, Style::new());
        cpu.push(("×".to_owned(), Style::new()));
        cpu.extend(words(cores, Style::new()));
        if let Some(lp_core) = table("lp_core").filter(|lp_core| *lp_core != "none") {
            cpu.push(dot());
            cpu.extend(words(lp_core, Style::new()));
        }
        rows.push(Row::new("CPU", cpu, DATASHEET));
    }
    if let Some(clock) = table("max_clock_mhz") {
        rows.push(Row::new("Max clock", words(clock, Style::new()), DATASHEET));
    }
    rows.push(match read_of(report, CRYSTAL) {
        Ok((outcome, read)) => Row::read(
            "Crystal",
            outcome,
            words(read.as_str(), Style::new()),
            READ,
            "expected",
        ),
        Err(reason) => Row::missing("Crystal", &reason),
    });
    rows
}

/// Flash: size, external or embedded, and the JEDEC ID; `—` when the size was not read.
fn flash_row(report: &Report) -> Row {
    let Ok((_, size)) = read_of(report, FLASH_SIZE) else {
        // The size comes from the JEDEC ID: name the first test that gave nothing.
        let reason = match read_of(report, JEDEC_ID) {
            Err(reason) => reason,
            Ok(_) => not_read(report, FLASH_SIZE),
        };
        return Row::missing("Flash", &reason);
    };
    let mut value = words(size.as_str(), Style::new());
    if let Ok((_, embedded)) = read_of(report, EMBEDDED_FLASH) {
        let place = if embedded.as_str() == NO_EMBEDDED_FLASH {
            "external"
        } else {
            "embedded"
        };
        value.push((place.to_owned(), Style::new()));
    }
    if let Ok((_, jedec)) = read_of(report, JEDEC_ID) {
        value.push(dot());
        value.push(("JEDEC".to_owned(), gray()));
        value.extend(words(jedec.as_str(), Style::new()));
    }
    Row::new("Flash", value, READ)
}

/// PSRAM in the package, as the eFuse declares it.
fn psram_row(report: &Report) -> Row {
    match read_of(report, PSRAM) {
        Ok((outcome, read)) => Row::read(
            "PSRAM",
            outcome,
            words(read.as_str(), Style::new()),
            READ,
            "expected",
        ),
        Err(reason) => Row::missing("PSRAM", &reason),
    }
}

/// USB-Serial/JTAG (the table says whether the chip has it, `port/usb-serial-jtag` what this
/// board did with it) and USB OTG.
fn usb_rows(report: &Report, fields: &[FieldView]) -> Vec<Row> {
    let mut rows = Vec::new();
    if let Some(has) = field(fields, "usb_serial_jtag") {
        let label = "USB-Serial/JTAG";
        let mut value = words(has, Style::new());
        let outcome = outcome_of(report, USB_SERIAL_JTAG);
        let row = match outcome {
            // The native port answered on a chip the table says has none: the read wins.
            Some(outcome) if outcome.finding().is_some() => {
                let read = outcome.read().map_or(has, SafeText::as_str);
                Row::read(label, outcome, words(read, Style::new()), READ, "expected")
            }
            Some(outcome) if outcome.state() == State::Pass => {
                value.push(dot());
                value.push(("tested".to_owned(), Style::new()));
                value.push(("✓".to_owned(), green()));
                Row::new(label, value, READ)
            }
            Some(outcome) if outcome.state() == State::Fail => {
                value.push(dot());
                value.push(("✕".to_owned(), red()));
                value.push(("failed".to_owned(), red()));
                Row::new(label, value, READ)
            }
            Some(outcome) if has == "yes" => {
                let reason = outcome.reason().map_or("", SafeText::as_str);
                let why = if reason == SKIP_NATIVE_NOT_CONNECTED {
                    "not connected on this board".to_owned()
                } else if let Some(cause) = reason.strip_prefix("not run: ") {
                    format!("not tested ({cause})")
                } else {
                    reason.to_owned()
                };
                if !why.is_empty() {
                    value.push(dot());
                    value.extend(words(&why, gray()));
                }
                Row::new(label, value, DATASHEET)
            }
            _ => Row::new(label, value, DATASHEET),
        };
        rows.push(row);
    }
    if let Some(otg) = field(fields, "usb_otg") {
        rows.push(Row::new("USB OTG", words(otg, Style::new()), DATASHEET));
    }
    rows
}

/// The factory MAC: decoded from the eFuse words when `rom/mac-efuse-reg` read them (marked
/// when they disagree with the ROM's MAC), else the MAC the ROM reported.
fn mac_row(report: &Report) -> Row {
    match read_of(report, MAC_EFUSE_REG) {
        Ok((outcome, read)) => Row::read(
            "MAC",
            outcome,
            words(read.as_str(), Style::new()),
            READ_EFUSE,
            "ROM",
        ),
        Err(reason) => match report.board().mac.as_ref() {
            Some(mac) => Row::new("MAC", words(mac.as_str(), Style::new()), READ_ROM),
            None => Row::missing("MAC", &reason),
        },
    }
}

/// How the ROM identified the chip, marked when it is not the table's.
fn rom_id_row(report: &Report) -> Row {
    match read_of(report, CHIP_IDENTITY) {
        Ok((outcome, read)) => Row::read(
            "ROM id",
            outcome,
            words(read.as_str(), Style::new()),
            READ,
            "expected",
        ),
        Err(reason) => Row::missing("ROM id", &reason),
    }
}

/// A row as lines: label, value (wrapped under itself) and the source flush right on the first
/// line, then the `└` detail lines.
fn row_lines(row: &Row, width: u16) -> Vec<Line<'static>> {
    let source_width = row
        .source
        .map_or(0, |source| source.chars().count() + SOURCE_GAP);
    let room = usize::from(width).saturating_sub(VALUE_COLUMN + MARGIN + source_width);
    let mut value: Vec<(&str, Style)> = row
        .value
        .iter()
        .map(|(word, style)| (word.as_str(), *style))
        .collect();
    if row.flag {
        value.push(("!", yellow()));
    }
    let mut rows = wrap_words(value, room);
    if rows.is_empty() {
        rows.push(Vec::new());
    }

    let mut lines = Vec::with_capacity(rows.len());
    for (index, mut spans) in rows.into_iter().enumerate() {
        if index > 0 {
            spans.insert(0, Span::raw(" ".repeat(VALUE_COLUMN)));
            lines.push(Line::from(spans));
            continue;
        }
        let label = format!("    {:<LABEL_COLUMN$}", row.label);
        spans.insert(0, Span::styled(label, gray()));
        lines.push(match row.source {
            Some(source) => spread(spans, vec![Span::styled(source, gray())], width),
            None => Line::from(spans),
        });
    }

    let detail_room = usize::from(width).saturating_sub(DETAIL_COLUMN + MARGIN);
    let detail = row
        .detail
        .iter()
        .map(|(word, style)| (word.as_str(), *style));
    for (index, mut spans) in wrap_words(detail, detail_room).into_iter().enumerate() {
        let lead = if index == 0 {
            format!("{:VALUE_COLUMN$}└ ", "")
        } else {
            " ".repeat(DETAIL_COLUMN)
        };
        spans.insert(0, Span::styled(lead, gray()));
        lines.push(Line::from(spans));
    }
    lines
}

/// The board's ports from the report, in test order, as the tree of the board list:
/// `├─ COM12  CH343            bridge   primary`. The name comes from the run's [`Board`] (same
/// order, checked by VID:PID); a port that cannot be matched shows its VID:PID instead.
fn port_lines(report: &Report, board: &Board, width: u16) -> Vec<Line<'static>> {
    let infos = &report.board().ports;
    if infos.is_empty() {
        return vec![Line::styled(format!("{:PORT_INDENT$}—", ""), gray())];
    }
    let names: Vec<String> = infos
        .iter()
        .zip(board.ports().map(Some).chain(std::iter::repeat(None)))
        .map(|(info, port)| {
            let same = port
                .and_then(|port| port.usb.as_ref())
                .is_some_and(|usb| usb.vid == info.vid && usb.pid == info.pid);
            match port {
                Some(port) if same => safe(&port.name).as_str().to_owned(),
                _ => format!("{}:{}", info.vid_hex(), info.pid_hex()),
            }
        })
        .collect();
    let longest = names
        .iter()
        .map(|name| name.chars().count())
        .fold(PORT_COLUMN, usize::max);
    let room = usize::from(width).saturating_sub(MARGIN + PORT_INDENT + PORT_TAIL);
    let name_width = longest.min(room.max(1));

    let mut lines = Vec::with_capacity(infos.len());
    for (index, (info, name)) in infos.iter().zip(&names).enumerate() {
        let branch = if index + 1 == infos.len() {
            "└─"
        } else {
            "├─"
        };
        let model = match (&info.bridge_model, info.kind) {
            (Some(model), _) => model.as_str(),
            (None, PortKind::Native) => "USB-Serial/JTAG",
            (None, PortKind::Bridge) => "bridge",
        };
        let kind = info.kind.as_str();
        let mut spans = vec![
            Span::styled(format!("    {branch} "), gray()),
            Span::raw(format!(
                "{:<name_width$} {model:<MODEL_COLUMN$} ",
                fit(name, name_width)
            )),
        ];
        if info.primary {
            spans.push(Span::raw(format!("{kind:<KIND_COLUMN$} ")));
            spans.push(Span::styled("primary", bold()));
        } else {
            spans.push(Span::raw(kind));
        }
        lines.push(Line::from(spans));
    }
    lines
}
