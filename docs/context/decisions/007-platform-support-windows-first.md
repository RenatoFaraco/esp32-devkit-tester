# ADR-007: Plataformas — Windows (PowerShell) primeiro, depois WSL2, depois Linux/macOS
- Status: aceita
- Data: 2026-11-26

## Contexto
A bancada é um PC Windows, e as placas aparecem como portas COM. O Renato também usa o WSL para
outros projetos e quer o testador lá no futuro; outros sistemas virão depois. O USB no WSL2 não é
nativo: a placa precisa ser repassada do Windows com `usbipd-win` (`usbipd bind` / `attach`), e
aparece como `/dev/ttyACM*` (USB-Serial/JTAG) ou `/dev/ttyUSB*` (pontes). O USB-Serial/JTAG
nativo é um periférico **dentro do chip**: quando o chip reseta, o dispositivo USB some do
barramento e volta, e no WSL2 isso derruba o repasse do usbipd. O próprio espflash documenta que
não grava pelo USB-Serial/JTAG no WSL2 (pela UART funciona) e que não suporta WSL1.

## Decisão
1. **Fase 1 — Windows nativo, PowerShell.** Plataforma de referência até a v1.0: todos os
   critérios de aceite são verificados nela. Documentação de uso e comandos em PowerShell.
2. **Fase 2 — WSL2** (depois da v1.0; versão a definir no ROADMAP). Requisitos já conhecidos:
   - `usbipd-win` com `--auto-attach`, documentado passo a passo;
   - tolerar a porta sumir e voltar com outro nome (`ttyACM0` → `ttyACM1`) a cada reset;
   - pela porta nativa, testes que dependem de reset viram SKIP com motivo, ou o host orienta a
     usar a ponte UART; JTAG pelo USB nativo a validar;
   - WSL1: não suportado (mensagem clara ao detectar).
3. **Fase 3 — Linux nativo e macOS** (depois). Linux nativo tende a ser o mais simples
   (`/dev/ttyACM*`, regras `udev` para permissão de grupo `dialout`/`plugdev`).
4. **Desde já, o código não pode impedir as fases 2 e 3:**
   - nada de API só-Windows fora de um módulo `platform` com `#[cfg(...)]`;
   - portas identificadas por VID:PID/número de série (o crate `serialport` dá isso nos três
     sistemas), nunca pelo nome (`COMx`, `ttyACMx`);
   - caminhos com `std::path`/`dirs`, nunca `\` literal; nada de depender de PowerShell no
     binário (PowerShell é só para a documentação e scripts de desenvolvimento);
   - o CI compila e roda os testes do host em **Windows e Linux** desde o M0, mesmo sem
     suporte oficial ao Linux, para pegar código não portátil cedo.
   - detecção de WSL já registrada como ponto de extensão, sem comportamento especial na fase 1:
     `/proc/sys/kernel/osrelease` contendo `microsoft` → WSL; com `WSL2` ou `microsoft-standard`
     → WSL2, senão WSL1 (implementada no M0 em `host/src/platform.rs`, com as fontes).

## Consequências
- O primeiro usuário (Windows) tem a melhor experiência; os outros sistemas chegam sem
  reescrita, só com o módulo `platform` e documentação.
- A reconexão robusta (porta que some e volta) precisa existir já na fase 1 (o Windows também
  renumera COMs), e é a base do suporte ao WSL2.
- Firmware não é afetado: o build do ESP-IDF roda em qualquer um dos três sistemas.
