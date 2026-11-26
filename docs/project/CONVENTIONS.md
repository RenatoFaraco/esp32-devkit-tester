# Convenções — esp32-devkit-tester

> Na dúvida: consistência com o que já existe no repositório vence preferência pessoal.

## 1. Idioma
- **Código, nomes, comentários no código, commits, issues, PRs:** inglês.
- **`docs/`:** português na fase de desenvolvimento. `README.md` em inglês (portfólio/SEO).

## 2. Git
- `main` sempre compilando e com CI verde. Trabalho em `feat/…`, `fix/…`, `docs/…`, `chore/…`;
  merge por Pull Request, mesmo sozinho.
- **Conventional Commits:** `<tipo>(<escopo>): <resumo no imperativo, ≤ 72>`.
  Tipos: `feat`, `fix`, `test`, `refactor`, `docs`, `build`, `ci`, `chore`, `perf`.
  Escopos: `host`, `fw`, `spec`, `ci`, `docs`.
  ```
  feat(host): group the two ports of one board by MAC
  fix(fw): skip the PSRAM march when the eFuse says none
  ```
- Um commit = uma mudança coerente que compila e passa nos testes; bug corrigido entra com o
  teste que o reproduz.
- **Versões:** SemVer; cada marco fecha uma versão (M1 → `v0.1.0`, …, M5 → `v1.0.0`).
  `CHANGELOG.md` em *Keep a Changelog*; cada versão do changelog tem um PRD em
  `docs/context/prd/`.

## 3. C (firmware)
- C17, `-Wall -Wextra -Werror`; sem VLA; `static` em tudo que não é API.
  No IDF isso exige religar como erro os avisos que as flags globais do IDF desligam: todo
  componente nosso copia a lista de `-Werror=<x>` de `firmware/main/CMakeLists.txt` (ADR-001).
- Prefixo `dkt_` em funções, tipos e componentes (`dkt_report_test()`, `dkt_mem_result_t`);
  macros e constantes em `DKT_MAIUSCULAS`; arquivos `snake_case`.
- Tipos de largura fixa (`uint32_t`), `bool` de `<stdbool.h>`, `const` sempre que couber.
- Diferenças de chip via `SOC_*` e Kconfig (ADR-004).
- Componentes em `firmware/components/dkt_<nome>/` com `include/` público.
- `.clang-format` na raiz (base Google, indentação 4, coluna 100).
- Tags de log do IDF: maiúsculas, curtas, uma por componente (`"DKT_MEM"`).

## 4. Rust (host)
- Rust estável, edition 2024; `cargo fmt` e `cargo clippy --all-targets -- -D warnings`
  limpos. `unwrap`/`expect` só em testes e em invariantes comentadas.
- Módulos por camada: `platform` (único lugar com `#[cfg(windows)]`/`#[cfg(unix)]`), `transport`, `probe`, `chips`, `runner`, `report`, `ui` (ADR-002/004).
- Nomes: `snake_case` para funções/módulos, `PascalCase` para tipos, `SCREAMING_SNAKE` para
  constantes. Doc comments em inglês em todo item público.
- Erros: `thiserror` nas bibliotecas internas, `anyhow` só no `main`.
- Dependências: o mínimo; cada nova tem motivo no PR.

## 5. Plataformas (ADR-007)
- Referência: Windows + PowerShell. Comandos na documentação de uso em PowerShell; quando um
  passo for diferente no WSL/Linux, acrescentar um bloco `bash` ao lado, não no lugar.
- Scripts de desenvolvimento: Python (multiplataforma) ou `.ps1`; nada que só rode em bash.
- Nunca escrever `COMx`, `ttyACMx` ou caminhos com `\` no código ou em config versionada.

## 6. Testes
| Onde | Ferramenta | Comando |
|------|-----------|---------|
| Host | `cargo test` | `cargo test --manifest-path host/Cargo.toml` |
| Host, lint | clippy / rustfmt | ver `test-runner` |
| Firmware | Unity na placa (v0.2+) | fixado no M0 / v0.2 |

- Host testado sem placa: transporte falso e fixtures de bytes reais capturados da placa em
  `host/tests/fixtures/` (bytes exatos, CRLF preservado).
- Nome do teste descreve o comportamento: `groups_native_and_bridge_ports_of_one_board`.

## 7. Documentação
- Mudou o comportamento → documento correspondente atualizado no mesmo PR.
- `ROADMAP.md`: marcar itens ao fechar cada PR.
- Decisão não óbvia → ADR em `docs/context/decisions/NNN-titulo-curto.md`:

```markdown
# ADR-NNN: <título>
- Status: proposta | aceita | substituída por ADR-XXX
- Data: AAAA-MM-DD

## Contexto
<qual problema/força motivou a decisão>

## Decisão
<o que foi decidido>

## Consequências
<ganhos, custos, o que fica mais difícil>
```
