# ADR-003: Não destrutivo por padrão; v0.1 só com o ROM bootloader
- Status: aceita
- Data: 2026-11-26

## Contexto
Quem testa uma placa pode já ter um firmware nela. Ler eFuses e a flash pelo ROM bootloader
(o código gravado em máscara no silício, que roda quando os pinos de strapping indicam modo
download no reset) não muda nada; gravar um firmware de teste muda.

## Decisão
1. A v0.1 só lê: sincroniza com o ROM, lê registradores, eFuses e o ID da flash.
2. A partir da v0.2, gravar exige confirmação explícita na TUI (ou `--yes` no modo sem TUI).
3. Testes de flash escrevem só na partição `scratch` do firmware de teste.
4. Escrita em eFuse: **nunca** (é irreversível: o eFuse é um fusível, 0 → 1 sem volta).

## Consequências
- A v0.1 já entrega valor (a placa enumera, o ROM responde, flash e PSRAM batem com o
  declarado) sem nenhum firmware.
- Carregar o teste na RAM em vez da flash fica como opção aberta da v0.2.
- Refinado pelo dono (2026-11-26): a partir da v0.2, além de confirmar antes de gravar, o
  testador deixa a flash idêntica à de antes e não deixa nada seu na placa (PRD v0.2, "Requisito
  do dono"). Isso substitui o item 3 ("testes de flash escrevem só na partição `scratch`").
