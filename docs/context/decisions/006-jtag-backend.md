# ADR-006: Backend de JTAG
- Status: proposta (fechar no início da v0.3)
- Data: 2026-11-26

## Contexto
A v0.3 precisa ler o IDCODE do TAP, parar/retomar núcleos e ler memória pelo JTAG, no USB-Serial/
JTAG embutido (C3, C5, C6, S3, P4, H2) e em sonda FTDI externa (ESP32 original).

## Opções
1. **OpenOCD-esp32 como processo**, controlado pela porta TCL: suporta toda a família (é o que
   o ESP-IDF usa), mas é um binário externo para distribuir/instalar.
2. **probe-rs como biblioteca Rust:** integra no binário único; cobertura de Xtensa e dos chips
   novos (C5, P4) precisa ser conferida.
3. Híbrido: probe-rs onde suportar, OpenOCD como fallback.

## Critério de decisão
Cobertura real das 6 placas da bancada, medida no início da v0.3, e impacto na distribuição.
