# Documentação

- **`project/`: como trabalhamos.** Convenções, roadmap, metadados do GitHub.
- **`context/`: o que o sistema é e por quê.** PRDs, ADRs, specs e as placas reais — leitura
  obrigatória para quem (pessoa ou agente) vai mudar o código.

Regra para documento novo: "como faço?" → `project/`; "o que é?" ou "por que é assim?" →
`context/`.

## `project/`

| Documento | Para quê |
|-----------|----------|
| [CONVENTIONS.md](project/CONVENTIONS.md) | Idioma, Git, estilo de C e Rust, testes, modelo de ADR |
| [ROADMAP.md](project/ROADMAP.md) | Marcos M0…M5 (um por versão) e o que falta em cada um |
| [GITHUB.md](project/GITHUB.md) | Nome, descrição, tópicos (SEO), labels e milestones do repositório |

## `context/`

### PRDs — um por versão (a mesma do `CHANGELOG.md`)

| PRD | Marco | Status |
|-----|-------|--------|
| [v0.1 — Identificar](context/prd/v0.1-identify.md) | M1 | **corrente** |
| [v0.2 — Testes na placa](context/prd/v0.2-on-target-tests.md) | M2 | rascunho |
| [v0.3 — JTAG](context/prd/v0.3-jtag.md) | M3 | rascunho |
| [v0.4 — Rádios e periféricos](context/prd/v0.4-radios-peripherals.md) | M4 | rascunho |
| [v1.0 — Release](context/prd/v1.0-release.md) | M5 | rascunho |

O PRD da versão em andamento é o corrente; os seguintes ficam em rascunho, só com o que os ADRs
já decidiram, e viram corrente quando a versão começar. "PRD" sem versão = o corrente.

### Outros

| Documento | Para quê |
|-----------|----------|
| [hardware.md](context/hardware.md) | As placas da bancada: chip, portas, VID:PID, armadilhas |
| [spec/](context/spec/README.md) | Contratos no fio host ↔ placa |

### Decisões (`context/decisions/`)

| ADR | Decisão | Status |
|-----|---------|--------|
| [001](context/decisions/001-c-firmware-with-plain-esp-idf.md) | Firmware em C com ESP-IDF puro (`idf.py`), sem PlatformIO | aceita |
| [002](context/decisions/002-rust-host-ratatui-espflash.md) | Host em Rust com ratatui e espflash como biblioteca | aceita |
| [003](context/decisions/003-non-destructive-by-default.md) | Não destrutivo por padrão; v0.1 só com o ROM | aceita |
| [004](context/decisions/004-chip-capabilities-as-data.md) | Capacidades por chip como dado; SKIP para o que não se aplica | aceita |
| [005](context/decisions/005-result-line-protocol.md) | Resultados da placa em linhas ASCII próprias sobre Unity | proposta |
| [006](context/decisions/006-jtag-backend.md) | Backend de JTAG (OpenOCD vs probe-rs) | proposta |
| [007](context/decisions/007-platform-support-windows-first.md) | Plataformas: Windows/PowerShell primeiro, depois WSL2, depois Linux/macOS | aceita |

ADR novo segue o modelo de [CONVENTIONS.md](project/CONVENTIONS.md) §7 e entra nesta tabela no
mesmo commit.
