# ADR-011: Costura de teste acima do espflash e concorrência do host
- Status: proposta
- Data: 2026-11-26

## Contexto
O PRD (critério 5) exige que agrupamento, runner, relatório e tratamento de erro (H9) sejam
testados no CI **sem placa**. O lugar natural para um transporte falso seria abaixo do
protocolo, injetando bytes. No espflash isso não dá: `Connection::new(serial: Port, …)` recebe
um tipo **concreto** — `Port` é `serialport::TTYPort` no Unix (docs.rs, espflash 4.7.0) e, pelo
mesmo padrão, `serialport::COMPort` no Windows — não um trait. Mesmo que desse, simular o
protocolo SLIP do ROM testaria o espflash, não o nosso código.

Do outro lado, a TUI precisa continuar respondendo enquanto o host conversa com a placa
(critério 3), e o espflash e o `serialport` são **bloqueantes**: cada comando ao ROM espera a
resposta na serial até um timeout.

## Decisão
1. **Costura = trait `Probe` no módulo `probe`, acima do espflash, devolvendo fatos.** Fato é
   o que a placa disse, já decodificado: família, revisão, cristal, MAC, palavras de eFuse
   lidas, ID da flash (3 bytes) e tamanho, leituras de registrador `(endereço, valor)`, info de
   segurança. Granularidade de um método por fato (abrir/sincronizar, cada leitura, encerrar
   com hard reset) para que o runner meça a duração e atribua a falha a cada teste. O erro é
   um enum nosso (`timeout`, `disconnected`, `busy`, `protocol`, `unsupported`…), nunca o tipo
   do espflash.
2. **Uma implementação real**, `EspflashProbe`, único lugar que importa `espflash` (ADR-002) e
   que respeita a lista fechada de chamadas do ADR-009.
3. **`FakeProbe` para testes**, que reproduz fatos gravados por placa em
   `host/tests/fixtures/<chip>.json` e falha sob encomenda (timeout no passo N, desconexão,
   porta ocupada, MAC diferente). O formato da fixture é interno do host (não é spec), tem
   campo de versão e é escrito à mão na parte 1 do bring-up a partir da saída do `esptool`
   (com a versão e o comando registrados), depois conferido com a captura do próprio
   `devkit-tester`.
4. **Fixtures de bytes crus** (bytes exatos, CRLF preservado — CONVENTIONS §6) só para parsers
   nossos: classificação de portas, sanitização de texto da placa.
5. **Concorrência: thread + canal, sem runtime async.**
   - A UI roda na thread principal: `crossterm::event::poll` com timeout curto + `try_recv`
     no canal de eventos. Nunca chama `probe`.
   - Cada execução roda numa thread de trabalho (`std::thread`), que envia eventos
     (`BoardIdentified`, `TestStarted`, `TestFinished`, `RunFinished`…) por
     `std::sync::mpsc`. Uma execução por vez; as portas de uma placa em sequência (ADR-008).
   - A enumeração do hot-plug roda numa thread separada que só lista portas e envia
     `PortsChanged`.
   - Cancelamento: flag atômica checada entre passos. Uma chamada bloqueante não é
     interrompida no meio; o teto de espera é o timeout da serial configurado na conexão, e o
     teste em andamento vira `fail` por timeout (ADR-010).
6. `runner` depende do trait, não do espflash; `report` não conhece nem o trait.

## Opções descartadas
- **Transporte falso abaixo do espflash (bytes do ROM):** impossível sem fork (tipo
  concreto), e o PRD §6 proíbe fork; testaria o protocolo dos outros.
- **Fork do espflash para aceitar um trait de porta:** manutenção nossa de uma dependência
  inteira; contraria o PRD §6.
- **Runtime async (tokio + tokio-serial):** espflash é bloqueante, então tudo acabaria em
  `spawn_blocking`; dependência grande sem ganho para uma execução por vez.
- **Chamar o probe na thread da UI:** congela a TUI por segundos em cada timeout; reprova o
  critério 3.
- **`crossbeam-channel`:** o `std::sync::mpsc` basta para um produtor e um consumidor;
  dependência nova sem motivo.

## Consequências
- O CI testa agrupamento, runner, relatório e H9 só com fixtures; a placa real fica para o
  bring-up e os critérios 1–4.
- Cada placa nova traz sua fixture; o bring-up passa a ter um artefato testável.
- O que só o espflash faz (detalhes do protocolo, sequências de reset) fica sem teste no CI;
  é coberto no bring-up de cada placa.
- Nenhuma dependência nova além do espflash (já prevista no ADR-002). Plataformas: `std::thread`,
  `mpsc` e `crossterm` se comportam igual em Windows, Linux e macOS.

## A verificar
- O alias `espflash::connection::Port` no Windows (esperado: `serialport::COMPort`) na versão
  fixada no `Cargo.lock` quando o espflash entrar no `host/Cargo.toml` (hoje ainda não está).
- Se o timeout da `Connection` cobre também o sync e o reset, ou se o runner precisa de um
  teto próprio por teste.
