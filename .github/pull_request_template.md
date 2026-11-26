## Summary

<!-- What changes and why. Keep the "why"; the diff already shows the "how". -->

Closes #

**Milestone:** M?

## Type

- [ ] feat
- [ ] fix
- [ ] test
- [ ] refactor / perf
- [ ] docs
- [ ] build / ci / chore

## How it was tested

- [ ] Host: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`
- [ ] Firmware: `idf.py set-target <target> build` for the affected targets
- [ ] On a board — chip, board model, port (native / bridge) and what was observed:

<!-- Output, captures or fixtures that support the change. -->

## Checklist

- [ ] Commits follow Conventional Commits; each one builds and passes tests
- [ ] Layers respected: only `ui` imports ratatui/crossterm; OS `cfg` only in `platform`
- [ ] Bytes from the board are untrusted: no panic, no raw escape sequences in the terminal
- [ ] Non-destructive: nothing written outside the `scratch` partition; eFuses never written
- [ ] Chip differences are data (`chips` table / `SOC_*`), not `if chip ==`; not applicable → SKIP with reason
- [ ] No fixed port names (`COMx`, `ttyACMx`) in code or versioned config
- [ ] Contract changed → spec in `docs/context/spec/`, host and firmware updated in this PR
- [ ] New dependency justified in this description
- [ ] Docs updated: ROADMAP items, ADR for non-obvious decisions, README if user-facing, agent prompts if a project rule changed
