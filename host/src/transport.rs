//! Serial ports and raw bytes.
//!
//! Finds ports by USB VID:PID, serial number and description (never by name such as `COMx` or
//! `ttyACMx`), groups the ports of one board, and survives ports that disappear and come back
//! under another name. It may use `platform`, but must not import `probe`, `runner`, `report`,
//! `ui`, `ratatui` or `crossterm`.
