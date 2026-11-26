# ADR-001: Firmware de teste em C com ESP-IDF puro (`idf.py`), sem PlatformIO
- Status: proposta (confirmar no M0)
- Data: 2026-11-26

## Contexto
O firmware de teste (v0.2+) precisa compilar para sete alvos — ESP32, C3, C5, C6, S3, P4 e H2 —
inclusive os mais novos (C5, P4 rev3, H2), e rodar testes Unity na placa. No esp32-logcat a
escolha foi PlatformIO (ADR-001 de lá), que funcionou bem para um alvo só (C3).

## Decisão
- Linguagem **C** (C17): é a linguagem do ESP-IDF e dos exemplos da Espressif; o firmware é
  pequeno e não precisa das abstrações do C++.
- Build com **ESP-IDF puro** (`idf.py`, CMake): `set-target` por chip, `sdkconfig.defaults` +
  `sdkconfig.defaults.<target>`, componentes em `components/`, Unity que já vem no IDF.
- Matriz de alvos no CI com a imagem Docker oficial `espressif/idf` na versão fixada.

## Opções descartadas
- **PlatformIO:** o suporte a chips novos chega com atraso em relação ao IDF e uma matriz de 7
  envs repete o que o `set-target` já faz. Volta a ser opção se, no M0, todos os 7 alvos
  compilarem na versão do IDF que ele empacota.
- **Arduino:** esconde exatamente o que queremos testar (memória, cache, eFuse).

## Consequências
- Ambiente: ESP-IDF instalado no Windows (`~/.espressif`) e ativado no terminal; o M0 fixa a
  versão e escreve o comando exato no `CLAUDE.md` e no `test-runner`.
- Um binário por chip: o host escolhe qual gravar pelo chip detectado (v0.2).
- Diferença em relação ao esp32-logcat, registrada para não parecer inconsistência.
