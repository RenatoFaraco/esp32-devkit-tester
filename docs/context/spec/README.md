# Specs — contratos no fio

Tudo o que passa pelo cabo entre o host e a placa, para todas as implementações.

| Spec | Desde | Status |
|------|-------|--------|
| `report-json.md` | v0.1 | a escrever pelo `architect` quando o ADR-010 for aceito (antes da issue do H8) |
| `result-protocol.md` | v0.2 | a escrever pelo `architect` quando o ADR-005 for aceito |

No fio, a v0.1 não tem spec própria: fala o protocolo do ROM bootloader da Espressif
(documentado pela Espressif e implementado pelo espflash). O `report-json.md` é a exceção da
pasta: não é contrato host ↔ placa, e sim host ↔ quem consome o `--json` (scripts, CI), com
`schema_version` (ADR-010).
