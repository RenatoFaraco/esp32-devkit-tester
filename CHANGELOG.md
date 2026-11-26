# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

Each version has a product requirements document (in Portuguese) in
[`docs/context/prd/`](docs/context/prd/):

| Version | Scope | PRD |
|---------|-------|-----|
| 0.1 | Identify the board through the ROM bootloader, Jest-style report | [v0.1](docs/context/prd/v0.1-identify.md) |
| 0.2 | On-target tests: SRAM, PSRAM, flash, CPU | [v0.2](docs/context/prd/v0.2-on-target-tests.md) |
| 0.3 | JTAG | [v0.3](docs/context/prd/v0.3-jtag.md) |
| 0.4 | Radios and peripherals | [v0.4](docs/context/prd/v0.4-radios-peripherals.md) |
| 1.0 | Release binaries, crates.io, manual | [v1.0](docs/context/prd/v1.0-release.md) |

## [Unreleased]

### Added
- Project plan: PRDs per version, architecture decision records, conventions and roadmap.
- Platform plan: Windows (PowerShell) first, then WSL2, then Linux and macOS (ADR-007).
