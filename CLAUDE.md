# esp32-devkit-tester — contexto para o Claude

Testador de placas de desenvolvimento ESP32: você liga a placa recém-comprada, aperta uma tecla
e uma TUI no PC diz, no estilo do Jest, o que passou e o que falhou (UART, JTAG, registradores,
memória) e mostra as características da placa. Ferramenta irmã do
[esp32-logcat](https://github.com/RenatoFaraco/esp32-logcat), com o mesmo jeito de trabalhar.

Antes de qualquer tarefa de implementação, leia o PRD da versão corrente (ver `docs/README.md`;
hoje `docs/context/prd/v0.1-identify.md`) e os ADRs que ele cita.

## Sobre o dono
Renato: engenheiro eletricista, 8 anos de firmware C/C++ no ESP32 (ESP-IDF/FreeRTOS), Rust em
software de PC e reaprendendo Rust embarcado. Atue como mentor: explique o porquê das decisões.
Em embarcados, **parta do hardware** (registrador como flip-flops, barramento, mapa de memória,
pinos de strapping, ROM) e só depois mostre o código; não explique a partir do C.
Português nas explicações e em `docs/`; código, nomes, commits e issues em inglês.

## Ambiente
- **Plataformas (ADR-007):** fase 1 = Windows nativo com **PowerShell** (referência até a v1.0);
  fase 2 = WSL2 (via `usbipd-win`); fase 3 = Linux nativo e macOS. O código não pode fechar a porta
  para as fases 2 e 3: nada só-Windows fora do módulo `platform` (`#[cfg]`), portas por VID:PID,
  caminhos com `std::path`. CI do host em Windows **e** Linux desde o M0.
- Comandos para o usuário (README, docs) em PowerShell. No WSL2 o reset do USB-Serial/JTAG
  derruba o repasse USB: não usar o WSL para gravar/testar na fase 1.
- Firmware: ESP-IDF via `idf.py` (ADR-001). Versão e comandos exatos são fixados no M0 e
  anotados aqui e no `test-runner`.
- Host: Rust estável (edition 2024), `cargo`; workspace em `host/`.
- Placas (inventário e armadilhas em `docs/context/hardware.md`): ESP32, C3, C5, C6, S3, P4;
  H2 a caminho. O número da COM muda de máquina para máquina: **nunca fixar porta** em código
  ou config versionada; o host identifica portas por VID:PID.

## Estrutura (alvo; o M0 cria `firmware/` e `host/`)
```
esp32-devkit-tester/
├── CLAUDE.md
├── README.md                 inglês, pensado para busca (SEO)
├── CHANGELOG.md              Keep a Changelog; uma versão ↔ um PRD
├── .claude/
│   ├── agents/               subagentes (ver "Subagentes")
│   └── settings.json         permissões dos comandos de teste
├── docs/                     índice em docs/README.md
│   ├── project/              como trabalhamos: CONVENTIONS, ROADMAP, GITHUB (metadados/SEO)
│   └── context/              o que o sistema é e por quê
│       ├── prd/              um PRD por versão (v0.1 corrente; os demais em rascunho)
│       ├── decisions/        ADRs curtos (um por arquivo)
│       ├── spec/             contratos no fio (protocolo de resultados, a partir da v0.2)
│       └── hardware.md       placas reais: chip, portas, VID:PID, armadilhas
├── firmware/                 projeto ESP-IDF em C (v0.2+): um build por chip
│   ├── main/
│   ├── components/dkt_*/     testes on-target (Unity) e o emissor de resultados
│   └── sdkconfig.defaults[.<target>]
├── host/                     workspace Cargo: crate `esp32-devkit-tester`, binário `devkit-tester`
│   └── src/                  transport → probe → runner → report → ui (ratatui)
└── scripts/                  github_bootstrap.py (labels, milestones, issues)
```

## Regras de arquitetura
- **Duas metades, um contrato.** O host (Rust) roda no PC; o firmware (C/ESP-IDF) roda na placa.
  Tudo o que cruza o cabo está em `docs/context/spec/` — mudou o formato, mudam a spec, os dois
  lados e os testes no mesmo commit.
- **Não destrutivo por padrão.** A v0.1 só fala com o ROM bootloader e não grava nada (ADR-003).
  Gravar firmware de teste (v0.2+) exige confirmação na TUI; testes de flash só escrevem numa
  partição `scratch`.
- **Capacidades por chip são dado, não `if`.** Host: uma tabela única (ADR-004). Firmware:
  macros `SOC_*` de `soc/soc_caps.h`, nunca `CONFIG_IDF_TARGET_*` espalhado.
- **Teste que não se aplica é SKIP, não FAIL** (ex.: JTAG embutido no ESP32 original, Wi-Fi no H2).
- **A placa é entrada não confiável.** Bytes da serial nunca derrubam o host, nunca viram
  markup nem sequência de escape no terminal.
- **Host em camadas:** `platform` (diferenças de SO, `#[cfg]`) e `transport` (bytes/portas) → `probe` (ROM via espflash, JTAG) → `runner`
  (executa suítes) → `report` (modelo puro) → `ui` (ratatui). Só `ui` importa ratatui/crossterm;
  `report` é testável sem terminal e sem placa.
- Firmware: C17, `-Wall -Wextra -Werror`, sem VLA; nada de `ESP_LOGx`/malloc em ISR.

## Fluxo de trabalho
1. Plano antes de editar (modo de planejamento) para qualquer coisa além de ajuste trivial.
2. Uma tarefa por vez, ligada a uma issue e a um marco do ROADMAP (M0…M5 ↔ versões).
3. Teste primeiro quando der. Host: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`.
   Firmware: build por alvo e testes Unity na placa (comandos fixados no M0).
4. Commit pequeno por passo que funciona, Conventional Commits (`feat(host):`, `fix(fw):`…).
5. Release: atualizar `CHANGELOG.md`, marcar o PRD da versão como entregue e promover o próximo
   de rascunho para corrente.

## Subagentes (`.claude/agents/`)
| Agente | Quando delegar | Pode |
|--------|----------------|------|
| `architect` | Antes de começar uma versão ou uma mudança que cruza host ↔ firmware, ou quando surgir uma decisão não óbvia | Ler tudo; escrever **só** em `docs/context/` (ADR/PRD/spec) |
| `firmware-engineer` | Implementar ou revisar C/ESP-IDF em `firmware/` | Editar `firmware/`; build/flash |
| `host-specialist` | Implementar ou revisar Rust em `host/` (probe, runner, TUI) | Editar `host/`; cargo |
| `test-runner` | Depois de mudar `firmware/` ou `host/`; antes de commitar | Rodar testes e builds; devolve só falhas |
| `milestone-checker` | Antes de fechar marco ou abrir PR de release | Só ler |

Os engenheiros não editam fora da sua pasta nem mudam a spec sozinhos: mudança de contrato passa
pelo `architect`. A conversa principal decide, integra e commita. Mudou uma regra do projeto
(CLAUDE.md, CONVENTIONS, PRD)? Revise os prompts dos agentes no mesmo PR.
