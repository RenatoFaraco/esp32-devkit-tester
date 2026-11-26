//! Host side of esp32-devkit-tester: detects ESP32 dev boards, talks to them and shows a
//! Jest-style report in a terminal UI.
//!
//! Layering (each layer may only depend on the ones before it):
//! `platform`/`transport` → `probe` → `runner` → `report` → `ui`.
//! `chips` is pure data used by `probe` and `runner`. Only `ui` imports `ratatui`/`crossterm`.

pub mod chips;
pub mod platform;
pub mod probe;
pub mod report;
pub mod runner;
pub mod transport;
pub mod ui;
