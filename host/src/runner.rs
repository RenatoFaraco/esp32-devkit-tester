//! Runs test suites against a board: ordering, timeouts and SKIP when a test does not apply.
//!
//! Testable without a board (fake transport plus byte fixtures). It may use `probe`, `chips`
//! and the `report` model, but must not import `ui`, `ratatui` or `crossterm`.
