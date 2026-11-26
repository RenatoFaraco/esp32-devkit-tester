# ADR-009: O que "só leitura" quer dizer na v0.1 (refina o ADR-003)
- Status: proposta
- Data: 2026-11-26

## Contexto
O ADR-003 diz que a v0.1 "só lê". Mas conversar com o ROM bootloader não é só ler: para obter
o JEDEC ID da flash o host precisa **programar** o controlador SPI, e o espflash, por padrão,
carrega um *stub* na RAM e troca o baud. É preciso separar o que muda estado **persistente**
(flash, eFuse) do que muda estado **volátil** (registradores, SRAM), e dizer como a placa fica
no fim.

Do lado do hardware:
- **ROM** é código em máscara no silício; roda quando o strapping indica download no reset
  (GPIO0 = 0 no ESP32/S3, GPIO9 = 0 nos RISC-V). Fala o protocolo serial da Espressif
  (`SYNC`, `READ_REG`, `WRITE_REG`, `SPI_ATTACH`, `CHANGE_BAUDRATE`, `MEM_*`, `FLASH_*`…).
- **Stub** é um programa que o host escreve na SRAM (comandos `MEM_BEGIN/DATA/END`) e manda
  executar; ele substitui o ROM no resto da sessão. Ganha velocidade e comandos extras para
  gravar/apagar a flash. Some no reset (SRAM é volátil), mas é código nosso rodando na CPU,
  não mais o ROM.
- **Registradores de periférico** são flip-flops: voltam ao valor de reset quando o chip
  reseta. Escrever no controlador SPI1 (`USR`, `USR1`/`MOSI_DLEN`/`MISO_DLEN`, `USR2`, `W0`,
  `CMD`) só configura a próxima transação no barramento SPI; não toca no array da flash.
- **RDID (`0x9F`)** é comando de leitura da própria flash: devolve 3 bytes (fabricante, tipo,
  capacidade). Não liga o *write enable latch*, não muda registrador de status, não escreve.
- O baud é o divisor da UART0 (registrador volátil). Na porta nativa o "baud" nem existe no
  fio (USB full-speed); o ROM ignora.

No espflash 4.7.0 (docs.rs, conferido em 2026-11-26): `Flasher::connect(connection, use_stub,
verify, skip, chip, baud)`; com `use_stub = false` o stub não é carregado; `flash_detect` lê o
ID da flash por `spi_command`, que escreve `USR`/`USR2`/`W0`/`CMD` e restaura só `USR` e
`USR2`; `SPI_ATTACH` roda na conexão com ou sem stub; `change_baud` só roda se `baud` for
`Some` e maior que 115 200.

## Decisão
1. **Só o ROM, sem stub:** `use_stub = false`. Tudo o que a v0.1 precisa (`READ_REG`,
   `SPI_ATTACH`, leitura do ID da flash via SPI) o ROM já faz, e o volume é pequeno (dezenas de
   registradores). Testar o próprio ROM é parte do que a v0.1 promete.
2. **Escritas permitidas — todas voláteis, todas listadas:**
   - registradores do controlador **SPI1** para emitir RDID (via `flash_detect` do espflash);
   - `SPI_ATTACH` (configura os pinos da flash no IO MUX/controlador);
   - `CHANGE_BAUDRATE` (divisor da UART0): **permitido, mas desligado na v0.1** (`baud = None`,
     fica em 115 200). Menos um modo de falha (pontes CH340 em baud alto) por um ganho de
     décimos de segundo.
   - linhas DTR/RTS das sequências de reset (não é escrita em registrador do chip, mas muda o
     estado da placa).
3. **Proibido:** `WRITE_REG` direto pelo nosso código; qualquer escrita no controlador de eFuse
   (`EFUSE_PGM_*`, `EFUSE_CMD`); `FLASH_BEGIN/DATA/END`, `ERASE_*`; `MEM_BEGIN/DATA/END`
   (carregar código na RAM). `READ_REG` só em
   endereços de blocos sempre alimentados (eFuse, ID/sistema, ROM): periférico sem clock pode
   devolver lixo ou travar o barramento.
4. **Estado final da placa: hard reset de volta para a aplicação.** Toda sessão termina com
   reset com as linhas de strapping soltas: o chip relê o strapping, sai do modo download e
   roda o que estava na flash, como antes do teste. Se a porta sumiu e o reset não for
   possível, o relatório diz isso (não é silencioso).
5. **Guarda no código:** a implementação real do `Probe` (ADR-011) só chama uma lista
   fechada de funções do espflash; um teste de guarda em `host/tests/` (no estilo de
   `layering.rs`) falha se o módulo `probe` mencionar as APIs de gravação, apagamento,
   `write_reg` ou carga de RAM.

## Opções descartadas
- **Usar o stub (padrão do espflash):** mais rápido, mas é código nosso rodando na CPU e uma
  escrita na SRAM; o stub de chips novos (C5, P4 rev3) pode atrasar em relação ao ROM; e não
  precisamos de nada que só o stub faz.
- **Ler o tamanho da flash só pelo cabeçalho da imagem/eFuse, sem RDID:** dispensaria as
  escritas no SPI, mas o cabeçalho diz o que o firmware *declarou*, não o que está soldado; o
  RDID é o fato.
- **Subir o baud por padrão:** ganho irrelevante para o volume da v0.1.
- **Deixar a placa parada no ROM no fim:** a placa "some" para o usuário até ele apertar
  reset; contraria "estado igual ao de antes".

## Consequências
- O critério 4 do PRD (flash idêntica antes e depois) vale por construção: nenhum comando de
  flash é emitido; a verificação manual continua em `hardware.md`.
- O "não destrutivo" do ADR-003 passa a ter definição operacional: **nada persistente muda;
  o volátil que muda está listado aqui e some no hard reset final**.
- A v0.2 reabre a questão do stub (gravação do firmware de teste) num ADR próprio.
- Plataformas: nada muda entre Windows, Linux e macOS. No WSL2 (fase 2) o hard reset pela
  porta nativa derruba o repasse do usbipd; a ponte como porta primária (ADR-008) contorna.

## A verificar (no código do espflash fixado no `Cargo.lock`, antes de aceitar)
- ~~Lista completa das escritas~~ **Conferido no código do espflash 4.5.0 (2026-11-26).**
  Com `use_stub = false` e `baud = None` não há `CHANGE_BAUDRATE`, `MEM_*`, `FLASH_*` nem
  desligamento de watchdog. As escritas, todas voláteis, são:
  1. DTR/RTS (`UsbJtagSerialReset` na nativa; `ClassicReset` 50/500 ms nas pontes;
     `UnixTightReset` antes no Unix), até 7 tentativas.
  2. `SPI_ATTACH` (IO MUX + controlador SPI), com os pinos padrão e depois os do ESP32-PICO-D4.
  3. RDID no SPI1: `USR`, `USR2`, `MISO_DLEN` (ou `USR1` sem registradores de comprimento),
     `W0`, `CMD`; `USR`/`USR2` restaurados. Feito pelo `flash_detect` do espflash e pelo nosso
     `probe/rdid.rs` (o espflash não expõe o JEDEC completo, lacuna da #16).
  4. **`SPI_SET_PARAMS`**: grava na RAM do ROM os parâmetros da flash (tamanho, bloco 64 KiB,
     setor 4 KiB, página 256, máscara de status). Volátil; acrescentado ao item 2.
  5. **Só no ESP32-P4 rev 3.1/3.2** (`power_on_flash`): `LP_SYSTEM_REG_ANA_XPD_PAD_GROUP_REG`
     e registradores do LDO do PMU, para ligar a alimentação da flash. Volátil.
  Bases do SPI1: ESP32 `0x3FF42000`; C3/S3 `0x60002000`; C5/C6/H2 `0x60003000`; P4
  `0x5008D000`.
- `CMD`, `W0` e `USR1` não são restaurados pelo `spi_command`: confirmar que o hard reset
  final basta (deve bastar: são flip-flops com valor de reset).
- **Secure Download Mode** (eFuse): o ROM recusa parte dos comandos; confirmar quais (em
  especial `READ_REG`). Testes que dependem deles viram SKIP com motivo, não FAIL.
- Hard reset: ponte = RTS 1 → 100 ms → 0; nativa = DTR 0, RTS 1, DTR 0, RTS 1, 100 ms, RTS 0.
  Conferido na C3 e na C6 (2026-11-26) pelas duas portas: o ROM responde de novo na próxima
  conexão e a porta nativa re-enumera (ver `hardware.md`). Falta confirmar visualmente que a
  aplicação volta a rodar (C3 com o firmware do esp32-logcat).
