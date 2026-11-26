//! Pure, serializable result model: PASS/FAIL/SKIP per suite, durations and the
//! `Tests: X failed, Y passed, Z skipped, N total` summary.
//!
//! No I/O and no terminal: testable on its own. It must not import `transport`, `probe`,
//! `ui`, `ratatui` or `crossterm`.
