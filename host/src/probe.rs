//! Talking to the chip: ROM bootloader (via the espflash library API) and JTAG.
//!
//! Read-only by default (ADR-003). It may use `transport`, `platform` and `chips`, but must not
//! import `runner`, `report`, `ui`, `ratatui` or `crossterm`.
