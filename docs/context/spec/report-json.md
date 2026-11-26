# Spec — relatório `--json` (schema v1)

> Status: **corrente (v0.1)** · `schema_version`: **1** · Contrato host ↔ consumidor (scripts, CI)
> Regras de origem: ADR-010 (estados, `finding`, códigos de saída, versão, sanitização),
> ADR-008 (identidade = MAC base, portas), ADR-004 e `host/src/chips.rs` (nomes de campo),
> ADR-009 (hard reset no fim), ADR-007 (plataformas). O ADR-010 ainda está como proposta:
> se ele mudar ao ser aceito, esta spec muda junto.

## 1. Onde o documento aparece

`devkit-tester --no-tui --json [--port <porta | número-de-série-USB>]`

- `--json` implica `--no-tui`: a TUI e o JSON disputam o mesmo stdout.
- **stdout:** exatamente **um** documento JSON (um objeto), em **uma linha**, terminado por
  `\n` (LF, também no Windows). Nada mais vai para o stdout: nem progresso, nem aviso.
- **stderr:** tudo o que é diagnóstico (progresso, lista de placas na seleção ambígua, erro de
  argumento). Texto livre, **não** é contrato.
- **Codificação:** UTF-8 sem BOM. Na prática o documento inteiro é ASCII (regra 5.1), então é
  igual em UTF-8, em qualquer página de código OEM do Windows e em Latin-1.
- A indentação dos exemplos abaixo é só para leitura; o consumidor usa um parser JSON e não
  depende de espaço, quebra de linha nem ordem das chaves.

### Código de saída × conteúdo do stdout

| Código | Significado (ADR-010 §5) | stdout |
|--------|--------------------------|--------|
| `0` | Execução completa, nenhum teste `fail` (`skip` não é falha) | um documento; `summary.failed == 0` |
| `1` | Execução feita, ao menos um `fail` (inclui porta ocupada, timeout, desconexão **depois** que a execução começou) | um documento; `summary.failed > 0` |
| `2` | **Não houve execução:** nenhuma placa, seleção ambígua, argumento inválido, ambiente não suportado (WSL1), cancelado pelo usuário | **vazio** (0 bytes); motivo só no stderr |

O consumidor olha o código de saída **antes** de ler o stdout. Com `0` ou `1` o documento é
obrigatório e completo; com `2` não há documento. Invariante: código `1` ⇔ `summary.failed > 0`.

Cancelamento no meio da execução (Ctrl+C no `--no-tui`) é código 2 e stdout vazio, mesmo que
alguns testes já tenham rodado: relatório parcial não é relatório. O hard reset final (ADR-009)
é tentado do mesmo jeito; o resultado dele vai para o stderr.

## 2. Estrutura

```
{
  schema_version: 1,
  tool:       { name, version },
  board:      { chip: {target, name, description, revision: {major, minor}} | null,
                mac | null,
                ports: [ {kind, vid, pid, bridge_model, usb_serial, primary}, ... ] },
  tests:      [ {suite, name, state, reason, duration_ms, expected, read, finding}, ... ],
  summary:    { passed, failed, skipped, total },
  hard_reset: { done, reason }
}
```

**Regra de presença:** o host sempre emite **todas** as chaves do schema v1 em todos os
objetos. Valor ausente, desconhecido ou "não se aplica" é `null`, nunca chave omitida. (O
consumidor ainda precisa tolerar a falta de chaves **adicionadas depois** da v1 em documentos
antigos; ver §6.)

## 3. Campos

### 3.1 Topo

| Campo | Tipo | Obrigatório | Descrição |
|-------|------|-------------|-----------|
| `schema_version` | inteiro | sim | Sempre `1` nesta spec. Primeira chave emitida pelo host (convenção; o consumidor não depende da ordem). |
| `tool` | objeto | sim | Quem gerou o documento. |
| `tool.name` | string | sim | Sempre `"devkit-tester"`. |
| `tool.version` | string | sim | Versão SemVer do binário, sem `v` (`"0.1.0"`). Diz qual catálogo de testes (§4) vale. |
| `board` | objeto | sim | A placa testada nesta execução (uma por documento). |
| `tests` | array de objetos | sim | Todos os testes da execução, nenhum omitido (ADR-004, ADR-010 §3). Nunca vazio com código 0/1. |
| `summary` | objeto | sim | Contagens; derivadas de `tests`. |
| `hard_reset` | objeto | sim | Se a placa voltou para a aplicação no fim (ADR-009 §4). |

### 3.2 `board`

| Campo | Tipo | Obrigatório | Descrição |
|-------|------|-------------|-----------|
| `board.chip` | objeto \| `null` | sim | `null` quando o chip não foi identificado nesta execução (ex.: `--port` numa ponte cujo sync falhou). |
| `board.chip.target` | string | sim | Nome de alvo do ESP-IDF/esptool, igual a `ChipCaps::target` (`"esp32c6"`). Regex `^[a-z0-9]+$`. |
| `board.chip.name` | string | sim | Nome comercial da família, igual a `ChipCaps::name` (`"ESP32-C6"`). Vem da tabela, não da placa. |
| `board.chip.description` | string \| `null` | sim | Descrição que o ROM/eFuse dá via espflash (`"ESP32-D0WD-V3"`, `"ESP32-C6 (QFN40)"`). **Texto da placa:** sanitizado (§5). `null` se não lida. |
| `board.chip.revision` | objeto \| `null` | sim | Revisão do silício lida do eFuse. `null` se não lida. |
| `board.chip.revision.major` | inteiro ≥ 0 | sim | Revisão maior (`3` em "v3.1"). |
| `board.chip.revision.minor` | inteiro ≥ 0 | sim | Revisão menor (`1` em "v3.1"). |
| `board.mac` | string \| `null` | sim | **Identidade da placa** (ADR-008 §1): MAC base de fábrica, eFuse, formato canônico §5.2. `null` se nenhuma porta chegou a ler o MAC e não há porta nativa com número de série. |
| `board.ports` | array de objetos | sim | As portas da placa, na **ordem em que foram testadas** (ponte primeiro, depois nativa — ADR-008 §5). Ao menos uma. No máximo uma porta de cada `kind`. |
| `ports[].kind` | string | sim | `"bridge"` (ponte USB-UART nos pinos da UART0) ou `"native"` (USB-Serial/JTAG, `303A:1001`). Enum aberto (§6). |
| `ports[].vid` | string | sim | USB Vendor ID, 4 dígitos hexadecimais **maiúsculos**, sem `0x` (`"303A"`). Regex `^[0-9A-F]{4}$`. |
| `ports[].pid` | string | sim | USB Product ID, mesmo formato (`"1001"`). |
| `ports[].bridge_model` | string \| `null` | sim | Modelo da ponte pela tabela VID:PID do host (`"CP210x"`, `"CH340"`, `"CH340K"`, `"CH343"`, `"CH9102"`, `"FTDI"`). `null` em `native`. Texto do host, não da placa. |
| `ports[].usb_serial` | string \| `null` | sim | Número de série USB **como o SO o entrega** (sanitizado, §5). Na nativa é o MAC no formato do descritor (`"E8:F6:0A:F7:FF:D4"`); na ponte é da ponte, não do chip (ADR-008), ou `null` quando ela não tem (CH340K). `null` também se o SO não o expuser (ver ADR-008, "A verificar"). É o valor que se passa a `--port` para repetir o teste nesta porta. |
| `ports[].primary` | booleano | sim | `true` na porta pela qual rodaram as suítes `rom`, `efuse` e `flash` (ADR-008 §5). Exatamente uma porta `true`. |

**O nome da porta (`COM5`, `/dev/ttyACM0`) não está no documento**, de propósito (§7, D1).

### 3.3 `tests[]`

| Campo | Tipo | Obrigatório | Descrição |
|-------|------|-------------|-----------|
| `suite` | string | sim | Suíte. Na v0.1: `"port"`, `"rom"`, `"efuse"`, `"flash"`. Regex `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`. Enum aberto: versões futuras somam suítes (§6). |
| `name` | string | sim | Nome do teste, mesma regex. O par `suite/name` é único no documento e é o identificador estável do teste (catálogo em §4). |
| `state` | string | sim | `"pass"`, `"fail"` ou `"skip"`. **Enum fechado** (ADR-010 §1). |
| `reason` | string \| `null` | sim | Motivo. **Obrigatório (não vazio) em `fail` e `skip`**; `null` em `pass`. Texto do host, em inglês. Teste não executado por falha anterior: exatamente `not run: <suite>/<name> failed` (ex.: `"not run: flash/jedec-id failed"`). |
| `duration_ms` | inteiro ≥ 0 | sim | Duração em milissegundos, arredondada para baixo. `0` em teste que não executou (`skip`). |
| `expected` | string \| `null` | sim | Valor esperado quando o teste compara algo; `null` quando só lê. Notação por teste em §4. |
| `read` | string \| `null` | sim | Valor lido da placa, mesma notação de `expected`; `null` se não chegou a ler (`skip`, ou `fail` antes da leitura). Pode conter texto da placa: sanitizado (§5). |
| `finding` | objeto \| `null` | sim | Achado: divergência entre a tabela de capacidades e o lido (ADR-010 §2). `null` quando não há. Um teste compara no máximo **um** campo da tabela, então tem no máximo um achado. |

Ordem do array: suítes na ordem fixa `port`, `rom`, `efuse`, `flash` (e as que vierem depois,
no fim); dentro da suíte, a ordem do catálogo (§4). Não é necessariamente a ordem temporal.

### 3.4 `tests[].finding`

| Campo | Tipo | Obrigatório | Descrição |
|-------|------|-------------|-----------|
| `field` | string | sim | Nome estável do campo na tabela de capacidades, **igual ao de `ChipCaps::fields()`**: `arch`, `cores`, `lp_core`, `max_clock_mhz`, `crystal_mhz`, `radios`, `usb_serial_jtag`, `usb_otg`, `psram`, `identity`, `mac_efuse_reg`. Enum aberto (a tabela cresce). |
| `kind` | string | sim | `"invariant"` ou `"variable"`, a marca do campo **naquela linha** da tabela (`FieldKind`). Explica o estado: `invariant` ⇒ o teste é `fail`; `variable` ⇒ `pass` (ADR-010 §2). Enum fechado. |
| `table` | string | sim | Valor da tabela, exatamente o `FieldView::value` (`"40 MHz"`, `"yes"`, `"security info chip id 13"`). |
| `read` | string | sim | Valor lido, na mesma notação de `table`. |
| `source` | string | sim | Fonte da linha da tabela: o primeiro `Source` da linha, formatado `"<doc>, <section>"` (`"ESP32 Series Datasheet v5.3, Features; ESP32 Series Comparison"`). |

O achado nunca muda o código de saída sozinho (ADR-010 §2): quem decide é o `state`.

### 3.5 `summary` e `hard_reset`

| Campo | Tipo | Obrigatório | Descrição |
|-------|------|-------------|-----------|
| `summary.passed` | inteiro ≥ 0 | sim | Testes com `state == "pass"` (com ou sem achado). |
| `summary.failed` | inteiro ≥ 0 | sim | Testes com `state == "fail"`. |
| `summary.skipped` | inteiro ≥ 0 | sim | Testes com `state == "skip"`. |
| `summary.total` | inteiro ≥ 1 | sim | `len(tests)` = `passed + failed + skipped`. |
| `hard_reset.done` | booleano | sim | `true` se a sessão terminou com o hard reset de volta à aplicação (ADR-009 §4). |
| `hard_reset.reason` | string \| `null` | sim | Por que não foi feito (`"port disconnected"`). `null` quando `done == true`. |

Um hard reset que não aconteceu **não** é `fail` nem muda o código de saída: a causa (porta
sumiu, timeout) já é `fail` em algum teste; aqui só se informa em que estado a placa ficou.

## 4. Catálogo de testes da v0.1

O par `suite/name` identifica o teste. O catálogo é do binário (`tool.version`), não do schema:
somar testes é aditivo; renomear ou remover um teste sai no `CHANGELOG.md` como mudança
incompatível, sem trocar `schema_version` (§7, D6). Os nomes abaixo valem para H3–H5; se a
implementação precisar mudar algum antes do `v0.1.0`, muda aqui no mesmo PR.

| Suíte / teste | O que verifica | `expected` | `read` | Campo da tabela (achado) |
|---------------|----------------|------------|--------|--------------------------|
| `port/uart-bridge` | Sync com o ROM pela ponte + MAC lido igual a `board.mac`. Existe só se a placa tem uma ponte no grupo (§7, D7). | MAC (§5.2) | MAC | — |
| `port/usb-serial-jtag` | Sync com o ROM pela porta nativa + MAC lido igual a `board.mac` e ao número de série (ADR-008 §6). Sempre presente. SKIP `chip has no USB-Serial/JTAG; JTAG needs an external probe (GPIO12-15)` quando a tabela diz `no`; SKIP `native USB not connected on this board` quando a tabela diz `yes` e não há porta `303A:1001` no grupo. | MAC | MAC | `usb_serial_jtag` (porta nativa com o MAC de um chip que a tabela diz não ter → `fail` + achado `invariant`) |
| `rom/chip-identity` | Identidade do chip pelo ROM igual à da tabela (registrador mágico no ESP32, `GET_SECURITY_INFO` nos demais). | `FieldView` de `identity` | idem | `identity` |
| `rom/revision` | Revisão do silício legível. | `null` | `v<major>.<minor>` (`"v0.2"`) | — |
| `rom/crystal` | Cristal detectado pelo ROM. | `FieldView` de `crystal_mhz` (`"40 MHz"`) | idem | `crystal_mhz` |
| `rom/mac-efuse-reg` | `READ_REG` nas palavras do eFuse em `mac_efuse_reg` decodifica o mesmo MAC que o ROM informou (H5). | MAC do ROM | MAC do eFuse | — |
| `efuse/package` | Versão de pacote no eFuse. | `null` | `package <n>` | — |
| `efuse/psram` | PSRAM no encapsulamento declarada no eFuse. | `null` | `none in package` ou `<n> MB in package (<fornecedor>)` | — |
| `efuse/embedded-flash` | Flash no encapsulamento declarada no eFuse. | `null` | `none` ou `<n> MB (<fornecedor>)` | — |
| `flash/jedec-id` | RDID (`0x9F`) responde; `0x000000` e `0xFFFFFF` são `fail` (barramento sem flash). | `null` | `0x` + 6 dígitos hex maiúsculos: fabricante + dispositivo (`"0x684018"`) | — |
| `flash/size` | Tamanho deduzido do byte de capacidade do JEDEC ID. | `null` | `<n> MB` (MB = 2^20 bytes, como o esptool) | — |

Notações gerais: MAC no formato §5.2; registradores e endereços `0x` + 8 dígitos hex
maiúsculos (como `FieldView`); valores de campo da tabela exatamente como `FieldView::value`.

## 5. Sanitização e formatos canônicos

### 5.1 Texto

- **Todo valor string do documento é ASCII imprimível (`0x20`–`0x7E`).** Isso vale para texto
  do host e, principalmente, para texto que veio da placa ou do descritor USB
  (`chip.description`, `usb_serial`, partes de `read`). Qualquer outro caractere (controle,
  `ESC`, `DEL`, não-ASCII, byte inválido de UTF-8) vira `?` **antes** de entrar no modelo do
  `report`. Assim o JSON nunca carrega `\u001b` que um consumidor ingênuo imprimiria no terminal.
- O único escape JSON que pode aparecer é `\"` e `\\` (o serializador escapa aspas e barra
  invertida); nenhum `\uXXXX`, nenhum `\n` dentro de string.
- Bytes crus da serial (lixo, resposta inesperada do ROM) **nunca** são copiados para `reason`
  ou `read`; o host os descreve em hexadecimal (`"unexpected response 0x1B 0x5B"`), no máximo
  16 bytes.
- Limites: string que veio da placa/descritor, até **128** caracteres; qualquer outra string,
  até **256**. Excedeu → corta e termina em `...` dentro do limite.
- Documento inteiro: menos de 1 MiB (na v0.1, poucos KiB).

### 5.2 MAC

Formato canônico de `board.mac`, de `expected`/`read` dos testes de MAC: **seis octetos em hex
minúsculo separados por `:`**, regex `^[0-9a-f]{2}(:[0-9a-f]{2}){5}$` (`"e8:f6:0a:f7:ff:d4"`).
É a normalização do ADR-008 §1. O número de série da porta nativa chega como
`E8:F6:0A:F7:FF:D4` (maiúsculas com `:`, conferido na C6); para comparar, o host o normaliza
para este formato, mas `ports[].usb_serial` guarda o texto como o SO entregou (é o que o usuário
digita em `--port`). O EUI-64 do 802.15.4 não é identidade e não aparece na v1.

### 5.3 VID:PID

`vid`/`pid`: 4 dígitos hex maiúsculos, como em `hardware.md` e no Gerenciador de Dispositivos.
O host normaliza o que o `serialport` entrega (inteiro `u16`), então o formato não depende do SO.

## 6. Versionamento (ADR-010 §4)

- `schema_version` é um inteiro; esta spec define `1`.
- **Aditivo não troca a versão:** chave nova em qualquer objeto; valor novo num enum **aberto**
  (`suite`, `ports[].kind`, `finding.field`, `bridge_model`); teste novo no catálogo.
- **Troca a versão:** remover ou renomear chave; mudar tipo ou significado de um campo; mudar
  um formato canônico (§5); valor novo num enum **fechado** (`state`, `finding.kind`); mudar
  a relação código de saída ↔ conteúdo (§1).
- **O consumidor deve:** checar `schema_version` e recusar versão que não conhece; **ignorar**
  chave desconhecida; tratar valor desconhecido de enum aberto como "outro" (uma suíte nova não
  pode quebrar o script); não depender da ordem das chaves.
- Uma versão nova desta spec mantém a anterior como seção "schema v1" enquanto o host puder
  emiti-la; não há negociação de versão na v0.1.

## 7. Decisões desta spec

| # | Decisão | Por quê | Descartado |
|---|---------|---------|------------|
| D1 | Sem nome de porta (`COMx`, `ttyACMx`) no JSON | ADR-007: porta nunca é identificada por nome; o número muda de PC para PC, então o documento não seria comparável entre máquinas nem usável como fixture versionada. `vid`/`pid`/`usb_serial` identificam; o nome aparece no stderr. | Incluir `port_name`: ajudaria a achar um CH340K sem número de série, mas convida script a chavear pelo nome e suja fixture/artefato de CI. |
| D2 | Código 2 ⇒ stdout vazio | "Não houve execução" não é relatório; um documento com `tests: []` passa em quem só checa `failed == 0`. O código de saída é o discriminador e o diagnóstico já tem lugar (stderr). | JSON de erro (`{"error": ...}`): segundo formato a versionar, e na seleção ambígua o ADR-010 já manda a lista para o stderr. |
| D3 | Todas as chaves sempre presentes, `null` para ausente | Um formato só, validável chave a chave; `select(.finding)` e `$t.finding` funcionam igual em jq e PowerShell. | Omitir chave opcional: dois jeitos de dizer "não há". |
| D4 | Todo string é ASCII imprimível; resto vira `?` | Uma regra só cobre texto da placa e do host, o documento fica idêntico em qualquer codificação do console (Windows PowerShell 5.1 decodifica stdout nativo pela página OEM). | Permitir UTF-8 e só tirar controles: o host não ganha nada (a placa não manda texto legítimo não-ASCII na v0.1) e reabre o problema da codificação. Escapar `\xHH`: ambíguo com `\` vindo da placa. |
| D5 | Uma linha compacta + `\n` | Fácil de validar byte a byte e de concatenar em log (uma execução = uma linha). | JSON indentado: legível, mas o texto humano já é o `--no-tui` sem `--json`. |
| D6 | Catálogo de testes versionado pelo binário, não pelo schema | O catálogo cresce a cada versão (v0.2 traz suítes); amarrá-lo ao schema trocaria a versão a toda release. Renomear teste continua sendo anunciado no CHANGELOG. | Catálogo dentro do schema: `schema_version` viraria número de release. |
| D7 | `port/usb-serial-jtag` sempre presente; `port/uart-bridge` só se houver ponte | A tabela diz se o chip tem USB-Serial/JTAG, então a ausência da porta é informação (SKIP com motivo, ADR-004). Se a placa tem ponte é propriedade da placa, que o host não tem como saber (há placas só com USB nativo): um SKIP seria ruído em toda placa assim. | `uart-bridge` sempre presente com SKIP: SKIP permanente sem informação. |
| D8 | `finding` objeto único, um campo da tabela por teste | É o que o ADR-010 chama de "anotação"; uma linha do relatório = um fato, mais claro no estilo Jest. | `findings: []`: mais geral, mas nenhum teste da v0.1 compara dois campos; se precisar, chave nova aditiva. |
| D9 | `finding.kind` além dos quatro campos pedidos | Sem ele o consumidor não sabe por que uma divergência é `pass` e outra `fail`. Aditivo sobre o ADR-010. | Deixar o consumidor consultar a tabela: ela está dentro do binário. |

## 8. Exemplos

Todos são JSON válido e saídas completas (o host as emitiria em uma linha). Cada um traz um
marcador `<!-- example: <id> -->` antes do bloco para o teste do H8 poder extraí-los daqui
(fonte única; ver §10). Os dados são das fixtures do bring-up parte 1
(`host/tests/fixtures/*.json`); as durações são ilustrativas.

### 8.1 `pass` — ESP32-C6 pela porta nativa (código 0)

Placa só com a porta `303A:1001` ligada: não há `port/uart-bridge` (D7).

<!-- example: pass -->
```json
{
  "schema_version": 1,
  "tool": { "name": "devkit-tester", "version": "0.1.0" },
  "board": {
    "chip": {
      "target": "esp32c6",
      "name": "ESP32-C6",
      "description": "ESP32-C6 (QFN40)",
      "revision": { "major": 0, "minor": 2 }
    },
    "mac": "e8:f6:0a:f7:ff:d4",
    "ports": [
      { "kind": "native", "vid": "303A", "pid": "1001", "bridge_model": null, "usb_serial": "E8:F6:0A:F7:FF:D4", "primary": true }
    ]
  },
  "tests": [
    { "suite": "port", "name": "usb-serial-jtag", "state": "pass", "reason": null, "duration_ms": 1840, "expected": "e8:f6:0a:f7:ff:d4", "read": "e8:f6:0a:f7:ff:d4", "finding": null },
    { "suite": "rom", "name": "chip-identity", "state": "pass", "reason": null, "duration_ms": 12, "expected": "security info chip id 13", "read": "security info chip id 13", "finding": null },
    { "suite": "rom", "name": "revision", "state": "pass", "reason": null, "duration_ms": 9, "expected": null, "read": "v0.2", "finding": null },
    { "suite": "rom", "name": "crystal", "state": "pass", "reason": null, "duration_ms": 3, "expected": "40 MHz", "read": "40 MHz", "finding": null },
    { "suite": "rom", "name": "mac-efuse-reg", "state": "pass", "reason": null, "duration_ms": 8, "expected": "e8:f6:0a:f7:ff:d4", "read": "e8:f6:0a:f7:ff:d4", "finding": null },
    { "suite": "efuse", "name": "package", "state": "pass", "reason": null, "duration_ms": 6, "expected": null, "read": "package 0", "finding": null },
    { "suite": "efuse", "name": "psram", "state": "pass", "reason": null, "duration_ms": 5, "expected": null, "read": "none in package", "finding": null },
    { "suite": "efuse", "name": "embedded-flash", "state": "pass", "reason": null, "duration_ms": 5, "expected": null, "read": "none", "finding": null },
    { "suite": "flash", "name": "jedec-id", "state": "pass", "reason": null, "duration_ms": 31, "expected": null, "read": "0x684018", "finding": null },
    { "suite": "flash", "name": "size", "state": "pass", "reason": null, "duration_ms": 0, "expected": null, "read": "16 MB", "finding": null }
  ],
  "summary": { "passed": 10, "failed": 0, "skipped": 0, "total": 10 },
  "hard_reset": { "done": true, "reason": null }
}
```

### 8.2 `skip` — ESP32 original pela ponte CH9102 (código 0)

O ESP32 não tem USB-Serial/JTAG (tabela: `usb_serial_jtag = no`): SKIP com motivo, não FAIL
(PRD §7, critério 2).

<!-- example: skip -->
```json
{
  "schema_version": 1,
  "tool": { "name": "devkit-tester", "version": "0.1.0" },
  "board": {
    "chip": {
      "target": "esp32",
      "name": "ESP32",
      "description": "ESP32-D0WD-V3",
      "revision": { "major": 3, "minor": 1 }
    },
    "mac": "1c:c3:ab:d2:25:b4",
    "ports": [
      { "kind": "bridge", "vid": "1A86", "pid": "55D4", "bridge_model": "CH9102", "usb_serial": "5B53026573", "primary": true }
    ]
  },
  "tests": [
    { "suite": "port", "name": "uart-bridge", "state": "pass", "reason": null, "duration_ms": 2310, "expected": "1c:c3:ab:d2:25:b4", "read": "1c:c3:ab:d2:25:b4", "finding": null },
    { "suite": "port", "name": "usb-serial-jtag", "state": "skip", "reason": "chip has no USB-Serial/JTAG; JTAG needs an external probe (GPIO12-15)", "duration_ms": 0, "expected": null, "read": null, "finding": null },
    { "suite": "rom", "name": "chip-identity", "state": "pass", "reason": null, "duration_ms": 11, "expected": "magic register 0x40001000 = 0x00F01D83", "read": "magic register 0x40001000 = 0x00F01D83", "finding": null },
    { "suite": "rom", "name": "revision", "state": "pass", "reason": null, "duration_ms": 14, "expected": null, "read": "v3.1", "finding": null },
    { "suite": "rom", "name": "crystal", "state": "pass", "reason": null, "duration_ms": 3, "expected": "40 MHz", "read": "40 MHz", "finding": null },
    { "suite": "rom", "name": "mac-efuse-reg", "state": "pass", "reason": null, "duration_ms": 9, "expected": "1c:c3:ab:d2:25:b4", "read": "1c:c3:ab:d2:25:b4", "finding": null },
    { "suite": "efuse", "name": "package", "state": "pass", "reason": null, "duration_ms": 6, "expected": null, "read": "package 1", "finding": null },
    { "suite": "efuse", "name": "psram", "state": "pass", "reason": null, "duration_ms": 5, "expected": null, "read": "none in package", "finding": null },
    { "suite": "efuse", "name": "embedded-flash", "state": "pass", "reason": null, "duration_ms": 5, "expected": null, "read": "none", "finding": null },
    { "suite": "flash", "name": "jedec-id", "state": "pass", "reason": null, "duration_ms": 38, "expected": null, "read": "0x5E4016", "finding": null },
    { "suite": "flash", "name": "size", "state": "pass", "reason": null, "duration_ms": 0, "expected": null, "read": "4 MB", "finding": null }
  ],
  "summary": { "passed": 10, "failed": 0, "skipped": 1, "total": 11 },
  "hard_reset": { "done": true, "reason": null }
}
```

### 8.3 `fail` + `not run` — ESP32-S3 pela ponte CH343, cabo puxado na leitura da flash (código 1)

A placa foi desconectada durante `flash/jedec-id` (ADR-010 §3): esse teste é `fail` com a
mensagem; `flash/size`, que dependia da sessão, é `skip` com `not run: flash/jedec-id failed`.
A porta nativa da S3 não estava ligada (SKIP, D7). O hard reset não foi possível.

<!-- example: fail -->
```json
{
  "schema_version": 1,
  "tool": { "name": "devkit-tester", "version": "0.1.0" },
  "board": {
    "chip": {
      "target": "esp32s3",
      "name": "ESP32-S3",
      "description": "ESP32-S3 (QFN56)",
      "revision": { "major": 0, "minor": 2 }
    },
    "mac": "10:51:db:84:14:0c",
    "ports": [
      { "kind": "bridge", "vid": "1A86", "pid": "55D3", "bridge_model": "CH343", "usb_serial": "5D28088406", "primary": true }
    ]
  },
  "tests": [
    { "suite": "port", "name": "uart-bridge", "state": "pass", "reason": null, "duration_ms": 2295, "expected": "10:51:db:84:14:0c", "read": "10:51:db:84:14:0c", "finding": null },
    { "suite": "port", "name": "usb-serial-jtag", "state": "skip", "reason": "native USB not connected on this board", "duration_ms": 0, "expected": null, "read": null, "finding": null },
    { "suite": "rom", "name": "chip-identity", "state": "pass", "reason": null, "duration_ms": 12, "expected": "security info chip id 9", "read": "security info chip id 9", "finding": null },
    { "suite": "rom", "name": "revision", "state": "pass", "reason": null, "duration_ms": 9, "expected": null, "read": "v0.2", "finding": null },
    { "suite": "rom", "name": "crystal", "state": "pass", "reason": null, "duration_ms": 3, "expected": "40 MHz", "read": "40 MHz", "finding": null },
    { "suite": "rom", "name": "mac-efuse-reg", "state": "pass", "reason": null, "duration_ms": 8, "expected": "10:51:db:84:14:0c", "read": "10:51:db:84:14:0c", "finding": null },
    { "suite": "efuse", "name": "package", "state": "pass", "reason": null, "duration_ms": 6, "expected": null, "read": "package 0", "finding": null },
    { "suite": "efuse", "name": "psram", "state": "pass", "reason": null, "duration_ms": 5, "expected": null, "read": "8 MB in package (AP_3v3)", "finding": null },
    { "suite": "efuse", "name": "embedded-flash", "state": "pass", "reason": null, "duration_ms": 5, "expected": null, "read": "none", "finding": null },
    { "suite": "flash", "name": "jedec-id", "state": "fail", "reason": "port disconnected during the flash ID read", "duration_ms": 212, "expected": null, "read": null, "finding": null },
    { "suite": "flash", "name": "size", "state": "skip", "reason": "not run: flash/jedec-id failed", "duration_ms": 0, "expected": null, "read": null, "finding": null }
  ],
  "summary": { "passed": 8, "failed": 1, "skipped": 2, "total": 11 },
  "hard_reset": { "done": false, "reason": "port disconnected" }
}
```

### 8.4 `pass` com achado — ESP32 com cristal de 26 MHz (código 0)

**Exemplo sintético:** é o ESP32 da bancada (8.2) com o cristal trocado para 26 MHz, para
mostrar um achado num campo `variable`; nenhuma placa da bancada diverge da tabela hoje. O
datasheet do ESP32 aceita cristal de 2 a 60 MHz (Wi-Fi/BT pedem 40 MHz), por isso
`crystal_mhz` é `variable` e a divergência é `pass` com achado. Quando um bring-up mostrar uma
divergência real, ela substitui este exemplo.

<!-- example: finding -->
```json
{
  "schema_version": 1,
  "tool": { "name": "devkit-tester", "version": "0.1.0" },
  "board": {
    "chip": {
      "target": "esp32",
      "name": "ESP32",
      "description": "ESP32-D0WD-V3",
      "revision": { "major": 3, "minor": 1 }
    },
    "mac": "1c:c3:ab:d2:25:b4",
    "ports": [
      { "kind": "bridge", "vid": "1A86", "pid": "55D4", "bridge_model": "CH9102", "usb_serial": "5B53026573", "primary": true }
    ]
  },
  "tests": [
    { "suite": "port", "name": "uart-bridge", "state": "pass", "reason": null, "duration_ms": 2310, "expected": "1c:c3:ab:d2:25:b4", "read": "1c:c3:ab:d2:25:b4", "finding": null },
    { "suite": "port", "name": "usb-serial-jtag", "state": "skip", "reason": "chip has no USB-Serial/JTAG; JTAG needs an external probe (GPIO12-15)", "duration_ms": 0, "expected": null, "read": null, "finding": null },
    { "suite": "rom", "name": "chip-identity", "state": "pass", "reason": null, "duration_ms": 11, "expected": "magic register 0x40001000 = 0x00F01D83", "read": "magic register 0x40001000 = 0x00F01D83", "finding": null },
    { "suite": "rom", "name": "revision", "state": "pass", "reason": null, "duration_ms": 14, "expected": null, "read": "v3.1", "finding": null },
    { "suite": "rom", "name": "crystal", "state": "pass", "reason": null, "duration_ms": 3, "expected": "40 MHz", "read": "26 MHz", "finding": { "field": "crystal_mhz", "kind": "variable", "table": "40 MHz", "read": "26 MHz", "source": "ESP32 Series Datasheet v5.3, Features; ESP32 Series Comparison" } },
    { "suite": "rom", "name": "mac-efuse-reg", "state": "pass", "reason": null, "duration_ms": 9, "expected": "1c:c3:ab:d2:25:b4", "read": "1c:c3:ab:d2:25:b4", "finding": null },
    { "suite": "efuse", "name": "package", "state": "pass", "reason": null, "duration_ms": 6, "expected": null, "read": "package 1", "finding": null },
    { "suite": "efuse", "name": "psram", "state": "pass", "reason": null, "duration_ms": 5, "expected": null, "read": "none in package", "finding": null },
    { "suite": "efuse", "name": "embedded-flash", "state": "pass", "reason": null, "duration_ms": 5, "expected": null, "read": "none", "finding": null },
    { "suite": "flash", "name": "jedec-id", "state": "pass", "reason": null, "duration_ms": 38, "expected": null, "read": "0x5E4016", "finding": null },
    { "suite": "flash", "name": "size", "state": "pass", "reason": null, "duration_ms": 0, "expected": null, "read": "4 MB", "finding": null }
  ],
  "summary": { "passed": 10, "failed": 0, "skipped": 1, "total": 11 },
  "hard_reset": { "done": true, "reason": null }
}
```

Uma divergência num campo `invariant` tem a mesma forma, com `"state": "fail"`, `reason`
preenchido e `"kind": "invariant"` no achado.

### 8.5 Sem execução (código 2)

Duas placas ligadas, `--no-tui --json` sem `--port`: stdout vazio, lista no stderr (formato do
stderr não é contrato). Em PowerShell:

```powershell
$json = devkit-tester --no-tui --json
$LASTEXITCODE          # 2
$json                  # $null: nada no stdout
```

## 9. Plataformas (ADR-007)

- **O documento é o mesmo em Windows, WSL2, Linux e macOS:** sem nome de porta, VID:PID e MAC
  normalizados pelo host, `\n` como fim de linha em todos, só ASCII. Fixtures valem em qualquer
  SO do CI.
- **`usb_serial`** é o que o `serialport` entrega; se no Windows ele não expuser o número de
  série da `303A:1001` (dispositivo composto, ADR-008 "A verificar"), o campo fica `null` ali e
  preenchido no Linux. O contrato já admite `null`.
- **PowerShell:** `devkit-tester --no-tui --json | ConvertFrom-Json` funciona em 5.1 e 7.x
  (ASCII, uma linha). Código de saída em `$LASTEXITCODE`. Para gravar em arquivo, no
  Windows PowerShell 5.1 o `>` regrava em UTF-16LE: use
  `devkit-tester --no-tui --json | Set-Content -Encoding ascii report.json` (ou PowerShell 7,
  cujo `>` grava UTF-8 sem BOM). Em shells POSIX, `$?` e `>` diretos.
- WSL1 é código 2 antes de qualquer execução (ADR-007).

## 10. Para o H8 (host)

- O modelo do `report` serializa para exatamente esta forma; TUI, texto e JSON são três
  renderizações do mesmo modelo (ADR-010).
- **Fixtures:** extrair os blocos `json` marcados `<!-- example: <id> -->` deste arquivo num
  teste de integração (fonte única, a spec não diverge das fixtures) **ou** copiá-los para
  `host/tests/fixtures/report-json/<id>.json` com um teste que compara os dois. Em ambos:
  cada exemplo faz *round-trip* pelo modelo (desserializar com `deny_unknown_fields` só no teste,
  serializar, comparar como `serde_json::Value`), e `summary` bate com `tests`.
- Testes mínimos: códigos 0/1/2 e stdout vazio no 2; invariante `1 ⇔ failed > 0`;
  sanitização (`ESC`, `DEL`, não-ASCII, string longa → `?` e `...`); `reason` obrigatório em
  `fail`/`skip`; regex de MAC, VID, PID, `suite`, `name`.
- Dependência: só `serde`/`serde_json` (MIT/Apache-2.0, mantidos, multiplataforma). Validar
  com JSON Schema exigiria crate a mais sem ganho sobre o round-trip.
