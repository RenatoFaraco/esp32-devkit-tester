# ADR-002: Host em Rust com ratatui e espflash como biblioteca
- Status: aceita
- Data: 2026-11-26

## Contexto
O host é uma TUI que enumera portas, fala o protocolo do ROM bootloader da Espressif, lê
eFuses, grava firmware (v0.2) e mostra um relatório estilo Jest. Precisa rodar bem no Windows
e virar binário único para quem não tem Python nem ESP-IDF (v1.0).

## Decisão
- **Rust** estável, edition 2024; crate `esp32-devkit-tester`, binário `devkit-tester`.
- **espflash** (esp-rs) como biblioteca para conexão, detecção de chip, eFuse, flash ID e
  gravação. A versão 4.x traz stubs e bootloaders para ESP32, C2, C3, C5, C6, C61, H2, P4, S2 e
  S3 — cobre a bancada inteira. Usar a API de biblioteca, nunca o módulo `cli` (sem SemVer).
- **ratatui** + **crossterm** para a TUI (crossterm funciona no console do Windows).
- `serialport` para enumeração (VID:PID, número de série, descrição USB).
- Erros com `thiserror` na biblioteca e `anyhow` só no `main`.

## Opções descartadas
- **C no host** (libserialport + termbox2 + esp-serial-flasher): o protocolo do ROM teria de ser
  reimplementado em boa parte, e o port do esp-serial-flasher para Windows não é oficial.
- **Python** (esptool + Textual, como o esp32-logcat): rápido de fazer, mas exige Python no PC
  de quem só quer testar a placa; o binário único é requisito da v1.0.
- **Chamar o `espflash`/`esptool` como processo e fazer parse do texto:** frágil a mudanças de
  saída entre versões.

## Consequências
- A Espressif considera o espflash completo em funcionalidades (manutenção, sem evolução
  planejada): chip novo pode demorar a ser suportado. Fixar versão no `Cargo.lock`, isolar o
  uso dele no módulo `probe` para poder trocar.
- Portfólio: o projeto exercita Rust no host além do firmware em C.
