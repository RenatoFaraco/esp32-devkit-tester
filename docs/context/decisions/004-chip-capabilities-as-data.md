# ADR-004: Capacidades por chip como dado, testes não aplicáveis como SKIP
- Status: aceita
- Data: 2026-11-26

## Contexto
As famílias diferem em núcleo (Xtensa LX6/LX7, RISC-V), número de núcleos, rádios (Wi-Fi 4/6,
2,4/5 GHz, BLE, 802.15.4, nenhum no P4), USB-Serial/JTAG (ausente no ESP32 original), PSRAM e
registradores de identificação. Espalhar `if chip == ...` pelo host e `#if
CONFIG_IDF_TARGET_...` pelo firmware torna cada chip novo uma caça ao tesouro.

## Decisão
- **Host:** uma tabela única de capacidades por chip (módulo `chips` no host), com fonte citada
  (datasheet/TRM) em cada linha; o runner decide PASS/FAIL/SKIP a partir dela. O que é lido da
  placa (eFuse) prevalece sobre o que a tabela supõe, e divergência vira achado no relatório.
- **Firmware:** diferenças via `soc/soc_caps.h` (`SOC_WIFI_SUPPORTED`, `SOC_BT_SUPPORTED`,
  `SOC_IEEE802154_SUPPORTED`, `SOC_USB_SERIAL_JTAG_SUPPORTED`…) e Kconfig.
- Teste que não se aplica ao chip é **SKIP com motivo**, nunca FAIL e nunca omitido.

## Consequências
- Adicionar o H2 (ou outro chip) = uma linha na tabela + build do firmware para o alvo.
- A ficha da placa (painel "Board") sai da mesma tabela.
