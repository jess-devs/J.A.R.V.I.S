# Verifica prerrequisitos, crea el venv de
# Python, detecta el hardware para recomendar un modelo de Ollama, lo baja,
# descarga la voz de Piper que usa config.yaml y crea el .env si falta.
#
# No instala Rust/Python/Ollama por su cuenta. Si falta alguno, imprime el link y aborta.

param(
    [switch]$Yes
)

$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $false
$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

function Write-Step($msg)
{ Write-Host "`n==> $msg" -ForegroundColor Cyan
}
function Write-Warn($msg)
{ Write-Host "AVISO: $msg" -ForegroundColor Yellow
}
function Write-Err($msg)
{ Write-Host "ERROR: $msg" -ForegroundColor Red
}

function Test-CommandExists($name)
{
    return [bool](Get-Command $name -ErrorAction SilentlyContinue)
}

# Prerrequisitos: solo se verifica
Write-Step "Verificando prerrequisitos (Rust, Python 3.12, Ollama)..."

if (-not (Test-CommandExists "cargo"))
{
    Write-Err "No se encontro 'cargo'. Instala Rust desde https://rustup.rs y volve a correr este script."
    exit 1
}

if (-not (Test-CommandExists "py"))
{
    Write-Err "No se encontro el launcher 'py'. Instala Python 3.12 desde https://www.python.org/downloads/ (marca 'Add to PATH')."
    exit 1
}
py -3.12 --version *> $null
if ($LASTEXITCODE -ne 0)
{
    Write-Err "Python 3.12 no esta instalado para el launcher 'py'. Instalalo desde https://www.python.org/downloads/ (no uses el 3.14 ni el de Microsoft Store: ver workers/README.md)."
    exit 1
}

if (-not (Test-CommandExists "ollama"))
{
    Write-Err "No se encontro 'ollama'. Instalalo desde https://ollama.com y volve a correr este script."
    exit 1
}

Write-Host "OK: cargo, Python 3.12 y ollama estan disponibles."

# Entorno Python:
Write-Step "Creando/actualizando el entorno Python de los workers..."
& "$PSScriptRoot/setup_python_env.ps1"

# Hardware -> modelo de Ollama recomendado
Write-Step "Detectando hardware para recomendar un modelo de Ollama..."

$ramGb = [math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB, 1)
$vramGb = 0.0
$hasGpu = $false
if (Test-CommandExists "nvidia-smi")
{
    try
    {
        $smiOut = (nvidia-smi --query-gpu=memory.total --format=csv,noheader,nounits 2>$null | Select-Object -First 1)
        if ($smiOut)
        {
            $vramGb = [math]::Round([double]($smiOut.Trim()) / 1024, 1)
            $hasGpu = $true
        }
    } catch
    {
        $hasGpu = $false
    }
}

Write-Host "RAM total: $ramGb GB. GPU CUDA: $(if ($hasGpu) { "si, VRAM $vramGb GB" } else { "no detectada" })."

if ($hasGpu -and $vramGb -ge 24)
{
    $recommended = "qwen3:32b"
} elseif ($hasGpu -and $vramGb -ge 16)
{
    $recommended = "qwen3:14b"
} elseif ($hasGpu -and $vramGb -ge 8)
{
    $recommended = "qwen3:8b"
} elseif ($hasGpu -and $vramGb -ge 4)
{
    $recommended = "qwen3.5:0.8b"
} elseif (-not $hasGpu -and $ramGb -ge 16)
{
    $recommended = "qwen2.5:7b"
} else
{
    $recommended = "qwen2.5:3b-instruct"
    if (-not $hasGpu -and $ramGb -lt 8)
    {
        Write-Warn "RAM total por debajo de 8 GB: incluso '$recommended' puede rendir con lentitud."
    }
}

$configPath = Join-Path $RepoRoot "config.yaml"
$configRaw = Get-Content $configPath -Raw
$configNewline = if ($configRaw -match "`r`n")
{ "`r`n"
} else
{ "`n"
}
$configLines = Get-Content $configPath

# El modelo de Ollama vive dentro del bloque "  ollama:" (2 espacios de
# indentacion); hay varias claves "model:" mas abajo (lmstudio/anthropic/etc.)
# asi que hace falta acotar la busqueda a ese bloque especifico.
$ollamaStart = ($configLines | Select-String -Pattern '^\s{2}ollama:\s*$').LineNumber
$currentModel = $null
$modelLineIndex = $null
if ($ollamaStart)
{
    for ($i = $ollamaStart; $i -lt $configLines.Count; $i++)
    {
        if ($configLines[$i] -match '^\s{2}\S')
        { break
        }
        if ($configLines[$i] -match '^\s+model:\s*"([^"]+)"')
        {
            $currentModel = $Matches[1]
            $modelLineIndex = $i
            break
        }
    }
}

if (-not $currentModel)
{
    Write-Warn "No se pudo leer llm.ollama.model de config.yaml; se usa el recomendado ($recommended) solo para el pull, sin tocar el archivo."
    $modelToPull = $recommended
} elseif ($currentModel -eq "auto")
{
    Write-Host "config.yaml ya tiene 'auto': Jarvis elegira este mismo modelo solo al arrancar (y se re-adaptara si cambia el hardware)."
    $modelToPull = $recommended
} else
{
    Write-Host "config.yaml tiene '$currentModel' configurado; segun tu hardware se recomienda '$recommended' (via 'auto', que ademas se re-adapta solo si cambia el hardware)."
    $applyRecommendation = $Yes
    if (-not $Yes)
    {
        $answer = Read-Host "Cambiar config.yaml a 'auto'? [S/n]"
        $applyRecommendation = ($answer -eq "" -or $answer -match '^[sS]')
    }
    if ($applyRecommendation)
    {
        $configLines[$modelLineIndex] = $configLines[$modelLineIndex] -replace '(model:\s*)"[^"]+"', "`$1`"auto`""
        [System.IO.File]::WriteAllText($configPath, ($configLines -join $configNewline) + $configNewline)
        $modelToPull = $recommended
        Write-Host "config.yaml actualizado a 'auto'."
    } else
    {
        $modelToPull = $currentModel
    }
}

Write-Step "Descargando el modelo de Ollama ($modelToPull)..."
ollama pull $modelToPull
if ($LASTEXITCODE -ne 0)
{
    Write-Warn "No se pudo hacer pull de '$modelToPull'. Confirma que Ollama este corriendo ('ollama serve') y volve a intentar con 'ollama pull $modelToPull'."
}

# Voz de Piper
Write-Step "Verificando la voz de Piper configurada..."

$voicePathMatch = $configLines | Select-String -Pattern 'voice_path:\s*"voices/([^"]+)\.onnx"' | Select-Object -First 1
if (-not $voicePathMatch)
{
    Write-Warn "No se pudo leer tts.piper.voice_path de config.yaml; se omite la descarga automatica de voz."
} else
{
    $voiceName = $voicePathMatch.Matches[0].Groups[1].Value
    $onnxPath = Join-Path $RepoRoot "voices/$voiceName.onnx"
    $jsonPath = Join-Path $RepoRoot "voices/$voiceName.onnx.json"

    if ((Test-Path $onnxPath) -and (Test-Path $jsonPath))
    {
        Write-Host "OK: la voz '$voiceName' ya esta en voices/."
    } else
    {
        Write-Host "Descargando la voz '$voiceName'..."
        & "workers/.venv/Scripts/python.exe" -m piper.download_voices $voiceName
        Move-Item -Force "$voiceName.onnx" "voices/$voiceName.onnx" -ErrorAction SilentlyContinue
        Move-Item -Force "$voiceName.onnx.json" "voices/$voiceName.onnx.json" -ErrorAction SilentlyContinue
        if ((Test-Path $onnxPath) -and (Test-Path $jsonPath))
        {
            Write-Host "OK: voz '$voiceName' descargada a voices/."
        } else
        {
            Write-Warn "No se pudo confirmar la descarga de la voz '$voiceName'. Revisa manualmente (ver README.md, seccion 'Voz de Piper')."
        }
    }
}

# .env
Write-Step "Verificando .env..."
$envPath = Join-Path $RepoRoot ".env"
$envExamplePath = Join-Path $RepoRoot ".env.example"
if (Test-Path $envPath)
{
    Write-Host "OK: .env ya existe, no se toca."
} else
{
    Copy-Item $envExamplePath $envPath
    Write-Host "Creado .env a partir de .env.example (solo hace falta completarlo si usas un proveedor en la nube)."
}

Write-Step "Listo."
Write-Host "Compila y corre Jarvis con: cargo run --release"
