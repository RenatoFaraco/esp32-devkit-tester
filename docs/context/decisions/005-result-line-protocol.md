# ADR-005: Resultados dos testes na placa em linhas ASCII próprias, sobre Unity
- Status: proposta (fechar no início da v0.2)
- Data: 2026-11-26

## Contexto
Os testes on-target (v0.2) usam Unity, que imprime `arquivo:linha:teste:PASS|FAIL:msg`. O host
precisa de mais que isso — suíte, duração, SKIP com motivo, versão do firmware, fim da execução
— e precisa separar os resultados do log do ROM e do IDF que saem na mesma serial.

## Proposta
Linhas com prefixo fixo e versão, uma por evento, ASCII imprimível, ex.:
```
@DKT1 begin chip=esp32c6 fw=0.2.0
@DKT1 test memory/sram march_c ok 88
@DKT1 test memory/psram detect skip 0 no PSRAM in eFuse
@DKT1 end passed=5 failed=0 skipped=1
```
Unity continua sendo o framework de asserções; um emissor (`dkt_report`) traduz os eventos.
Gramática exata, limites e o que o host faz com linha inválida → `spec/result-protocol.md`.

## Alternativas a avaliar
- Fazer parse da saída padrão do Unity (sem duração nem SKIP com motivo).
- JSON por linha (mais fácil de estender, mais caro de validar no firmware).
- `pytest-embedded` da Espressif (exige Python no PC; vai contra o binário único).
