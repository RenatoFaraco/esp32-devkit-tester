<#
.SYNOPSIS
    Activates ESP-IDF in the current PowerShell session (dot-source this script).

.DESCRIPTION
    Works around three traps of the stock export.ps1 on Windows (docs/project/SETUP.md):
    - export.ps1 uses the first `python` on PATH to find the IDF virtualenv, so another Python
      (WinPython, Microsoft Store...) makes it look for a venv that does not exist. This script
      points IDF_PYTHON_ENV_PATH at the venv installed by EIM for this IDF version.
    - Processes started from Git Bash carry MSYSTEM, and the IDF refuses to run under MSys/Mingw.
    - activate.py guesses the shell from the parent process and may emit a .bat when PowerShell
      was not started interactively (CI, agents). The shell is passed explicitly here.

.EXAMPLE
    . .\scripts\Enable-Idf.ps1
    idf.py -C firmware -B firmware/build-esp32c6 -D SDKCONFIG=firmware/build-esp32c6/sdkconfig set-target esp32c6 build
#>
[CmdletBinding()]
param(
    [string]$IdfPath = $(if ($env:IDF_PATH) { $env:IDF_PATH } else { 'C:\esp\v5.5.2\esp-idf' }),
    [string]$ToolsPath = $(if ($env:IDF_TOOLS_PATH) { $env:IDF_TOOLS_PATH } else { 'C:\Espressif' })
)

# Dot-sourced: runs in the caller's scope, so no global preference changes here.

if (-not (Test-Path (Join-Path $IdfPath 'tools\idf.py'))) {
    throw "ESP-IDF not found at '$IdfPath'. Pass -IdfPath or set IDF_PATH (see docs/project/SETUP.md)."
}

# IDF version from the tree itself (tools/cmake/version.cmake), e.g. 5.5.2.
$versionFile = Get-Content (Join-Path $IdfPath 'tools\cmake\version.cmake') -Raw -ErrorAction Stop
$parts = foreach ($name in 'MAJOR', 'MINOR', 'PATCH') {
    if ($versionFile -match "IDF_VERSION_$name\s+(\d+)") { $Matches[1] } else { throw "Cannot read IDF_VERSION_$name from version.cmake" }
}
$idfVersion = $parts -join '.'

# Virtualenv installed by EIM for this version: <ToolsPath>\tools\python\v<version>\venv
$venv = Join-Path $ToolsPath "tools\python\v$idfVersion\venv"
if (-not (Test-Path (Join-Path $venv 'Scripts\python.exe'))) {
    throw "Python venv for ESP-IDF $idfVersion not found at '$venv'. Install or repair it with EIM (see docs/project/SETUP.md)."
}

$env:IDF_TOOLS_PATH = $ToolsPath
$env:IDF_PYTHON_ENV_PATH = $venv
Remove-Item Env:MSYSTEM -ErrorAction SilentlyContinue

$exports = & (Join-Path $venv 'Scripts\python.exe') (Join-Path $IdfPath 'tools\activate.py') --export --shell powershell
if ($LASTEXITCODE -ne 0 -or -not $exports -or -not (Test-Path $exports)) {
    throw "activate.py failed (exit $LASTEXITCODE)."
}
. $exports
Write-Host "ESP-IDF $idfVersion active (IDF_PATH=$env:IDF_PATH)"
Remove-Variable versionFile, parts, idfVersion, venv, exports -ErrorAction SilentlyContinue
