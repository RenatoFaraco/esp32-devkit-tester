# ADR-010: O relatório como contrato — estados, `--json`, códigos de saída e seleção de placa
- Status: proposta
- Data: 2026-11-26

## Contexto
O relatório tem três consumidores: a TUI (H6/H7), o texto do `--no-tui` e scripts/CI pelo
`--json` e pelo código de saída (H8). Se cada um inventar sua semântica, um SKIP vira falha no
CI ou uma divergência de tabela some. O ADR-004 já disse que o lido da placa prevalece sobre a
tabela e que "divergência vira achado", mas não disse como o achado aparece nem se ele falha o
teste. E com várias placas ligadas, o modo sem TUI precisa saber o que testar sem perguntar.

## Decisão
1. **Três estados por teste, e só três:** `pass`, `fail`, `skip`. `fail` e `skip` exigem
   motivo em texto. Cada teste tem suíte, nome, estado, motivo, duração (ms, inteiro) e os
   valores esperado/lido quando houver comparação.
2. **Achado (divergência tabela × lido) não é um quarto estado.** É uma anotação `finding` no
   teste (campo, valor da tabela, valor lido, fonte da tabela). O estado depende do tipo do
   campo na tabela de capacidades (ADR-004):
   - campo **invariante do silício** (registrador de ID, família, número de núcleos, presença
     de USB-Serial/JTAG) → divergência é `fail`;
   - campo **variável por módulo/pacote** (tamanho da flash, PSRAM, pacote, cristal) → `pass`
     com achado; a ficha mostra o valor lido.
   Achado nunca muda o código de saída sozinho.
3. **Execução interrompida** (placa desconectada, timeout que derruba a sessão): o teste em
   andamento é `fail` com a mensagem; os seguintes que dependiam da sessão são `skip` com motivo
   apontando a causa (`not run: <suíte/teste> failed`). Nenhum teste é omitido (ADR-004).
4. **`--json` é contrato versionado** em `docs/context/spec/report-json.md`, com
   `schema_version` inteiro no topo. Mudança aditiva (campo novo) não troca a versão e o
   consumidor deve ignorar campo desconhecido; remover/renomear campo ou mudar significado
   troca a versão. JSON só no stdout; diagnóstico só no stderr. Texto vindo da placa entra
   sanitizado (sem controle nem escape). A spec é escrita pelo `architect` antes da issue do
   H8; este ADR fixa só as regras acima.
5. **Códigos de saída:**
   - `0` — execução completa sem nenhum `fail` (`skip` não é falha);
   - `1` — ao menos um `fail`;
   - `2` — não houve execução: nenhuma placa, seleção ambígua, argumento inválido, ambiente
     não suportado (WSL1), execução cancelada pelo usuário.
   Uma vez que a execução começou, porta ocupada, timeout e desconexão são resultado de teste
   (`fail`, código 1), não erro de ambiente.
6. **Seleção de placa sem `--port`:** faz-se o passo de identificação (ADR-008) e então:
   - nenhuma placa → código 2 com mensagem;
   - uma placa → roda;
   - várias → **TUI:** lista para o usuário escolher; **`--no-tui`:** não adivinha, lista as
     placas (chip, MAC, portas) no stderr e sai com 2.
   `--port <nome-da-porta | número-de-série-USB>` testa só aquela porta, sem identificar as
   outras; o valor é argumento de execução, nunca gravado.

## Opções descartadas
- **Quarto estado `warn`/`finding` no mesmo nível de PASS/FAIL/SKIP:** o resumo estilo Jest e
  o código de saída ficam ambíguos ("warn falha o CI?"); a anotação dá a mesma informação.
- **Toda divergência é FAIL:** placa com 8 MB de flash onde a tabela supõe 4 MB não está
  quebrada; reprovaria placas boas.
- **Testes não executados como FAIL:** infla a contagem de falhas com uma causa só.
- **Testar todas as placas sem `--port`:** reseta placas que o usuário não pediu e mistura
  relatórios; fica para depois, se surgir demanda (`--all`).
- **JSON sem versão:** qualquer mudança quebra consumidores em silêncio.

## Consequências
- `report` modela exatamente isto e é testável sem terminal; TUI, texto e JSON são três
  renderizações do mesmo modelo.
- A tabela de capacidades ganha, por campo, a marca "invariante" ou "variável" (issue da
  tabela de capacidades).
- `docs/context/spec/` passa a ter um contrato que não é do fio host ↔ placa, mas do host ↔
  consumidor; o README da pasta registra isso.
- Plataformas: códigos de saída e stdout/stderr têm o mesmo significado em PowerShell
  (`$LASTEXITCODE`) e em shells POSIX (`$?`); o texto sem TTY usa marcadores ASCII.
