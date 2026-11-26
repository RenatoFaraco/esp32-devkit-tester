# ADR-008: Identidade da placa e agrupamento das portas
- Status: proposta
- Data: 2026-11-26

## Contexto
Uma DevKit pode ter duas portas USB ligadas ao mesmo chip: o **USB-Serial/JTAG** nativo
(`303A:1001`, periférico dentro do chip, nos C3, C5, C6, S3, P4, H2) e uma **ponte USB-UART**
(CP210x, CH340, CH343/CH9102, FTDI) ligada aos pinos da UART0. O PRD (H2, critério 2) quer
**uma** placa com um teste por porta. Para agrupar, é preciso algo que identifique o chip, não
o cabo.

Do lado do hardware:
- **MAC de fábrica em eFuse.** A Espressif grava o MAC base no eFuse na fabricação (bloco 0 no
  ESP32; `EFUSE_BLK1`/`MAC_SPI_SYS` nos chips mais novos — conferir no TRM de cada chip ao
  preencher a tabela). eFuse é fusível (0 → 1 sem volta): o valor não muda com firmware, reset
  nem regravação da flash. É único por chip. Um "MAC customizado" (eFuse de usuário) não
  substitui o de fábrica, que continua legível.
- **A porta nativa já mostra o MAC sem ser aberta.** O descritor USB do USB-Serial/JTAG é
  fixo em hardware e o *string descriptor* de número de série é o MAC do chip. Por isso o
  Windows dá uma COM diferente a cada placa e a **mesma** COM à mesma placa (a COM é chaveada
  pelo número de série). Fonte: Espressif, *esp-iot-solution — Prevent Windows from
  incrementing COM numbers based on USB device serial number*. O firmware da aplicação não muda
  esse descritor (só o USB OTG do S3/P4 tem descritor programável, e aparece com outro PID).
- **A ponte não sabe nada do ESP.** O número de série de uma ponte é da ponte: o CH340 não tem
  nenhum; muitos CP2102 saem de fábrica com o mesmo (`0001`). E a posição no hub não ajuda: as
  duas portas de uma placa usam dois cabos, em duas portas USB diferentes do PC. Só dá para
  saber a que chip uma ponte está ligada conversando com o ROM por ela.
- **Abrir uma ponte mexe no chip.** Em quase toda DevKit, `RTS` → EN (reset) e `DTR` → pino de
  strapping de boot (GPIO0/GPIO9), e alguns drivers acionam as linhas ao abrir a porta.
  Abrir uma ponte desconhecida pode resetar uma placa (ou outro aparelho) que não é a que o
  usuário quer testar.
- **Um chip, um ROM.** As duas portas alimentam o mesmo laço de download do ROM; o reset feito
  por uma porta derruba a sessão da outra (e o reset do chip faz o dispositivo USB nativo sumir
  e voltar).

## Decisão
1. **Identidade da placa = MAC base de fábrica (6 bytes, eFuse)**, normalizado como texto
   `aa:bb:cc:dd:ee:ff` (minúsculas). Não se usa nome de porta, posição USB nem número de série
   de ponte. O EUI-64 do 802.15.4 (C5, C6, H2) é dado da ficha, não identidade.
2. **Antes de abrir qualquer porta (só enumeração):**
   - porta `303A:1001` → grupo identificado pelo seu número de série USB (= MAC);
   - ponte conhecida → "porta não identificada", sem grupo;
   - VID:PID desconhecido → listado como desconhecido e **nunca** aberto.
3. **Portas são abertas e resetadas só por ação do usuário** (tecla na TUI ou execução de
   `--no-tui`), nunca por hot-plug. O hot-plug só reenumera.
4. **Passo de identificação, com confirmação.** Na TUI, a ação do usuário mostra as pontes ainda
   não identificadas e pede confirmação explícita antes de abri-las, avisando que cada uma sofre
   um reset; recusar mantém essas pontes fora dos grupos. Confirmado, cada ponte não
   identificada é aberta uma vez: reset para o ROM, sync, leitura do MAC, hard reset (ADR-009).
   Em `--no-tui`, invocar o comando é a ação explícita; `--port` restringe a uma porta.
   O MAC lido junta a ponte ao grupo da porta nativa com o mesmo número de série; sem par, a
   ponte vira uma placa própria (ESP32 original, placa só com ponte). O resultado fica em
   memória durante a sessão enquanto a porta continuar enumerada; nunca é gravado em disco.
5. **Portas de uma placa são testadas em sequência, nunca em paralelo.** Ordem fixa: primeiro a
   ponte (porta primária quando existe), depois a nativa. A suíte `port` tem um teste por porta
   (sync + MAC igual à identidade do grupo); as suítes `rom`, `efuse` e `flash` rodam uma vez,
   pela porta primária. Motivo da ponte primeiro: o reset por EN não derruba o dispositivo USB
   da porta em uso; resetar pela nativa faz a própria porta sumir e voltar, o caminho frágil
   no Windows. Placa sem ponte usa a nativa como primária.
6. Porta nativa cujo número de série não for legível pela enumeração → tratada como ponte
   (identificada no passo 4). Divergência entre número de série e MAC lido pelo ROM → FAIL no
   teste `port` dessa porta.

## Opções descartadas
- **Agrupar por posição no hub / caminho USB:** dois cabos, duas posições; não agrupa nada.
- **Agrupar pelo número de série da ponte:** identifica a ponte, não o chip; repetido entre
  placas (CP2102 `0001`) ou ausente (CH340).
- **Sondar todas as portas no hot-plug:** reseta placas sem o usuário pedir e pode mexer em
  aparelhos que não são ESP; contraria o espírito do ADR-003.
- **Sondar as portas de uma placa em paralelo:** um chip só tem um ROM; o reset de uma porta
  derruba a outra.
- **Persistir o mapa ponte → MAC entre sessões:** a ponte pode ir para outra placa; seria
  dado desatualizado e, no disco, dado de máquina (ADR-007, sem COM em nada versionado).

## Consequências
- O critério 2 do PRD sai sem heurística: C6/S3 com dois cabos = um grupo, dois testes `port`.
- O passo de identificação pode abrir pontes de placas que o usuário não escolheu; por isso
  exige confirmação na TUI. Essas placas sofrem um reset (estado final igual ao de antes,
  ADR-009). Isso refina a issue do runner ("só as portas da placa escolhida são abertas" vale
  depois da identificação). `--port` restringe a uma porta e pula a identificação das outras
  (ADR-010).
- Decisões do dono (2026-11-26): confirmação antes de identificar pontes; ponte como porta
  primária.
- Reconexão (ADR-007): a porta nativa que volta com o mesmo número de série depois do reset é a
  mesma porta, mesmo que o nome mude.
- Plataformas: Windows, Linux e macOS expõem o número de série USB pelo crate `serialport`;
  nenhuma regra depende do nome da porta. No WSL2 (fase 2) o reset pela nativa derruba o
  repasse do usbipd; por isso a ponte já é a primária. Nada muda no firmware.

## A verificar
- Se o `serialport` no Windows entrega o número de série da porta `303A:1001` (dispositivo
  composto: CDC + JTAG; o número de série está no pai, não na interface `MI_00`). Se não
  entregar, vale o item 6 e o agrupamento antes da sondagem fica vazio no Windows.
- Formato do número de série no descritor (com ou sem `:`, maiúsculas) em cada chip da
  bancada; a normalização do item 1 cobre as variações vistas no bring-up.
- Localização exata do MAC de fábrica no eFuse de cada chip (TRM), registrada na tabela de
  capacidades.
