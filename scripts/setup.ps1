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
    $recommended = "qwen3.5:4b"
} elseif (-not $hasGpu -and $ramGb -ge 16)
{
    $recommended = "qwen3.5:4b"
} else
{
    $recommended = "qwen3.5:0.8b"
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

# Modelo de reconocimiento de voz (Whisper small + Silero VAD)
Write-Step "Verificando el modelo de reconocimiento de voz..."

$sttModelDir = Join-Path $RepoRoot "models/stt/sherpa-onnx-whisper-small"
$sttVadPath = Join-Path $RepoRoot "models/stt/silero_vad.onnx"
$sttModelFiles = @("small-encoder.onnx", "small-decoder.int8.onnx", "small-tokens.txt")
$sttModelComplete = (Test-Path $sttModelDir) -and -not ($sttModelFiles | Where-Object { -not (Test-Path (Join-Path $sttModelDir $_)) })
$sttVadComplete = Test-Path $sttVadPath

if ($sttModelComplete -and $sttVadComplete)
{
    Write-Host "OK: el modelo de reconocimiento de voz ya esta en models/stt/."
} else
{
    $downloadStt = $Yes
    if (-not $Yes)
    {
        $answer = Read-Host "Descargar el modelo de reconocimiento de voz (~640MB)? [S/n]"
        $downloadStt = ($answer -eq "" -or $answer -match '^[sS]')
    }
    if ($downloadStt)
    {
        New-Item -ItemType Directory -Force (Join-Path $RepoRoot "models/stt") | Out-Null
        # Invoke-WebRequest con la barra de progreso por defecto es muchisimo
        # mas lenta para archivos grandes en PowerShell 5/7 sobre Windows.
        $prevProgressPreference = $ProgressPreference
        $ProgressPreference = "SilentlyContinue"
        try
        {
            if (-not $sttVadComplete)
            {
                Write-Host "Descargando silero_vad.onnx..."
                Invoke-WebRequest -Uri "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx" -OutFile $sttVadPath
            }

            if (-not $sttModelComplete)
            {
                Write-Host "Descargando Whisper small (~640MB comprimido, puede tardar unos minutos)..."
                $tarPath = Join-Path $RepoRoot "models/stt/sherpa-onnx-whisper-small.tar.bz2"
                Invoke-WebRequest -Uri "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-whisper-small.tar.bz2" -OutFile $tarPath
                tar -xf $tarPath -C (Join-Path $RepoRoot "models/stt")
                Remove-Item $tarPath -Force -ErrorAction SilentlyContinue
                # El tarball trae ademas small-encoder.int8.onnx y
                # small-decoder.onnx (las variantes que NO usamos: preferimos
                # encoder fp32 + decoder int8, mejor precision/velocidad que
                # cuantizar el encoder) y una carpeta test_wavs vacia -- se
                # descartan para no duplicar ~300MB sin uso.
                Remove-Item (Join-Path $sttModelDir "small-encoder.int8.onnx") -Force -ErrorAction SilentlyContinue
                Remove-Item (Join-Path $sttModelDir "small-decoder.onnx") -Force -ErrorAction SilentlyContinue
                Remove-Item (Join-Path $sttModelDir "test_wavs") -Recurse -Force -ErrorAction SilentlyContinue
            }
        } finally
        {
            $ProgressPreference = $prevProgressPreference
        }

        $sttModelComplete = (Test-Path $sttModelDir) -and -not ($sttModelFiles | Where-Object { -not (Test-Path (Join-Path $sttModelDir $_)) })
        if ($sttModelComplete -and (Test-Path $sttVadPath))
        {
            Write-Host "OK: modelo de reconocimiento de voz listo en models/stt/."
        } else
        {
            Write-Warn "No se pudo confirmar la descarga del modelo de reconocimiento de voz. Revisa manualmente (ver README.md, seccion 'Modelo de reconocimiento de voz')."
        }
    } else
    {
        Write-Warn "Sin el modelo de reconocimiento de voz, Jarvis no va a poder arrancar el STT. Volve a correr este script cuando quieras descargarlo."
    }
}

Write-Step "Descargando el modelo de Ollama ($modelToPull)..."

# El pull necesita el servidor de Ollama corriendo, no solo el binario en
# PATH. Si no responde, se intenta levantarlo en segundo plano (queda
# corriendo tras el setup, que es lo deseable: el paso siguiente del usuario
# es arrancar Jarvis).
function Test-OllamaServer
{
    try
    {
        Invoke-RestMethod "http://127.0.0.1:11434/api/version" -TimeoutSec 2 | Out-Null
        return $true
    } catch
    {
        return $false
    }
}

if (-not (Test-OllamaServer))
{
    Write-Host "El servidor de Ollama no responde; levantando 'ollama serve' en segundo plano..."
    Start-Process ollama -ArgumentList "serve" -WindowStyle Hidden
    $deadline = (Get-Date).AddSeconds(15)
    while (-not (Test-OllamaServer) -and (Get-Date) -lt $deadline)
    {
        Start-Sleep -Milliseconds 500
    }
    if (-not (Test-OllamaServer))
    {
        Write-Warn "No se pudo levantar el servidor de Ollama. Arrancalo a mano ('ollama serve' o la app de Ollama)."
    }
}

ollama pull $modelToPull
if ($LASTEXITCODE -ne 0)
{
    Write-Warn "No se pudo hacer pull de '$modelToPull'. Confirma que Ollama este corriendo ('ollama serve') y volve a intentar con 'ollama pull $modelToPull'."
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
