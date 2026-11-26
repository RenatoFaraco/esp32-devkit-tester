# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

Each version has a product requirements document (in Portuguese) in
[`docs/context/prd/`](docs/context/prd/):

| Version | Scope | PRD |
|---------|-------|-----|
| 0.0.1 | Foundation (M0): repository, tooling, skeletons, CI | — (criteria in the [roadmap](docs/project/ROADMAP.md), M0) |
| 0.1 | Identify the board through the ROM bootloader, Jest-style report | [v0.1](docs/context/prd/v0.1-identify.md) |
| 0.2 | On-target tests: SRAM, PSRAM, flash, CPU | [v0.2](docs/context/prd/v0.2-on-target-tests.md) |
| 0.3 | JTAG | [v0.3](docs/context/prd/v0.3-jtag.md) |
| 0.4 | Radios and peripherals | [v0.4](docs/context/prd/v0.4-radios-peripherals.md) |
| 1.0 | Release binaries, crates.io, manual | [v1.0](docs/context/prd/v1.0-release.md) |

## [Unreleased]

## [0.0.1] - 2026-11-26

Foundation (milestone M0). Nothing talks to a board yet; this release proves the toolchains and
the guard rails that v0.1 builds on.

### Added
- Project plan: PRDs per version, architecture decision records, conventions and roadmap.
- Platform plan: Windows (PowerShell) first, then WSL2, then Linux and macOS (ADR-007).
- Host (`devkit-tester`, Rust): Cargo workspace with the layer modules, a ratatui start screen
  ("Press any key to test the board"; `q` quits and restores the terminal), and tests that keep
  ratatui/crossterm inside `ui` and OS-specific `cfg` inside `platform`.
- `platform` module: detects WSL1 vs WSL2 from the kernel release string.
- Firmware (C, ESP-IDF v5.5.2): hello project that prints the chip's own view of itself, built for
  ESP32, ESP32-C3, ESP32-C6 and ESP32-S3; the build fails below ESP-IDF 5.5.2.
- CI: host fmt, clippy and tests on Windows and Linux; firmware build matrix in
  `espressif/idf:v5.5.2`; validation of issue forms and workflows.
- Issue forms (task, bug, board bring-up) and pull request template.
- `scripts/github_bootstrap.py` (labels, milestones, M0/M1 issues) and `scripts/Enable-Idf.ps1`
  (ESP-IDF activation on Windows).
- Windows setup guide: `docs/project/SETUP.md`.

### Changed
- ADR-001 accepted: inside ESP-IDF, `-Wall -Wextra -Werror` needs the warnings the IDF disables
  re-enabled one by one as `-Werror=<x>`.

[Unreleased]: https://github.com/RenatoFaraco/esp32-devkit-tester/compare/v0.0.1...HEAD
[0.0.1]: https://github.com/RenatoFaraco/esp32-devkit-tester/releases/tag/v0.0.1
