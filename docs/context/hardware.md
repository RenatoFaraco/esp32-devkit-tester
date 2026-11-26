# Hardware — placas da bancada

> Preenchido no bring-up de cada placa (issues com label `board`). O que estiver marcado
> "datasheet" ainda não foi conferido na placa. A porta COM de cada placa **não** entra aqui:
> muda de PC para PC.

## Inventário

| Chip | Placa (modelo exato) | Núcleo | Rádios (datasheet) | USB-Serial/JTAG | Portas na placa | Flash / PSRAM | Revisão | Conferido em |
|------|----------------------|--------|--------------------|-----------------|-----------------|---------------|---------|--------------|
| ESP32 | ESP32 DevKit V1, chip ESP32-D0WD-V3 (pacote 1) | Xtensa LX6 ×2 | Wi-Fi 4, BT clássico + BLE | não (JTAG só com sonda externa, GPIO12–15) | 1 UART via CH9102 (`1A86:55D4`) | 4 MB externa (JEDEC `5E` `4016`), sem PSRAM (D0WD) | v3.1 | 2026-11-26 (parte 1) |
| ESP32-C3 | DevKit ESP32-C3 (modelo exato a confirmar), chip ESP32-C3 QFN32 com flash embutida (eFuse: 4 MB, XMC, 105 °C → provável ESP32-C3FH4) | RISC-V ×1 | Wi-Fi 4, BLE 5 | sim | UART via CH340K (`1A86:7522`); USB nativo na placa, não conectado na parte 1 | 4 MB embutida (JEDEC `46` `4016`), sem PSRAM | v0.4 | 2026-11-26 (parte 1) |
| ESP32-C5 | a caminho | RISC-V ×1 | Wi-Fi 6 2,4/5 GHz, BLE 5, 802.15.4 | sim | a preencher | a preencher | — | — |
| ESP32-C6 | DevKit ESP32-C6 (modelo exato a confirmar), chip ESP32-C6 QFN40 sem flash embutida | RISC-V HP + LP | Wi-Fi 6 2,4 GHz, BLE 5, 802.15.4 | sim | USB nativo (`303A:1001`); UART da placa não conectada na parte 1 | 16 MB externa (JEDEC `68` `4018`), sem PSRAM | v0.2 | 2026-11-26 (parte 1) |
| ESP32-S3 | DevKit ESP32-S3 (modelo exato a confirmar), chip ESP32-S3 QFN56 com PSRAM embutida (eFuse: 8 MB, AP 3,3 V, 85 °C → provável ESP32-S3R8) | Xtensa LX7 ×2 | Wi-Fi 4, BLE 5 | sim (+ USB OTG) | UART via CH343 (`1A86:55D3`); USB nativo na placa, não conectado na parte 1 | 16 MB externa quad 3,3 V (JEDEC `68` `4018`), PSRAM 8 MB embutida | v0.2 | 2026-11-26 (parte 1) |
| ESP32-P4 | a caminho | RISC-V ×2 (HP) | nenhum no chip (DevKits usam um coprocessador de rádio) | sim (+ USB OTG HS) | a preencher | a preencher | — | — |
| ESP32-H2 | não comprada | RISC-V ×1 | BLE 5, 802.15.4 (sem Wi-Fi) | sim | a preencher | a preencher | — | — |

Referências de cada placa (chip, eFuses, flash): `host/tests/fixtures/<chip>.json`, com a versão
do esptool e os comandos exatos. Fontes: [ESP32 Datasheet](https://www.espressif.com/sites/default/files/documentation/esp32_datasheet_en.pdf),
[ESP32-C3 Datasheet](https://www.espressif.com/sites/default/files/documentation/esp32-c3_datasheet_en.pdf),
[ESP32-C6 Datasheet](https://www.espressif.com/sites/default/files/documentation/esp32-c6_datasheet_en.pdf),
[ESP32-S3 Datasheet](https://www.espressif.com/sites/default/files/documentation/esp32-s3_datasheet_en.pdf)
(variantes e nomenclatura de pacote/flash).

Tudo nas colunas "datasheet" deve ser confirmado contra o datasheet/TRM de cada chip ao
preencher a linha (citar a fonte na issue de bring-up).

## Identificação das portas (VID:PID)

| VID:PID | O que é | Observação |
|---------|---------|------------|
| `303A:1001` | USB-Serial/JTAG nativo da Espressif | serial + JTAG na mesma porta; reset faz a porta sumir e voltar. No Windows a COM se chama "Dispositivo Serial USB" (driver `usbser`), interface `MI_00`; o JTAG é `MI_02` (WinUSB). O número de série (= MAC base, `AA:BB:CC:DD:EE:FF`, maiúsculas com `:`) está no **dispositivo composto pai**, não na COM — conferido na C6 |
| `10C4:EA60` | Silicon Labs CP210x | ponte USB-UART comum em DevKitC |
| `1A86:7523` | WCH CH340 | |
| `1A86:7522` | WCH CH340K | placa C3 da bancada (driver `CH341SER`); **sem número de série USB**: o ID do Windows é a posição no hub |
| `1A86:55D3` / `1A86:55D4` | WCH CH343 / CH9102 | `55D3` = CH343 na placa S3, `55D4` = CH9102 na placa ESP32 (as duas com driver `CH343SER`); **as duas têm número de série USB**, que identifica a ponte, não o chip (ADR-008) |
| `0403:6010` | FTDI FT2232 | ESP-Prog: um canal JTAG + um canal UART |

Conferir cada linha com o Gerenciador de Dispositivos ao fazer o bring-up.

## Armadilhas conhecidas (herdadas do esp32-logcat)
- ESP32 original: com os defaults do IDF v5.5.2 o bootloader ocupa 0x66c0 de 0x7000 (só 8%
  livre). Opção de bootloader no `sdkconfig.defaults` (log verbose, secure boot) pode estourar.
- Nas pontes, `RTS` costuma ir para EN (reset) e `DTR` para o pino de boot (GPIO0/GPIO9):
  abrir a porta com os dois soltos para não segurar a placa em reset.
- Driver `usbser.sys` (porta nativa no Windows): depois de mexer no `RTS`, reescrever o `DTR`.
- O esptool v4.12 mostra "Unknown Embedded Flash" na C6 sem flash embutida (eFuse
  `FLASH_CAP = 0`): não confiar na string de features; ler o campo de eFuse.
- C6 tem MAC de 8 bytes (EUI-64, por causa do 802.15.4) além do MAC base de 6 bytes; a
  identidade da placa (ADR-008) é o MAC base, que é o que aparece no número de série USB.
- 12 V num pino de 3,3 V queima o chip: ponte USB-UART externa
  sempre em 3,3 V, só TX/RX/GND cruzados.
