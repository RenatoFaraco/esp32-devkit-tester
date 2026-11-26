# GitHub — metadados e descoberta (SEO)

Fonte única para o `scripts/github_bootstrap.py` e para a configuração do repositório.

## Nome: `esp32-devkit-tester`
- `esp32` primeiro: é o termo que todo mundo busca, e o nome bate com o `esp32-logcat`.
- `devkit`: é como a Espressif nomeia as placas (ESP32-C6-DevKitC-1, ESP32-S3-DevKitC-1…) e
  como elas aparecem nas lojas.
- `tester`: diz a intenção. Alternativa considerada: `esp32-board-tester` ("board" é mais
  genérico; "devkit" é mais específico e menos disputado).

## Descrição (campo "About", ≤ 350 caracteres)
> Test any ESP32 dev board in seconds: a terminal UI that detects the chip, checks UART and JTAG,
> reads eFuses, tests RAM, PSRAM and flash, and reports Jest-style. ESP32, C3, C5, C6, S3, P4, H2.

## Tópicos (máximo 20)
`esp32` `esp32-c3` `esp32-c5` `esp32-c6` `esp32-s3` `esp32-p4` `esp32-h2` `esp-idf`
`espressif` `hardware-testing` `board-test` `devkit` `jtag` `bring-up` `tui` `ratatui` `rust`
`embedded` `firmware` `factory-test`

## Outras alavancas
- **README em inglês**, com as palavras-chave no título, na primeira frase e nos cabeçalhos
  (nomes completos dos chips: "ESP32-C6", não só "C6"); tabela de chips suportados.
- **Social preview** (Settings → Social preview, 1280×640): captura da TUI com o resumo
  `Tests: … passed`. Imagem de capa no README a partir da v0.1.
- **GIF da TUI** no topo do README a partir da v0.1 (é o que faz a pessoa ficar).
- **Releases** com notas a partir do `CHANGELOG.md`; o GitHub ranqueia repositórios ativos.
- Link cruzado com o esp32-logcat nos dois READMEs; post no fórum esp32.com e no r/esp32 ao
  lançar a v0.1.
- Publicar no crates.io na v1.0 (mais uma porta de entrada).

## Labels
| Label | Uso | Cor |
|-------|-----|-----|
| `host` | Rust, TUI, probe | `fbca04` |
| `fw` | Firmware C/ESP-IDF | `0e8a16` |
| `spec` | Contrato host ↔ placa | `1d76db` |
| `ci` | GitHub Actions e tooling | `5319e7` |
| `board` | Bring-up ou validação numa placa específica | `b60205` |
| `chip:<nome>` | Específico de um chip (`chip:esp32p4`…) | `c5def5` |
| `test` | Teste ou critério de aceite | `c2e0c6` |
| `docs` | Documentação | `0075ca` |
| `release` | Preparação de release | `0052cc` |

## Milestones
`M0 · Foundation`, `M1 · Identify (v0.1)`, `M2 · On-target tests (v0.2)`, `M3 · JTAG (v0.3)`,
`M4 · Radios & peripherals (v0.4)`, `M5 · Release (v1.0)` — descrições a partir do ROADMAP.
Issues detalhadas só para M0 e M1; os demais marcos recebem issues quando o PRD virar corrente.
