# Crea el entorno virtual de los workers Python e instala sus dependencias.
# Requiere Python 3.11 o 3.12 instalado (no 3.14: PyAudio todavía no tiene
# wheel para Windows en esa versión; tampoco el Python de Microsoft Store).

param(
    # Instala nvidia-cublas-cu12/nvidia-cudnn-cu12 (workers/requirements-gpu.txt):
    # las DLLs que ctranslate2 necesita para transcribir en GPU NVIDIA, sin
    # instalar el CUDA Toolkit completo. Apagado por default -- no cambia el
    # comportamiento de este script si no se pide explícitamente. Sin GPU
    # NVIDIA no tiene efecto útil (nvidia-smi u otro chequeo no se hace acá:
    # el pip install simplemente falla si no hay una GPU/driver compatible).
    [switch]$Gpu
)

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

if ($Gpu) {
    Write-Host "Instalando extras GPU (nvidia-cublas-cu12/nvidia-cudnn-cu12) para acelerar el motor nativo en CUDA..."
    & "$venvPath/Scripts/pip.exe" install -r workers/requirements-gpu.txt
}

Write-Host "Listo. El default de workers.python_executable usa $venvPath/Scripts/python.exe en esta plataforma."
