# Specs — contratos no fio

Tudo o que passa pelo cabo entre o host e a placa, para todas as implementações.

| Spec | Desde | Status |
|------|-------|--------|
| [`report-json.md`](report-json.md) | v0.1 | **corrente** — schema v1 do `--json` (ADR-010, ainda proposta; se mudar ao ser aceito, a spec muda junto) |
| `result-protocol.md` | v0.2 | a escrever pelo `architect` quando o ADR-005 for aceito |

No fio, a v0.1 não tem spec própria: fala o protocolo do ROM bootloader da Espressif
(documentado pela Espressif e implementado pelo espflash). O `report-json.md` é a exceção da
pasta: não é contrato host ↔ placa, e sim host ↔ quem consome o `--json` (scripts, CI), com
`schema_version` (ADR-010).
