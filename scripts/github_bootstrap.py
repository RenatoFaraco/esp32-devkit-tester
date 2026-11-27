"""Bootstrap GitHub labels, milestones and issues for esp32-devkit-tester.

Records the initial project plan as data, derived from docs/project/GITHUB.md (labels,
milestones), docs/project/ROADMAP.md (M0) and docs/context/prd/v0.1-identify.md (M1, H1-H9).
Idempotent: labels, milestones and issues are matched by name/title and only created when
missing; labels listed here are added to existing issues. Bodies and descriptions are not
updated after creation, so the title is the key: do not rename an issue here once applied.
Issues exist only for M0 and M1; the other milestones get issues when their PRD becomes current.

Usage (requires an authenticated GitHub CLI, `gh auth login`):
    python scripts/github_bootstrap.py           # dry run
    python scripts/github_bootstrap.py --apply
"""

# ruff: noqa: E501  (issue text is data; long lines read better than split strings)
import json
import shutil
import subprocess
import sys

GH = shutil.which("gh") or r"C:\Program Files\GitHub CLI\gh.exe"
REPO = "RenatoFaraco/esp32-devkit-tester"
APPLY = "--apply" in sys.argv

# Chips on the bench (docs/context/hardware.md); one `chip:<name>` label each.
# On the bench: ESP32, C3, C6, S3. C5 and P4 boards on their way; H2 not bought yet
# (label now, bring-up issue when a board exists).
CHIPS = [
    ("esp32", "ESP32"),
    ("esp32c3", "ESP32-C3"),
    ("esp32c5", "ESP32-C5"),
    ("esp32c6", "ESP32-C6"),
    ("esp32s3", "ESP32-S3"),
    ("esp32p4", "ESP32-P4"),
    ("esp32h2", "ESP32-H2"),
]

LABELS = [
    ("host", "Rust host: TUI, probe, runner, report", "fbca04"),
    ("fw", "Firmware in C / ESP-IDF", "0e8a16"),
    ("spec", "Host <-> board contract (docs/context/spec)", "1d76db"),
    ("ci", "GitHub Actions and tooling", "5319e7"),
    ("board", "Bring-up or validation on a specific board", "b60205"),
    ("test", "Test infrastructure or acceptance criterion", "c2e0c6"),
    ("docs", "Documentation", "0075ca"),
    ("release", "Release preparation", "0052cc"),
] + [(f"chip:{key}", f"Specific to the {name}", "c5def5") for key, name in CHIPS]

MILESTONES = [
    (
        "M0 · Foundation",
        "v0.0.1 — Repo and bootstrap script, issue/PR templates, Cargo workspace with TUI skeleton and platform module, ESP-IDF hello for the bench targets, CI green on Windows and Linux.",
        "open",
    ),
    (
        "M1 · Identify (v0.1)",
        "v0.1.0 — Read-only through the ROM bootloader (nothing written to flash or eFuse): ports classified by VID:PID and grouped per board; chip, revision, MAC, eFuses, flash ID/size, declared PSRAM and ID registers; Jest-style report, Board panel, --no-tui/--json.",
        "open",
    ),
    (
        "M2 · On-target tests (v0.2)",
        "v0.2.0 — Test firmware (C/ESP-IDF, one build per chip) loaded only after explicit confirmation; on-target SRAM, PSRAM, flash (scratch partition only), CPU and timer tests in the same Jest-style report via a versioned result-line protocol (ADR-005).",
        "open",
    ),
    (
        "M3 · JTAG (v0.3)",
        "v0.3.0 — JTAG: TAP IDCODE, halt/resume per core, registers and memory read over JTAG vs. UART; built-in USB-Serial/JTAG or external probe (original ESP32 without a probe → SKIP). Backend per ADR-006.",
        "open",
    ),
    (
        "M4 · Radios & peripherals (v0.4)",
        "v0.4.0 — Radios and board peripherals per chip capabilities: Wi-Fi scan (5 GHz on C5), BLE, 802.15.4, P4 radio coprocessor; LED/BOOT button (interactive), optional GPIO loopback and ADC. Absent → SKIP.",
        "open",
    ),
    (
        "M5 · Release (v1.0)",
        "v1.0.0 — Ready without cloning: Windows binary (Linux build marked unvalidated) with the test firmware bundled, crates.io, manual in pt-BR and English, all 7 families (incl. ESP32-H2) validated on boards.",
        "open",
    ),
]


def body(summary, scope, done, refs, depends=None):
    lines = [summary, "", "### Scope"] + [f"- {s}" for s in scope]
    lines += ["", "### Done when"] + [f"- [ ] {d}" for d in done]
    if depends:
        lines += ["", f"**Depends on:** {depends}"]
    lines += ["", f"**Refs:** {refs}"]
    return "\n".join(lines)


M0, M1 = (m[0] for m in MILESTONES[:2])


def bring_up(key, name, extra_scope=(), extra_done=()):
    """Bring-up issue for one bench board (PRD v0.1 §7 criteria 1 and 2)."""
    return (
        M1,
        ["board", f"chip:{key}"],
        f"board: bring-up {name}",
        body(
            f"Record what the {name} board on the bench really is, then check `devkit-tester` against it.",
            [
                "Part 1 (start of M1, no devkit-tester needed): exact board model, ports and VID:PID (Device Manager), chip and revision from `esptool chip_id` (record the esptool version and exact command), probe fixture captured → unblocks H2 and the probe seam tests",
                "Part 2 (after H6/H7): `devkit-tester` run compared with the Part 1 reference",
                "Flash size / PSRAM read by the probe vs. datasheet and seller listing",
                *extra_scope,
            ],
            [
                "`devkit-tester` reports the same chip and revision as `esptool chip_id` (criterion 1)",
                "Board panel shown and matches the board",
                f"{name} row of `docs/context/hardware.md` filled, with datasheet/TRM sources",
                f"Probe fixture `host/tests/fixtures/{key}.json` committed",
                *extra_done,
            ],
            "PRD v0.1 §7 criteria 1–2 · hardware.md · ADR-004",
        ),
    )


ISSUES = [
    # ---------------- M0 ----------------
    (
        M0,
        ["docs"],
        "docs: repository metadata and social preview",
        body(
            "Make the repository findable: About, topics and social preview as in GITHUB.md.",
            [
                "Description and the 20 topics from GITHUB.md (set at repo creation)",
                "Provisional social preview 1280×640 from the TUI start screen (replaced by a `Tests: … passed` capture in the v0.1.0 release)",
                "Cross-link with esp32-logcat in both READMEs",
            ],
            [
                "About and topics match GITHUB.md exactly",
                "Social preview uploaded (Settings → Social preview)",
            ],
            "GITHUB.md · ROADMAP M0",
            depends="host: Cargo workspace and TUI skeleton (for the capture)",
        ),
    ),
    (
        M0,
        ["ci"],
        "ci: scripts/github_bootstrap.py",
        body(
            "Record the plan as data: labels, milestones M0–M5 and the M0/M1 issues, like esp32-logcat.",
            [
                "Labels and milestones from GITHUB.md; issues from ROADMAP M0 and PRD v0.1 (H1–H9)",
                "Idempotent (match by name/title), dry run by default, `--apply` to create",
            ],
            [
                "Second `--apply` run prints no changes",
                "Closed right after the first `--apply` (it creates itself)",
            ],
            "GITHUB.md · ROADMAP M0",
        ),
    ),
    (
        M0,
        ["ci"],
        "ci: issue and PR templates",
        body(
            "Templates in `.github/` so every issue has Scope, Done when and Refs.",
            [
                "Issue forms: feature/task, bug, **board bring-up** (chip, board model, ports + VID:PID, `esptool chip_id` output and esptool version)",
                "PR template: linked issue, tests run, docs/ROADMAP updated, contract change → spec",
            ],
            ["New issue page offers the three templates; PR template shows on a new PR"],
            "ROADMAP M0 · CONVENTIONS §2, §7",
        ),
    ),
    (
        M0,
        ["host"],
        "host: Cargo workspace and TUI skeleton",
        body(
            "Workspace in `host/` with the crate and binary that later versions build on.",
            [
                "Crate `esp32-devkit-tester`, binary `devkit-tester`, Rust stable, edition 2024",
                "Empty layer modules: `platform`, `transport`, `probe`, `chips`, `runner`, `report`, `ui`",
                'ratatui + crossterm screen "Press any key to test the board"; `q` exits and restores the terminal',
                "`Cargo.lock` committed (pins espflash later, ADR-002)",
            ],
            [
                "`cargo run` shows the screen and `q` exits cleanly in PowerShell",
                "fmt, clippy `-D warnings` and `cargo test` clean",
                "Only `ui` depends on ratatui/crossterm",
            ],
            "ROADMAP M0 · ADR-002 · CONVENTIONS §4",
        ),
    ),
    (
        M0,
        ["host"],
        "host: platform module as the OS extension point",
        body(
            "Single place for OS differences so WSL2, Linux and macOS arrive without rewrites.",
            [
                "`platform` module: the only place allowed to use `#[cfg(windows)]` / `#[cfg(unix)]`",
                "WSL detection exposed as WSL1 vs. WSL2 (H9 needs the WSL1 message); detection rule documented with source; no special behavior yet",
            ],
            [
                "Compiles and tests on Windows and Linux",
                "No `cfg` on OS outside `platform` (checked in review)",
            ],
            "ADR-007 · ROADMAP M0",
        ),
    ),
    (
        M0,
        ["fw"],
        "fw: ESP-IDF hello project for the bench targets",
        body(
            "Smallest ESP-IDF project that proves the toolchain for every bench chip.",
            [
                "`firmware/` with `idf.py`, C17, `-Wall -Wextra -Werror`",
                "Builds for the boards on the bench: esp32, esp32c3, esp32c6, esp32s3 (esp32c5 and esp32p4 join in their M1 bring-up)",
                "Pin ESP-IDF ≥ v5.5.2 so C5 (≥ v5.5.1) and P4 rev3 (v5.5.2) can be added later without an IDF upgrade",
                "Accept or revise ADR-001 with what was learned",
            ],
            [
                "`idf.py set-target <t> build` passes for the 4 targets",
                "ADR-001 status updated (accepted or revised) and docs/README.md table synced",
            ],
            "ROADMAP M0 · ADR-001",
        ),
    ),
    (
        M0,
        ["ci", "host"],
        "ci: host fmt, clippy and tests on Windows and Linux",
        body(
            "Catch non-portable host code from day one.",
            [
                "GitHub Actions matrix `windows-latest` + `ubuntu-latest`",
                "`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`",
                "Cargo cache",
            ],
            ["Workflow green on a push to `main` and on PRs"],
            "ADR-007 · ROADMAP M0",
            depends="host: Cargo workspace and TUI skeleton",
        ),
    ),
    (
        M0,
        ["ci", "fw"],
        "ci: firmware build matrix on espressif/idf",
        body(
            "Build the firmware for every bench target with the pinned IDF image.",
            [
                "Matrix over the 4 bench targets (esp32, esp32c3, esp32c6, esp32s3) using the `espressif/idf:<pinned>` container",
                "Run only when `firmware/` or the workflow changes; with a `paths` filter, do not make the job a required check (or add a skip job that reports success), or PRs stay pending forever",
            ],
            ["All targets build green in CI"],
            "ADR-001 · ROADMAP M0",
            depends="fw: ESP-IDF hello project for the bench targets",
        ),
    ),
    (
        M0,
        ["docs"],
        "docs: pin build/test commands and PowerShell environment setup",
        body(
            "Exact commands written once, used by people and by the agents.",
            [
                "Host and firmware build/test commands in `CLAUDE.md` and `.claude/agents/test-runner.md`",
                "User setup in PowerShell: ESP-IDF install/activation, `cargo`",
            ],
            ["A clean PowerShell session can build host and firmware following only the docs"],
            "ROADMAP M0 · CLAUDE.md · CONVENTIONS §5",
        ),
    ),
    (
        M0,
        ["release"],
        "release: close M0 and tag v0.0.1",
        body(
            "Close the foundation milestone like every milestone closes.",
            [
                "CHANGELOG `0.0.1` entry (Keep a Changelog)",
                "ROADMAP M0 items checked",
                "Tag `v0.0.1` on a green `main`",
            ],
            ["CI green on the tagged commit; tag and CHANGELOG entry exist"],
            "ROADMAP · CONVENTIONS §2",
        ),
    ),
    # ---------------- M1 ----------------
    (
        M1,
        ["docs"],
        "docs: ADRs for board identity, read-only scope, report contract and test seam",
        body(
            "Decide the non-obvious points of M1 before writing the code that depends on them (architect).",
            [
                "ADR-008 board identity and port grouping: identity = chip MAC; native port carries it as USB serial number, bridge only after probing; ports opened/reset only on user action; ports of one board probed sequentially",
                "ADR-009 what read-only means in v0.1 (refines ADR-003): ROM only vs. RAM stub, allowed register writes (SPI controller for RDID, baud change), board state at the end (hard reset back to the app)",
                "ADR-010 report as a contract: states (PASS/FAIL/SKIP + table-vs-read finding), `--json` schema versioned in `spec/`, exit codes, behavior without `--port` when several boards are connected",
                "ADR-011 test seam and host concurrency: `Probe` trait above espflash (its `Port` is a concrete type), fact fixtures per chip, worker thread + channel, no async runtime",
            ],
            ["Four ADRs in `docs/context/decisions/` (status at least `proposta`) and listed in docs/README.md"],
            "ADR-002 · ADR-003 · ADR-004 · ADR-007 · PRD v0.1 §6",
        ),
    ),
    (
        M1,
        ["spec", "docs"],
        "spec: report-json.md — `--json` report schema v1 (ADR-010)",
        body(
            "Write the contract that `--json` consumers (scripts, CI) rely on, before H8 implements it (architect).",
            [
                "`docs/context/spec/report-json.md` with integer `schema_version` = 1 at the top",
                "Board (chip, revision, MAC, ports), suites, tests (suite, name, state, reason, duration ms, expected/read), `finding` annotation, summary counts",
                "Versioning rules from ADR-010: additive field keeps the version and consumers ignore unknown fields; removal, rename or change of meaning bumps it",
                "Board text sanitized (no control characters or escape sequences); JSON only on stdout",
                "One complete example per state (pass, fail, skip with `not run`, pass with finding)",
            ],
            [
                "Spec listed in `docs/context/spec/README.md` as current for v0.1",
                "Examples usable as test fixtures by H8",
            ],
            "ADR-010 · PRD v0.1 H8, §7 criterion 5",
            depends="ADR-010",
        ),
    ),
    (
        M1,
        ["host"],
        "host: chip capability table with sources (ADR-004)",
        body(
            "One table that drives PASS/FAIL/SKIP and the Board panel.",
            [
                "`chips` module: core, cores, clock, radios, USB-Serial/JTAG, PSRAM options, ID registers per chip",
                "Rows for ESP32, C3, C5, C6, S3, P4, H2, each with datasheet/TRM source",
                "Values read from the board override the table; disagreement becomes a finding (ADR-010)",
                "Each field is marked `invariant` (silicon: ID registers, family, cores, USB-Serial/JTAG presence) or `variable` (module/package: flash size, PSRAM, package, crystal); a divergence on an invariant field is FAIL, on a variable one PASS with finding (ADR-010)",
            ],
            [
                "Tests: every bench chip has a complete row; ESP32 marks built-in USB-Serial/JTAG absent",
                "Every field carries its invariant/variable mark (test over the table)",
                "Adding a chip = adding a row (no `if chip ==` elsewhere)",
            ],
            "ADR-004 · ADR-010 · PRD v0.1 H5, H7",
        ),
    ),
    (
        M1,
        ["host", "test"],
        "test: probe seam with fakes and recorded fixtures",
        body(
            "Test grouping, runner, report and error handling without a board.",
            [
                "`Probe` trait in `probe` returning facts (chip, revision, MAC, eFuse words, flash ID, register reads); espflash is the only real impl — its `Connection` takes a concrete `serialport::COMPort`/`TTYPort`, so bytes cannot be injected below it",
                "`FakeProbe` replays recorded facts per board (`host/tests/fixtures/<chip>.json`, captured during bring-up) and fails on demand (timeout, disconnect, busy)",
                "Raw-byte fixtures (exact bytes, CRLF preserved) only for parsers we own: port classification, sanitization",
            ],
            ["Grouping, runner, report and H9 tests run in CI with no board"],
            "ADR-011 · CONVENTIONS §6 · PRD v0.1 §7 criterion 5",
            depends="ADR-011",
        ),
    ),
    (
        M1,
        ["host", "test"],
        "test: guard that the probe never writes flash, eFuse or RAM (ADR-009)",
        body(
            "Make the read-only scope of v0.1 a failing test instead of a promise.",
            [
                "Guard test in `host/tests/` in the style of `layering.rs`: fails if `probe` mentions espflash write/erase/flash, eFuse write, `write_reg` or RAM-load (stub/`load_ram`) APIs",
                "Closed allowlist of espflash calls the real `Probe` may use, kept next to the test with a link to ADR-009",
                "Connection built with `use_stub = false` and `baud = None` (115200), asserted by the test",
            ],
            [
                "Guard test fails on a deliberately added write call (checked once, then reverted)",
                "Runs in CI on Windows and Linux with no board",
            ],
            "ADR-003 · ADR-009 · ADR-011",
            depends="probe seam",
        ),
    ),
    (
        M1,
        ["host"],
        "host: enumerate and classify serial ports by VID:PID (H2)",
        body(
            "Know what each port is before opening it.",
            [
                "Enumerate with `serialport`; classify native USB-Serial/JTAG (`303A:1001`), CP210x, CH340/CH340K, CH343/CH9102, FTDI",
                'VID:PID table as data next to `hardware.md`; unknown VID:PID listed as "unknown", not dropped',
                "FT2232 (`0403:6010`, ESP-Prog): only the UART interface is a probe candidate; the JTAG interface is listed, not probed (verify whether `serialport` exposes the interface number on Windows; otherwise flag both)",
                "Never match on port names (`COMx`, `ttyACMx`)",
            ],
            ["Unit tests over fake port lists cover every VID:PID in hardware.md and an unknown one"],
            "PRD v0.1 H2 · ADR-007 · hardware.md",
        ),
    ),
    (
        M1,
        ["host"],
        "host: enter ROM bootloader and sync on every port (H3)",
        body(
            "Reset into download mode and sync with the ROM; each port of the board is a test.",
            [
                "Bridges: classic DTR/RTS sequence; native USB-Serial/JTAG: its own reset sequence (espflash)",
                "Tolerate the native port disappearing and coming back on reset (Windows may renumber the COM)",
                "Open ports with DTR/RTS released so the board is not held in reset (hardware.md traps)",
                "Windows `usbser.sys`: rewrite DTR after changing RTS (hardware.md); if we implement it, it lives in `platform`",
                '`port` suite has a `usb-serial-jtag` test driven by the `chips` table: chip without it → SKIP "chip has no USB-Serial/JTAG; JTAG needs an external probe (GPIO12–15)"; chip with it but no `303A:1001` port in the group → SKIP "native USB not connected on this board"; port present → its sync result',
            ],
            [
                "Sync succeeds on both ports of a dual-port board and on single-port boards",
                "Each port produces a `port` suite test with duration",
                "ESP32 shows `usb-serial-jtag` as SKIP with reason; C6/S3 show it as PASS (criterion 2)",
            ],
            "PRD v0.1 H3, §7 criterion 2 · ADR-002 · ADR-004 · ADR-007 · hardware.md",
            depends="probe seam, H2 enumerate, chips table",
        ),
    ),
    (
        M1,
        ["host"],
        "host: probe chip, revision, MAC, eFuses, flash and PSRAM (H4)",
        body(
            "Everything the ROM can tell us without writing anything.",
            [
                "Through the espflash library (never its `cli` module): chip family, silicon revision, crystal",
                "MAC, feature/package eFuses, flash JEDEC ID and size, embedded PSRAM declared by eFuse",
                "ROM only, no RAM stub, unless ADR-009 says otherwise; register writes needed to read the flash JEDEC ID via the SPI controller are allowed and listed",
                "No flash or eFuse writes (ADR-003)",
            ],
            [
                "Results feed the `rom`, `efuse` and `flash` suites",
                "Chip and revision match `esptool chip_id` on a bench board",
                "Gaps in espflash's public API worked around in `probe` and recorded in an ADR (PRD §6)",
            ],
            "PRD v0.1 H4, §6 · ADR-002 · ADR-003",
            depends="H3, ADR-009",
        ),
    ),
    (
        M1,
        ["host"],
        "host: identification registers via READ_REG (H5)",
        body(
            "Read known identification registers per chip and compare with the capability table.",
            [
                "Register addresses and expected values come from the `chips` table, with TRM section",
                "What READ_REG checks (chips table, #12): on ESP32 the ROM magic register `0x40001000 == 0x00F01D83`; on every chip the factory MAC re-read from its eFuse address (`mac_efuse_reg`) must equal the MAC the ROM reported. Newer chips identify through `GET_SECURITY_INFO` chip id, not a register",
                "Only read-safe addresses (always-on blocks: eFuse, ID/system, ROM magic); a peripheral without clock may return garbage or hang the ROM",
                "Mismatch → FAIL with expected vs. read; chip without the register → SKIP with reason",
                "Results go to the `rom` suite (the PRD's four suites are kept; no `reg` suite)",
            ],
            ["Register tests pass on the bench boards; unit tests with fake reads cover FAIL and SKIP"],
            "PRD v0.1 H5 · ADR-004",
            depends="H3, chips table",
        ),
    ),
    (
        M1,
        ["host"],
        "host: group the ports of one board (H2)",
        body(
            "A board with native USB + bridge shows up as **one** board with two ports.",
            [
                "Before probing: native port identity = its USB serial number (the chip MAC, set by the USB-Serial/JTAG descriptor); bridge ports stay ungrouped (two cables share no USB location)",
                "After probing: a bridge port joins the group whose MAC matches",
                "Ports are opened/reset only on user action, never on hot-plug (DTR/RTS on an unknown bridge may reset a non-ESP device)",
            ],
            [
                "Test `groups_native_and_bridge_ports_of_one_board` with fake probe results",
                "Every group has exactly one `primary` port (the bridge when present, else the native one; ADR-008 §5), so the report's `BoardInfo` is built valid; test `each_group_has_exactly_one_primary_port`",
                "C6/S3 board shows as one board, one test per port (criterion 2)",
            ],
            "PRD v0.1 H2, §7 criterion 2 · ADR-008",
            depends="H3, H4, probe seam, ADR-008",
        ),
    ),
    (
        M1,
        ["host"],
        "host: start screen with the detected-board list (H1)",
        body(
            "First screen of the TUI: what is connected and how to start.",
            [
                '"Press any key to test the board" plus the list of detected boards (grouped ports)',
                "List refreshes on hot-plug (enumeration only, no port opened); `q` exits",
            ],
            ["Plugging/unplugging a board updates the list without restarting"],
            "PRD v0.1 H1",
            depends="H2 enumerate, grouping",
        ),
    ),
    (
        M1,
        ["host"],
        "host: runner — run the selected board off the UI thread (H1, H3)",
        body(
            "Turn a key press into a run whose events feed the report while the TUI stays responsive.",
            [
                "Identification step first (ADR-008): unidentified bridges are listed and opened only after an explicit confirmation in the TUI (each one gets a reset); declined bridges stay ungrouped",
                "Then the key press runs the selected board (one board → no prompt); only that board's ports are opened",
                "Worker thread (`std::thread` + channel): espflash is blocking; `ui` only receives events",
                "Ports of one board are probed sequentially, never in parallel (one chip, one ROM)",
                "After a port's hard reset the native `303A:1001` port re-enumerates (seen on C3/C6: back in < 2 s); wait for it by USB serial number (bounded, e.g. 5 s) before probing it, never by COM name (hardware.md)",
                "Per-test timeout: a hung port becomes FAIL, never a frozen run",
                "Measures each test through a `Clock` trait (fake clock in tests) so durations are deterministic; the report only receives `duration_ms`",
                "`q` during a run cancels it and restores the terminal",
            ],
            [
                "Unit test with a fake `Probe`: events arrive in order, UI loop never blocks",
                "Cancel mid-run leaves the terminal usable",
            ],
            "PRD v0.1 H1, H3, §7 criterion 3 · CLAUDE.md (layers) · ADR-008 · ADR-011",
            depends="probe seam, H3, H4",
        ),
    ),
    (
        M1,
        ["host"],
        "host: report model — suites, PASS/FAIL/SKIP, durations (H6)",
        body(
            "Pure model of the run, testable without terminal or board.",
            [
                "Suites `port`, `rom`, `efuse`, `flash`; tests with status, reason and duration; summary",
                "Table-vs-read divergence is a `finding` annotation on the test, not a fourth state (ADR-010)",
                "Interrupted run: the in-flight test is FAIL; tests that depended on the session are SKIP with `not run: <suite/test> failed`; no test is omitted",
                "Durations arrive as `duration_ms` measured by the runner; the `report` does no I/O, not even reading the clock (the `Clock` trait lives in the runner, #20)",
                "Mirrors `docs/context/spec/report-json.md` (#44) without serde: a live `Run` (every planned test present from the start) and a final `Report`; board text enters as sanitized `SafeText`",
                "No ratatui/crossterm dependency in `report`",
                "Text from the board is sanitized before it reaches the model (no escape sequences)",
            ],
            ["`cargo test` covers summary counts, SKIP reasons and sanitization"],
            "PRD v0.1 H6, §7 criterion 5 · CLAUDE.md (board is untrusted input) · ADR-010",
            depends="ADR-010",
        ),
    ),
    (
        M1,
        ["host"],
        "host: Jest-style report view in the TUI (H6)",
        body(
            "Render the report like Jest: suite headers, ✓/✕/○ per test, summary line.",
            [
                "Colors per status, reason under FAIL/SKIP, duration per test",
                "`Tests: N passed, M failed, K skipped, T total` summary",
            ],
            ["Snapshot tests of the rendered buffer (ratatui `TestBackend`)"],
            "PRD v0.1 H6",
            depends="report model, runner",
        ),
    ),
    (
        M1,
        ["host"],
        "host: Board panel (H7)",
        body(
            "Board datasheet view: capability table merged with what was read.",
            ["Chip, revision, cores, clock, flash, PSRAM, radios, MAC, ports"],
            [
                "Panel shown after a run; values read from the board win over the table",
                "Snapshot test with a fake probe result",
            ],
            "PRD v0.1 H7 · ADR-004",
            depends="report model, chips table",
        ),
    ),
    (
        M1,
        ["host", "spec", "test"],
        "host: --no-tui, --json and exit code (H8)",
        body(
            "Same report for scripts and CI.",
            [
                "`devkit-tester --no-tui [--port X]`: plain text, no colors when not a TTY",
                "Plain-text mode: ASCII markers (PASS/FAIL/SKIP) when stdout is not a UTF-8 TTY",
                "`--json` follows `docs/context/spec/report-json.md` (`schema_version`); JSON only on stdout, diagnostics on stderr",
                "Exit codes: 0 = no FAIL (SKIP is not a failure), 1 = at least one FAIL, 2 = no run (no board, ambiguous selection, usage error, unsupported environment such as WSL1, cancelled by the user)",
                "Without `--port` and several boards: list them (chip, MAC, ports) on stderr and exit 2; never guess",
                "`--port` accepts a port name or a USB serial number at run time; never stored",
            ],
            [
                "`--no-tui --json` output parses as JSON and matches the schema (criterion 5)",
                "Exit codes covered by tests",
            ],
            "PRD v0.1 H8, §7 criterion 5 · ADR-010",
            depends="report model, runner, ADR-010, spec report-json.md",
        ),
    ),
    (
        M1,
        ["host", "test"],
        "host: errors never panic — disconnect, busy port, timeout, garbage (H9)",
        body(
            "The board is untrusted input; failures become readable FAILs.",
            [
                "Board unplugged mid-run, port in use, ROM timeout and garbage bytes → FAIL/error with message",
                "Real unplug vs. reset re-enumeration: a port that returns within N s with the same USB serial number is the same port (Windows renumbers COMs; ADR-007)",
                "TUI keeps responding during and after the failure",
                'WSL1 detected → clear "not supported" message (ADR-007)',
            ],
            [
                "Every espflash error kind maps to a readable FAIL (unit test over the mapping); fake probe covers unplug, busy port and timeout",
                "Unplug during a run on a real board gives FAIL and a responsive TUI (criterion 3)",
            ],
            "PRD v0.1 H9, §7 criterion 3 · ADR-007",
            depends="probe seam, runner, platform module",
        ),
    ),
    bring_up(
        "esp32",
        "ESP32",
        extra_done=["`usb-serial-jtag` shows as SKIP with reason (criterion 2)"],
    ),
    bring_up("esp32c3", "ESP32-C3"),
    bring_up(
        "esp32c5",
        "ESP32-C5",
        extra_scope=["Board on its way: start when it arrives"],
        extra_done=["Target added to the firmware build matrix (`fw` hello + CI)"],
    ),
    bring_up(
        "esp32c6",
        "ESP32-C6",
        extra_done=["Native USB + bridge show as one board, one test per port (criterion 2)"],
    ),
    bring_up("esp32s3", "ESP32-S3"),
    bring_up(
        "esp32p4",
        "ESP32-P4",
        extra_scope=["Board on its way: start when it arrives; `sdkconfig.defaults.esp32p4` matches its silicon revision"],
        extra_done=["Target added to the firmware build matrix (`fw` hello + CI)"],
    ),
    (
        M1,
        ["docs", "board"],
        "docs: record the bench results observed on each board",
        body(
            "Write down what the tester actually saw on every bench board, so the v0.1 evidence and the traps live in the repo, not in chat screenshots.",
            [
                "New section per board in `docs/context/hardware.md` (\"Resultados da bancada\"): date, host commit, port combinations tried (bridge only, native only, both, one board alone, two boards on a hub with one unplugged mid-run), the report (passed/failed/skipped) and the values read",
                "ESP32 DevKit V1 (CH9102 `1A86:55D4`): ESP32-D0WD-V3 v3.1, package 1, JEDEC `0x5E4016` 4 MB, 10 passed + `usb-serial-jtag` SKIP (no USB-Serial/JTAG on the chip); connect ~6.4 s (ROM has no `GET_SECURITY_INFO`, ~3 s timeout inside espflash; auto-reset needs several tries); identification can time out right after plugging, fixed by one retry",
                "ESP32-C3 (CH340K `1A86:7522` + native): v0.4, 4 MB embedded XMC `0x464016`, 11/11; CH340K reports a fake USB serial (`\"6\"`), never used; the esp32-logcat firmware resumed after the run",
                "ESP32-C6 (CH343 `1A86:55D3` + native): v0.2, `0x684018` 16 MB, 11/11; the native port re-enumerates < 2 s after any hard reset (even through the bridge); unplugging another board from the same hub made the native port briefly unopenable (fixed by retrying by serial number); the app image header says 2 MB on a 16 MB flash",
                "ESP32-S3 (CH343 + USB): v0.2, PSRAM 8 MB AP_3v3 in package, `0x684018` 16 MB, 10 passed + `usb-serial-jtag` SKIP: the app uses USB OTG (TinyUSB, `303A:4001`, serial `123456`), so the GPIO19/20 PHY is not on USB-Serial/JTAG and the port is listed as \"other\", never probed",
                "Each finding links to the ADR or code that handles it (ADR-008/009, `runner/group.rs`, `transport::PortClass::EspressifApp`)",
            ],
            [
                "`hardware.md` has a results section for ESP32, C3, C6 and S3 (C5/P4 added when they arrive)",
                "Inventory rows updated with exact board models where known",
                "Bring-up issues #26, #27, #29, #30 (part 2) link to their section",
            ],
            "PRD v0.1 §7 criteria 1–4 · hardware.md · ADR-008 · ADR-009",
            depends="runner thread and start screen (#19/#20)",
        ),
    ),
    (
        M1,
        ["docs"],
        "docs: user manual with real screenshots (pt-BR + en)",
        body(
            "A manual for people who use the tester, built the same way as esp32-logcat's: every terminal image is a real bench session redrawn by the tool's own code.",
            [
                "`docs/project/MANUAL.md` (pt-BR) and `MANUAL.en.md`: start here, first test from scratch, reading each screen, what happens in each situation, script/CI mode (`--no-tui`, `--json`, `--port`, exit codes), the board's ports, guarantees (the board leaves as it came), troubleshooting, quick reference, how the images were made",
                "`host/examples/record_session.rs` records a real read-only run (ports, identifications, spec report) into `docs/project/manual/sessions/<name>.json`; port names live only there, never in the spec JSON",
                "`host/examples/manual_screenshots.rs` redraws each session with the same TUI/text code into SVG under `docs/project/manual/img/`; deterministic",
                "`report::json::parse` (inverse of `to_json_line`) and `Deserialize` on port types, tested against the spec examples",
                "Sessions: C6 both ports, C3 (logcat firmware), ESP32 DevKit V1, S3 with USB OTG demo, busy port, unplug mid-run, two boards, JSON",
                "Links from README and docs/README.md",
            ],
            [
                "Both manuals render on GitHub with every image present (a test checks that every image the manual cites exists)",
                "Running `manual_screenshots` twice changes no file",
                "Owner reviewed the rendered manual",
            ],
            "PRD v0.1 H1–H9 · report-json.md · hardware.md · esp32-logcat docs/project/MANUAL.md",
            depends="--no-tui/--json (#24), bench results (#59)",
        ),
    ),
    (
        M1,
        ["board", "test", "docs"],
        "test: remaining bench sessions for the manual and the native-port wedge",
        body(
            "Record the bench sessions the manual still shows only as text, and pin down the C6 native-port wedge seen while recording.",
            [
                "Record with `host/examples/record_session.rs`, then regenerate with `manual_screenshots`: `busy` (C6 with esp32-logcat open on the native port), `unplug` (cable pulled mid-run, criterion 3), `esp32` (DevKit V1, USB-Serial/JTAG SKIP), `s3_otg` (S3 demo firmware on USB OTG, `303A:4001` under Other ports), `json` (C3 by `--port <MAC>`)",
                "Replace the text-only examples in `MANUAL.md`/`MANUAL.en.md` §3 with the generated images",
                "Native-port wedge (C6 on a hub, 2026-11-27): after unplugging another board from the same hub, the C6 USB-Serial/JTAG answered `ERROR_GEN_FAILURE` (\"device not functioning\") and then timed out on every sync, even probed alone, until both cables were unplugged and replugged; afterwards 5/5 runs passed 11/11",
                "Host already retries a non-primary port that fails to sync (3 attempts, 500 ms settle after re-enumeration); confirm on the bench that it covers the intermittent case and that a wedged port still fails with a readable reason",
                "Find out whether the wedge needs the hub event or can be triggered by our own reset/reopen timing; record in `hardware.md`",
            ],
            [
                "All eight sessions recorded and their images in the manual",
                "Wedge reproduced or ruled out, with the finding in `hardware.md`",
            ],
            "PRD v0.1 §7 criteria 2–3 · hardware.md · #64",
            depends="user manual (#64)",
        ),
    ),
    (
        M1,
        ["board", "test"],
        "test: verify no byte is written to flash",
        body(
            "Acceptance criterion 4, checked by hand on one board.",
            [
                "Read the full flash with esptool before and after a `devkit-tester` run; compare hashes",
                "Use a board whose app does not write flash (blank flash or plain hello); record esptool version and the exact commands",
            ],
            ["Hashes identical; procedure and result recorded in hardware.md"],
            "PRD v0.1 §7 criterion 4 · ADR-003",
            depends="H4, runner",
        ),
    ),
    (
        M1,
        ["release"],
        "release: validate v0.1 acceptance criteria and ship v0.1.0",
        body(
            "Close M1 with evidence for every criterion.",
            [
                "CHANGELOG, README with TUI GIF and supported-chip table, tag `v0.1.0`",
                "GitHub Release notes from CHANGELOG",
                "Social preview replaced by a real `Tests: … passed` capture",
                "ROADMAP M1 checked; PRD v0.1 marked delivered; PRD v0.2 promoted to current",
                "Announce on esp32.com and r/esp32",
            ],
            ["Criteria 1–5 checked with links to evidence"],
            "PRD v0.1 §7 · CONVENTIONS §2 · GITHUB.md",
        ),
    ),
]


def gh(*args: str) -> str:
    r = subprocess.run([GH, *args], capture_output=True, text=True, encoding="utf-8")
    if r.returncode:
        raise SystemExit(f"gh {' '.join(args[:3])} failed: {r.stderr}")
    return r.stdout


def gh_json(*args: str) -> list[dict]:
    return json.loads(gh(*args))


def sync_labels() -> None:
    existing = {
        lbl["name"]
        for lbl in gh_json("label", "list", "-R", REPO, "--json", "name", "--limit", "200")
    }
    for name, desc, color in LABELS:
        if name not in existing:
            print(f"label     + {name}")
            if APPLY:
                gh("label", "create", name, "-R", REPO, "-d", desc, "-c", color)


def sync_milestones() -> None:
    existing = {m["title"] for m in gh_json("api", f"repos/{REPO}/milestones?state=all&per_page=100")}
    for title, desc, state in MILESTONES:
        if title not in existing:
            print(f"milestone + {title} [{state}]")
            if APPLY:
                gh(
                    "api",
                    f"repos/{REPO}/milestones",
                    "-f",
                    f"title={title}",
                    "-f",
                    f"description={desc}",
                    "-f",
                    f"state={state}",
                )


def sync_issues() -> None:
    existing = {
        i["title"]: (i["number"], {lbl["name"] for lbl in i["labels"]})
        for i in gh_json(
            "issue",
            "list",
            "-R",
            REPO,
            "--state",
            "all",
            "--limit",
            "500",
            "--json",
            "number,title,labels",
        )
    }
    for ms, labels, title, text in ISSUES:
        if title in existing:
            number, current = existing[title]
            missing = [lbl for lbl in labels if lbl not in current]
            if missing:
                print(f"issue #{number:<3} labels + {', '.join(missing)}")
                if APPLY:
                    gh("issue", "edit", str(number), "-R", REPO, "--add-label", ",".join(missing))
            continue
        print(f"issue     + [{ms.split(' ')[0]}] {title}  ({', '.join(labels)})")
        if APPLY:
            args = ["issue", "create", "-R", REPO, "-t", title, "-b", text, "-m", ms]
            for lbl in labels:
                args += ["-l", lbl]
            gh(*args)


def main() -> None:
    # Titles carry "·" and "—"; a redirected stdout on Windows defaults to cp1252.
    sys.stdout.reconfigure(encoding="utf-8")
    titles = [t for _, _, t, _ in ISSUES]
    dupes = {t for t in titles if titles.count(t) > 1}
    if dupes:
        raise SystemExit(f"duplicate issue titles break idempotency: {sorted(dupes)}")
    known = {name for name, _, _ in LABELS}
    unknown = {lbl for _, labels, _, _ in ISSUES for lbl in labels} - known
    if unknown:
        raise SystemExit(f"issues use labels not defined in LABELS: {sorted(unknown)}")
    sync_labels()
    sync_milestones()
    sync_issues()
    if not APPLY:
        print("\n(dry run — pass --apply to make these changes)")


if __name__ == "__main__":
    main()
