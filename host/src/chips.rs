//! Per-chip capabilities as data (ADR-004): one table, no scattered `if chip == ...`.
//!
//! Pure data with no I/O. It must not import `transport`, `probe`, `runner`, `report`, `ui`,
//! `ratatui` or `crossterm`.
