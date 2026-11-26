//! Operating-system differences (ADR-007).
//!
//! This is the only module allowed to use `#[cfg(windows)]`, `#[cfg(unix)]` or other
//! OS-specific APIs (e.g. WSL detection). Everything else stays portable. It must not import
//! `probe`, `runner`, `report`, `ui`, `ratatui` or `crossterm`.
