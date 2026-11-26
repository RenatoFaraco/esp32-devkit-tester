# Security Policy

## Supported versions

esp32-devkit-tester is pre-1.0 and maintained by one person. Only the latest code receives fixes:

| Version | Supported |
| ------- | --------- |
| `main` | ✅ |
| latest `0.x` release | ✅ |
| older `0.x` releases | ❌ — update to the latest release |

## Reporting a vulnerability

**Please do not open a public issue for security problems.**

Report privately through GitHub: **Security → Report a vulnerability** on this repository
([direct link](https://github.com/RenatoFaraco/esp32-devkit-tester/security/advisories/new)).
Include the affected component (test firmware, host tool), the version or
commit, steps to reproduce and, if possible, a log or proof of concept.

What to expect:

- **Acknowledgement within 7 days.** This is a personal project; there is no fixed deadline for
  a fix, but you will get updates on the advisory as the work progresses.
- If the report is accepted, the fix is developed in a private fork when needed, released, and
  the advisory is published crediting you (unless you prefer otherwise).
- If it is declined (for example, it falls under a known limitation below), you will get an
  explanation.

## Scope and known limitations

These are documented design decisions, not vulnerabilities:

- **The tester talks to whatever is plugged in.** It resets the board, reads eFuses and (from
  v0.2) writes a test firmware. Run it only on boards you own and whose flash you can lose.
- **No authentication on the serial or JTAG link.** Anyone with USB access to the board can do
  what the tester does.

In scope, among others: crashes or hangs of the host tool caused by bytes coming from a board
(the board is untrusted input), terminal escape sequences from a board reaching your terminal,
and the test firmware writing outside its scratch partition.
