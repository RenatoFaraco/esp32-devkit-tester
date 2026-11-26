# Ambiente de desenvolvimento — Windows + PowerShell

> Referência: Windows 11 com **PowerShell** (ADR-007). Todos os comandos rodam na **raiz do
> repositório**. Ao terminar, um PowerShell novo compila o host e o firmware só com isto.

## 1. Ferramentas

```powershell
winget install --id Git.Git -e
# Linker da Microsoft (link.exe): o Rust no Windows (alvo x86_64-pc-windows-msvc) linka com ele.
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
winget install --id Rustlang.Rustup -e
```

Feche e abra o terminal depois de instalar: cada terminal lê o `PATH` só quando abre. No VS Code
isso vale para o editor inteiro (feche e abra o VS Code, não só a aba do terminal).

A versão do Rust não precisa ser escolhida: `host/rust-toolchain.toml` pede a stable com
`rustfmt` e `clippy`, e o `rustup` instala sozinho na primeira vez que o `cargo` roda em `host/`.
Versão mínima suportada (MSRV): 1.88.

## 2. Smart App Control

O Windows 11 pode vir com o **Smart App Control** ligado. Ele só deixa rodar executáveis
assinados ou com reputação na nuvem da Microsoft — e todo `.exe` que o `cargo` gera (build
scripts das dependências, binários de teste, o próprio `devkit-tester.exe`) é novo e sem
assinatura. Sintoma: o build falha com `os error 4551`, e o log
*Microsoft-Windows-CodeIntegrity/Operational* mostra o evento 3077.

Não existe exceção por pasta. Para desenvolver é preciso desligá-lo em **Segurança do Windows →
Controle de aplicativos e do navegador → Smart App Control → Desativado**. Em muitas versões do
Windows 11 ele só volta a ligar reinstalando o sistema. Continuam ativos: antivírus do Defender
em tempo real, proteção na nuvem, proteção contra violação, SmartScreen e UAC.

## 3. Host (Rust)

```powershell
cargo run --manifest-path host/Cargo.toml          # abre a TUI; q sai

# O que o CI roda (Windows e Linux):
cargo fmt --manifest-path host/Cargo.toml --all -- --check
cargo clippy --manifest-path host/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path host/Cargo.toml
```

## 4. Firmware (ESP-IDF v5.5.2)

### Instalar
Use o instalador oficial da Espressif, o **EIM** (ESP-IDF Installation Manager), e escolha a
versão **v5.5.2** — a mesma que o `firmware/CMakeLists.txt` exige e que o CI usa
(imagem `espressif/idf:v5.5.2`). Guia oficial:
<https://docs.espressif.com/projects/esp-idf/en/v5.5.2/esp32/get-started/windows-setup.html>.

Caminhos padrão do EIM, que o script abaixo assume:
- ESP-IDF: `C:\esp\v5.5.2\esp-idf`
- Ferramentas (toolchains, Python): `C:\Espressif`

### Ativar no terminal
```powershell
. .\scripts\Enable-Idf.ps1
# outros caminhos: . .\scripts\Enable-Idf.ps1 -IdfPath D:\esp\v5.5.2\esp-idf -ToolsPath D:\Espressif
```
O ponto e o espaço no começo (*dot-source*) são necessários: o script altera o ambiente da sessão
atual. Ele faz o mesmo que o `export.ps1` do IDF, mas contorna os problemas da seção 5.

### Compilar
Um diretório de build **e** um `sdkconfig` por alvo — o `set-target` reescreve o `sdkconfig`, e
dois alvos no mesmo diretório se atropelam:
```powershell
idf.py -C firmware -B firmware/build-esp32c6 -D SDKCONFIG=build-esp32c6/sdkconfig set-target esp32c6 build
```
Alvos da bancada: `esp32`, `esp32c3`, `esp32c6`, `esp32s3` (C5 e P4 entram no bring-up).
Atenção à assimetria: com `-C firmware`, o `-B` é relativo ao diretório atual, mas o
`SDKCONFIG` é relativo ao projeto (`firmware/`).

Gravar e ver a saída numa placa (opcional; troque a porta pela que aparece no Gerenciador de
Dispositivos — ela muda de PC para PC):
```powershell
idf.py -C firmware -B firmware/build-esp32c6 -p COM5 flash monitor     # Ctrl+] sai do monitor
```

## 5. Problemas conhecidos

| Sintoma | Causa | Solução |
|---------|-------|---------|
| `ESP-IDF Python virtual environment "...idf5.5_py3.12_env..." not found` | O `export.ps1` usa o primeiro `python` do PATH (WinPython, Microsoft Store…) para achar o venv, mas o EIM criou o venv com o Python dele | Usar o `Enable-Idf.ps1`, que aponta `IDF_PYTHON_ENV_PATH` para `C:\Espressif\tools\python\v5.5.2\venv` |
| `MSys/Mingw is not supported` | Processo iniciado a partir do Git Bash herda `MSYSTEM` | `Enable-Idf.ps1` remove `MSYSTEM`; ou use o PowerShell direto |
| `O termo 'call ...activate_*.bat' não é reconhecido` | O `activate.py` deduz o shell pelo processo pai e, fora de um PowerShell interativo (CI, agentes), gera `.bat` | `Enable-Idf.ps1` passa `--shell powershell` |
| `fatal: bad config line 2 ... micro-ecc/config` e `IDF_VER` = `-128-NOTFOUND` no boot | Configuração git corrompida num submódulo da instalação do IDF | Cosmético (o binário é idêntico ao do CI). Para corrigir, reparar a instalação pelo EIM |
| `cargo` não encontrado no terminal do VS Code / Git Bash | Terminal aberto antes da instalação do Rust | Fechar e abrir o VS Code |
| Build do Rust falha com `os error 4551` | Smart App Control | Seção 2 |
