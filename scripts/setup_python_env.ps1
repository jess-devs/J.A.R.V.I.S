# Crea el entorno virtual de los workers Python e instala sus dependencias.
# Requiere Python 3.11 o 3.12 instalado (no 3.14: PyAudio todavía no tiene
# wheel para Windows en esa versión; tampoco el Python de Microsoft Store).

$ErrorActionPreference = "Stop"

# Las rutas de abajo son relativas al repo, así que el script se ancla a su
# propia ubicación en vez de depender del directorio desde el que lo invocaron.
$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

$pythonVersion = "3.12"
$venvPath = "workers/.venv"

if (-not (Get-Command py -ErrorAction SilentlyContinue)) {
    Write-Error "No se encontró el launcher 'py'. Instalá Python $pythonVersion desde python.org."
    exit 1
}

Write-Host "Creando venv en $venvPath con Python $pythonVersion..."
py "-$pythonVersion" -m venv $venvPath

Write-Host "Instalando dependencias..."
& "$venvPath/Scripts/pip.exe" install --upgrade pip
& "$venvPath/Scripts/pip.exe" install -r workers/requirements.txt

Write-Host "Listo. El default de workers.python_executable usa $venvPath/Scripts/python.exe en esta plataforma."
