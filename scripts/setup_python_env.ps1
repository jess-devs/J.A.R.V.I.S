# Crea el entorno virtual de los workers Python e instala sus dependencias.
# Requiere Python 3.11 o 3.12 instalado (no 3.14: PyAudio todavía no tiene
# wheel para Windows en esa versión; tampoco el Python de Microsoft Store).

$ErrorActionPreference = "Stop"

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

Write-Host "Listo. config.yaml ya apunta a $venvPath/Scripts/python.exe por defecto."
