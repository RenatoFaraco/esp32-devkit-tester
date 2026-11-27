//! Layout of the Jest-style report: the body of the `Running` and `Done` screens, the fixed
//! summary under it, and the scroll state of both.
//!
//! Pure: every function builds ratatui [`Line`]s from the model and a width; no I/O and no
//! terminal, so the layout is tested on its own and the `--no-tui` text (#24) can reuse it.
//!
//! # Columns (approved mockup, issue #22)
//! ```text
//!     ✓ crystal              26 MHz !                                     4 ms
//!                            └ table 40 MHz · read 26 MHz
//!                              variable (kept as pass) · ESP32 Series Datasheet
//!                              v5.3, Features; ESP32 Series Comparison
//! ```
//! - The test line is glyph, name, value and the duration flush right. The duration is always
//!   shown; only the value is shortened, with `…`, when the line is too narrow.
//! - Detail lines start with `└` under the value column, continue two cells further in, and
//!   break between words (a word longer than the room, such as a hash, is cut hard). No line is
//!   wider than the width it was laid out for, keeping one cell of margin on the right.
//!
//! Every text shown here is a [`SafeText`] from the model or a host literal: nothing coming
//! from the board reaches the terminal unsanitized.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::{Live, bold, cyan, gray, green, red, safe, spread, yellow};
use crate::report::{Finding, Report, SafeText, State, TestId, TestResult};
use crate::runner::MAC_EFUSE_REG;

/// Width of the test-name column.
const TEST_COLUMN: usize = 20;
/// Where the value starts: `    ✓ ` plus the name column and one space.
const VALUE_COLUMN: usize = 4 + 2 + TEST_COLUMN + 1;
/// Where detail text starts: after `└ ` under the value column.
const DETAIL_COLUMN: usize = VALUE_COLUMN + 2;
/// Cells kept free on the right edge (the same margin as [`spread`]).
const MARGIN: usize = 1;
/// Width of the `expected ` / `read     ` labels, so both values start in the same column.
const LABEL_WIDTH: usize = 9;
/// Width of the `table ` / `read  ` labels of a finding split over two rows.
const FINDING_LABEL_WIDTH: usize = 6;
/// Fewest spaces between a shortened value and the duration, so the two never run together.
const VALUE_GAP: usize = 2;
/// Where the summary text starts after its glyph (` ✓ `).
const SUMMARY_INDENT: usize = 3;

/// Breaks `text` into lines that fit in `width` columns once `indent` columns are taken in front
/// of each one; returns the text of each line without the indent.
///
/// Breaks between words (runs of whitespace become one space). A word longer than the room is
/// cut hard, so no line is ever wider than `width - indent` (at least one column). Counts one
/// column per `char`: the text here is printable ASCII ([`SafeText`]) or the host's own
/// one-column symbols.
pub(super) fn wrap(text: &str, width: usize, indent: usize) -> Vec<String> {
    let words = text.split_whitespace().map(|word| (word, Style::new()));
    wrap_words(words, width.saturating_sub(indent))
        .into_iter()
        .map(|row| row.iter().map(|span| span.content.as_ref()).collect())
        .collect()
}

/// The styled core of [`wrap`]: lays `words` out in rows of at most `room` columns, one space
/// between words, each word keeping its style.
pub(super) fn wrap_words<'w>(
    words: impl IntoIterator<Item = (&'w str, Style)>,
    room: usize,
) -> Vec<Vec<Span<'static>>> {
    let room = room.max(1);
    let mut rows = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    for (word, style) in words {
        let len = word.chars().count();
        if len == 0 {
            continue;
        }
        if used > 0 && used + 1 + len <= room {
            row.push(Span::raw(" "));
            row.push(Span::styled(word.to_owned(), style));
            used += 1 + len;
            continue;
        }
        if used > 0 {
            rows.push(std::mem::take(&mut row));
        }
        let mut rest = word;
        while rest.chars().count() > room {
            let cut = rest
                .char_indices()
                .nth(room)
                .map_or(rest.len(), |(index, _)| index);
            let (head, tail) = rest.split_at(cut);
            rows.push(vec![Span::styled(head.to_owned(), style)]);
            rest = tail;
        }
        used = rest.chars().count();
        row.push(Span::styled(rest.to_owned(), style));
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

/// `text` in at most `max` columns: as is when it fits, else cut with `…` as its last column.
pub(super) fn fit(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = text.chars().take(max - 1).collect();
    out.truncate(out.trim_end().len());
    out.push('…');
    out
}

fn width_of(spans: &[Span]) -> usize {
    spans.iter().map(Span::width).sum()
}

/// The glyph of a finished test: ✓ green, ✕ red, ○ yellow.
fn glyph(state: State) -> Span<'static> {
    match state {
        State::Pass => Span::styled("✓", green()),
        State::Fail => Span::styled("✕", red()),
        State::Skip => Span::styled("○", yellow()),
    }
}

/// `    <glyph> <name>  ` up to the value column.
fn test_prefix(glyph: Span<'static>, id: TestId) -> Vec<Span<'static>> {
    vec![
        Span::raw("    "),
        glyph,
        Span::raw(format!(" {:<TEST_COLUMN$} ", id.name)),
    ]
}

/// One finished test: glyph, name, value (with a yellow `!` when the outcome carries a finding)
/// and the duration flush right (none on a SKIP). The value is what was read or, failing that,
/// what was expected, shortened with `…` only when the line is too narrow for it.
fn test_line(test: &TestResult, width: u16) -> Line<'static> {
    let outcome = &test.outcome;
    let mut left = test_prefix(glyph(outcome.state()), test.id);
    let duration = match outcome.state() {
        State::Skip => None,
        State::Pass | State::Fail => Some(format!("{} ms", outcome.duration_ms())),
    };
    let right_width = duration
        .as_ref()
        .map_or(0, |text| text.chars().count() + VALUE_GAP);
    let room = usize::from(width).saturating_sub(width_of(&left) + MARGIN + right_width);
    let value = outcome
        .read()
        .or(outcome.expected())
        .map_or("", SafeText::as_str);
    match (outcome.finding().is_some(), value.is_empty()) {
        (true, true) => left.push(Span::styled("!", yellow())),
        (true, false) => {
            left.push(Span::raw(fit(value, room.saturating_sub(2))));
            left.push(Span::raw(" "));
            left.push(Span::styled("!", yellow()));
        }
        (false, false) => left.push(Span::raw(fit(value, room))),
        (false, true) => {}
    }
    match duration {
        None => Line::from(left),
        Some(duration) => spread(left, vec![Span::raw(duration)], width),
    }
}

/// `label` (gray, padded to `label_width`) and `value` (default colour) with an optional gray
/// `tag`, the value wrapped in the room left after the label and lined up under itself.
fn labeled_rows(
    (label, label_width): (&str, usize),
    value: &SafeText,
    tag: Option<&'static str>,
    room: usize,
) -> Vec<Vec<Span<'static>>> {
    let words = value
        .as_str()
        .split_whitespace()
        .map(|word| (word, Style::new()))
        .chain(tag.map(|tag| (tag, gray())));
    let mut rows = wrap_words(words, room.saturating_sub(label_width));
    if rows.is_empty() {
        rows.push(Vec::new());
    }
    for (index, row) in rows.iter_mut().enumerate() {
        let lead = if index == 0 {
            format!("{label:<label_width$}")
        } else {
            " ".repeat(label_width)
        };
        row.insert(0, Span::styled(lead, gray()));
    }
    rows
}

/// The rows of a finding: `table <x> · read <y>` (labels gray, values default), then
/// `<kind> (<failed|kept as pass>) · <source>` in gray. When `table … · read …` does not fit
/// on one row, `table` and `read` take a row each, their values lined up (as `expected`/`read`).
fn finding_rows(finding: &Finding, state: State, width: u16) -> Vec<Vec<Span<'static>>> {
    let room = usize::from(width).saturating_sub(DETAIL_COLUMN + MARGIN);
    let values = |text: &SafeText| {
        text.as_str()
            .split_whitespace()
            .map(|word| (word.to_owned(), Style::new()))
            .collect::<Vec<_>>()
    };
    let words: Vec<(String, Style)> = std::iter::once(("table".to_owned(), gray()))
        .chain(values(finding.table()))
        .chain([("·".to_owned(), gray()), ("read".to_owned(), gray())])
        .chain(values(finding.read()))
        .collect();
    let one_row = wrap_words(
        words.iter().map(|(word, style)| (word.as_str(), *style)),
        room,
    );
    let mut rows = if one_row.len() == 1 {
        one_row
    } else {
        let mut rows = labeled_rows(("table", FINDING_LABEL_WIDTH), finding.table(), None, room);
        rows.extend(labeled_rows(
            ("read", FINDING_LABEL_WIDTH),
            finding.read(),
            None,
            room,
        ));
        rows
    };
    let verdict = if state == State::Fail {
        "failed"
    } else {
        "kept as pass"
    };
    let rule = format!("{} ({verdict}) · {}", finding.kind(), finding.source());
    rows.extend(gray_rows(&rule, width));
    rows
}

/// The detail rows under a test:
/// - a finding: its table/read row and its kind/source row (on a FAIL the finding already says
///   why, so the reason, `<field> differs from the capability table`, is not repeated);
/// - a FAIL that compared two values: `expected` and `read` on their own rows, then the reason;
/// - any other FAIL, and every SKIP: the reason.
///
/// `(ROM)`/`(eFuse)` name where each side came from, only in `rom/mac-efuse-reg`.
fn detail_rows(test: &TestResult, width: u16) -> Vec<Vec<Span<'static>>> {
    let outcome = &test.outcome;
    let room = usize::from(width).saturating_sub(DETAIL_COLUMN + MARGIN);
    if let Some(finding) = outcome.finding() {
        return finding_rows(finding, outcome.state(), width);
    }
    let mut rows = Vec::new();
    if outcome.state() == State::Fail
        && let (Some(expected), Some(read)) = (outcome.expected(), outcome.read())
    {
        let (expected_tag, read_tag) = if test.id == MAC_EFUSE_REG {
            (Some("(ROM)"), Some("(eFuse)"))
        } else {
            (None, None)
        };
        rows.extend(labeled_rows(
            ("expected", LABEL_WIDTH),
            expected,
            expected_tag,
            room,
        ));
        rows.extend(labeled_rows(("read", LABEL_WIDTH), read, read_tag, room));
    }
    if let Some(reason) = outcome.reason() {
        rows.extend(gray_rows(reason.as_str(), width));
    }
    rows
}

/// Plain gray detail text wrapped under the detail column.
fn gray_rows(text: &str, width: u16) -> Vec<Vec<Span<'static>>> {
    wrap(
        text,
        usize::from(width).saturating_sub(MARGIN),
        DETAIL_COLUMN,
    )
    .into_iter()
    .map(|row| vec![Span::styled(row, gray())])
    .collect()
}

/// Detail rows as lines: `└ ` under the value column on the first, two cells further in after.
fn detail_lines(rows: Vec<Vec<Span<'static>>>) -> Vec<Line<'static>> {
    rows.into_iter()
        .enumerate()
        .map(|(index, mut row)| {
            let lead = if index == 0 {
                format!("{:VALUE_COLUMN$}└ ", "")
            } else {
                " ".repeat(DETAIL_COLUMN)
            };
            row.insert(0, Span::styled(lead, gray()));
            Line::from(row)
        })
        .collect()
}

/// A finished test and its detail lines.
fn result_lines(test: &TestResult, width: u16) -> Vec<(TestId, Line<'static>)> {
    let mut lines = vec![(test.id, test_line(test, width))];
    lines.extend(
        detail_lines(detail_rows(test, width))
            .into_iter()
            .map(|line| (test.id, line)),
    );
    lines
}

/// Suite headers and the lines under them, in catalog order; a suite appears with its first line.
fn suite_lines(tests: impl Iterator<Item = (TestId, Line<'static>)>) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut suite = None;
    for (id, line) in tests {
        if suite != Some(id.suite) {
            suite = Some(id.suite);
            lines.push(Line::styled(format!("  {}", id.suite.as_str()), bold()));
        }
        lines.push(line);
    }
    lines
}

/// The body of the `Running` screen: finished tests with their details, the one in flight with
/// `spinner` (and the port it waits for), then `…` while tests are still pending.
pub(super) fn running_lines(live: &Live, spinner: &str, width: u16) -> Vec<Line<'static>> {
    let mut pending = false;
    let shown = live.plan.iter().flat_map(|id| {
        if let Some(test) = live.results.iter().find(|test| test.id == *id) {
            return result_lines(test, width);
        }
        if live.current == Some(*id) {
            let mut spans = test_prefix(Span::styled(spinner.to_owned(), cyan()), *id);
            if let Some(port) = &live.waiting {
                let room = usize::from(width).saturating_sub(width_of(&spans) + MARGIN);
                let text = format!("waiting for {} to come back…", safe(&port.name));
                spans.push(Span::styled(fit(&text, room), gray()));
            }
            return vec![(*id, Line::from(spans))];
        }
        pending = true;
        Vec::new()
    });
    let mut lines = suite_lines(shown);
    if pending {
        lines.push(Line::styled("    …", gray()));
    }
    lines
}

/// The scrolling body of the `Done` screen: every test with its details, by suite.
pub(super) fn done_lines(report: &Report, width: u16) -> Vec<Line<'static>> {
    suite_lines(
        report
            .tests()
            .iter()
            .flat_map(|test| result_lines(test, width)),
    )
}

/// The fixed block under the `Done` body: `Tests: …` and whether the board is back to its app
/// (a long reason for a missed reset wraps under the text, it is never cut).
pub(super) fn summary_lines(report: &Report, width: u16) -> Vec<Line<'static>> {
    let summary = report.summary();
    let mut spans = vec![Span::styled(" Tests: ", bold())];
    let mut counts = Vec::new();
    if summary.failed > 0 {
        counts.push(Span::styled(format!("{} failed", summary.failed), red()));
    }
    if summary.passed > 0 {
        counts.push(Span::styled(format!("{} passed", summary.passed), green()));
    }
    if summary.skipped > 0 {
        counts.push(Span::styled(
            format!("{} skipped", summary.skipped),
            yellow(),
        ));
    }
    counts.push(Span::raw(format!("{} total", summary.total)));
    for (index, count) in counts.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(", "));
        }
        spans.push(count);
    }
    let mut lines = vec![Line::from(spans)];

    let (state, rows) = match report.hard_reset().reason() {
        None => (State::Pass, vec![vec![Span::raw("board back to its app")]]),
        Some(reason) => {
            let words = "board not back to its app"
                .split_whitespace()
                .map(|word| (word, red()))
                .chain(std::iter::once(("—", gray())))
                .chain(
                    reason
                        .as_str()
                        .split_whitespace()
                        .map(|word| (word, gray())),
                );
            let room = usize::from(width).saturating_sub(SUMMARY_INDENT + MARGIN);
            (State::Fail, wrap_words(words, room))
        }
    };
    for (index, mut row) in rows.into_iter().enumerate() {
        let lead = if index == 0 {
            vec![Span::raw(" "), glyph(state), Span::raw(" ")]
        } else {
            vec![Span::raw(" ".repeat(SUMMARY_INDENT))]
        };
        row.splice(0..0, lead);
        lines.push(Line::from(row));
    }
    lines
}

/// One step of scrolling, from a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Scroll {
    /// One line up.
    Up,
    /// One line down.
    Down,
    /// One page up.
    PageUp,
    /// One page down.
    PageDown,
    /// To the first line.
    Home,
    /// To the last line; on `Running`, follows new lines again.
    End,
}

/// Where the report body is scrolled to.
///
/// `offset` is the first visible line; `follow` keeps the last line in view as lines arrive
/// (the `Running` screen starts following; scrolling up stops it, reaching the bottom again,
/// with `↓`, `PgDn` or `End`, turns it back on). The offset is clamped against the content and
/// the viewport every time it is used, so a resize or a shorter body never shows blank lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct ScrollState {
    offset: usize,
    follow: bool,
}

impl ScrollState {
    /// At the first line, staying there (the `Done` screen).
    pub(super) fn top() -> Self {
        ScrollState {
            offset: 0,
            follow: false,
        }
    }

    /// Following the last line (the `Running` screen).
    pub(super) fn following() -> Self {
        ScrollState {
            offset: 0,
            follow: true,
        }
    }

    /// Whether new lines keep the view at the bottom.
    #[cfg(test)]
    pub(super) fn follows(&self) -> bool {
        self.follow
    }

    /// The first visible line for `content` lines in a `viewport` of that many lines.
    pub(super) fn offset(&self, content: usize, viewport: usize) -> usize {
        let max = content.saturating_sub(viewport);
        if self.follow {
            max
        } else {
            self.offset.min(max)
        }
    }

    /// Lines hidden above and below the viewport.
    pub(super) fn hidden(&self, content: usize, viewport: usize) -> (usize, usize) {
        let offset = self.offset(content, viewport);
        (offset, content.saturating_sub(offset + viewport))
    }

    /// Moves by one `step`, clamped to the content. A page is the viewport minus one line, so
    /// one line of context stays on screen.
    pub(super) fn apply(&mut self, step: Scroll, content: usize, viewport: usize) {
        let max = content.saturating_sub(viewport);
        let now = self.offset(content, viewport);
        let page = viewport.saturating_sub(1).max(1);
        let next = match step {
            Scroll::Up => now.saturating_sub(1),
            Scroll::Down => now.saturating_add(1),
            Scroll::PageUp => now.saturating_sub(page),
            Scroll::PageDown => now.saturating_add(page),
            Scroll::Home => 0,
            Scroll::End => max,
        };
        self.offset = next.min(max);
        self.follow = self.offset == max;
    }
}

/// The right end of the footer when part of the body is hidden: `↓ 6 more` below,
/// `↑ 3 more` above, `↑ 3  ↓ 6 more` both; nothing when everything fits.
pub(super) fn more_indicator(above: usize, below: usize) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if above > 0 {
        spans.push(Span::styled("↑", bold()));
        spans.push(Span::styled(format!(" {above}"), gray()));
    }
    if below > 0 {
        if above > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled("↓", bold()));
        spans.push(Span::styled(format!(" {below}"), gray()));
    }
    if !spans.is_empty() {
        spans.push(Span::styled(" more", gray()));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_breaks_on_words_and_hard_breaks_huge_words() {
        assert_eq!(
            wrap(
                "chip has no USB-Serial/JTAG; JTAG needs an external probe",
                30,
                0
            ),
            [
                "chip has no USB-Serial/JTAG;",
                "JTAG needs an external probe"
            ]
        );
        // The indent takes room from every line.
        assert_eq!(wrap("aa bb cc", 10, 4), ["aa bb", "cc"]);
        // Runs of whitespace are one space; blank text has no lines.
        assert_eq!(wrap("  a \t  b  ", 80, 0), ["a b"]);
        assert!(wrap("   ", 80, 0).is_empty());
        // A word wider than the room is cut hard, the rest flows on.
        assert_eq!(
            wrap("hash 0123456789abcdef end", 8, 0),
            ["hash", "01234567", "89abcdef", "end"]
        );
        // Exactly the room fits; one more column breaks.
        assert_eq!(wrap("abcd efgh", 9, 0), ["abcd efgh"]);
        assert_eq!(wrap("abcd efgh", 8, 0), ["abcd", "efgh"]);
        // No room at all still makes progress, one column per line.
        assert_eq!(wrap("ab", 3, 5), ["a", "b"]);
        for width in [60, 80] {
            for line in wrap(&"word ".repeat(40), width, 29) {
                assert!(line.chars().count() <= width - 29, "{line}");
            }
        }
    }

    #[test]
    fn fit_shortens_with_an_ellipsis_only_when_needed() {
        assert_eq!(fit("abcdef", 6), "abcdef");
        assert_eq!(fit("abcdef", 5), "abcd…");
        assert_eq!(fit("abcdef", 0), "");
    }

    #[test]
    fn scroll_state_clamps_and_follows() {
        let mut scroll = ScrollState::following();
        assert_eq!(scroll.offset(30, 10), 20);
        scroll.apply(Scroll::Up, 30, 10);
        assert_eq!((scroll.offset(30, 10), scroll.follows()), (19, false));
        // More content does not move a view the user scrolled.
        assert_eq!(scroll.offset(40, 10), 19);
        scroll.apply(Scroll::End, 40, 10);
        assert_eq!((scroll.offset(40, 10), scroll.follows()), (30, true));
        // A taller viewport (resize) clamps instead of showing blank lines.
        let mut scroll = ScrollState::top();
        scroll.apply(Scroll::PageDown, 30, 10);
        assert_eq!(scroll.offset(30, 10), 9);
        assert_eq!(scroll.offset(30, 25), 5);
        assert_eq!(scroll.hidden(30, 25), (5, 0));
        assert!(more_indicator(0, 0).is_empty());
    }
}
