# Roadmap — esp32-devkit-tester

> Escopo e critérios → `docs/context/prd/` · Placas → `docs/context/hardware.md`
> 🖥️ = roda sem hardware · 🔌 = precisa de placa · um marco = uma versão = um PRD

Cada marco termina com: CI verde, tag, `CHANGELOG.md` atualizado e este arquivo marcado.

## M0 · Fundação 🖥️ — `v0.0.1`
- [ ] Repositório no GitHub (nome, descrição, tópicos e social preview conforme `GITHUB.md`)
- [ ] `scripts/github_bootstrap.py`: labels, milestones M0…M5 e issues do M1 (idempotente,
      dry run por padrão, `--apply` para criar), como no esp32-logcat
- [ ] Templates de issue e PR (`.github/`), incluindo "board bring-up"
- [ ] `host/`: workspace Cargo, crate `esp32-devkit-tester`, binário `devkit-tester` que abre
      a tela "Press any key" e sai com `q`
- [ ] `firmware/`: projeto ESP-IDF "hello" que compila para os 4 alvos da bancada (ESP32, C3, C6, S3;
      C5 e P4 entram no bring-up do M1 quando as placas chegarem); versão do
      IDF fixada; ADR-001 aceita ou revista
- [ ] Módulo `platform` no host (vazio na fase 1, ponto de extensão para WSL/Linux/macOS, ADR-007)
- [ ] CI: fmt + clippy + test do host (Windows e Linux, ADR-007); build do firmware na matriz de alvos
      (imagem `espressif/idf`)
- [ ] Comandos exatos de build/teste escritos no `CLAUDE.md` e no `test-runner`; instruções de
      ambiente para o usuário em PowerShell (ESP-IDF ativado, `cargo`)
- **Pronto quando:** CI verde num push.

## M1 · Identificar 🖥️🔌 — `v0.1.0` · [PRD](../context/prd/v0.1-identify.md)
- [ ] Enumeração e classificação de portas por VID:PID; agrupamento por placa (H2)
- [ ] Entrar no ROM e sincronizar em cada porta (H3)
- [ ] Tabela de capacidades por chip com fontes (ADR-004)
- [ ] Probe: chip, revisão, MAC, eFuses, flash, PSRAM declarada (H4) e registradores (H5)
- [ ] Modelo do relatório + renderização estilo Jest (H6) e painel "Board" (H7)
- [ ] `--no-tui`, `--json`, código de saída (H8); erros sem panic (H9)
- [ ] Bring-up das 6 placas registrado em `hardware.md`
- **Pronto quando:** critérios 1–5 do PRD v0.1 → **release `v0.1.0`**.

## M2 · Testes na placa 🔌 — `v0.2.0` · [PRD](../context/prd/v0.2-on-target-tests.md)
Detalhar quando o PRD virar corrente (ADR-005 → spec do protocolo primeiro).

## M3 · JTAG 🔌 — `v0.3.0` · [PRD](../context/prd/v0.3-jtag.md)
Detalhar quando o PRD virar corrente (ADR-006 primeiro).

## M4 · Rádios e periféricos 🔌 — `v0.4.0` · [PRD](../context/prd/v0.4-radios-peripherals.md)

## M5 · Release 🖥️ — `v1.0.0` · [PRD](../context/prd/v1.0-release.md)

## Depois da v1.0 — outras plataformas (ADR-007)
- **WSL2:** guia do `usbipd-win` (`bind`, `attach --auto-attach`), reconexão quando a porta muda
  de `ttyACMx`, comportamento no reset do USB-Serial/JTAG, JTAG pelo usbip; PRD próprio quando
  começar.
- **Linux nativo:** regras `udev`, pacote/binário validado.
- **macOS:** nomes `/dev/cu.*`, assinatura do binário.
