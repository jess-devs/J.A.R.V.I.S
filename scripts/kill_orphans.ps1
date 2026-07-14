# Lista o termina procesos python.exe huerfanos de los workers de Jarvis
# (stt_worker.py / tts_worker.py). Red de seguridad manual para desarrollo/
# pruebas: el Job Object de Jarvis deberia evitar que esto pase, pero este
# script sirve para limpiar procesos que hayan quedado de antes de ese fix,
# o de cualquier otra causa inesperada.

param(
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"

function Write-Step($msg)
{ Write-Host "`n==> $msg" -ForegroundColor Cyan
}
function Write-Warn($msg)
{ Write-Host "AVISO: $msg" -ForegroundColor Yellow
}

Write-Step "Buscando procesos python.exe de workers de Jarvis..."

$targets = Get-CimInstance Win32_Process -Filter "Name = 'python.exe'" |
    Where-Object { $_.CommandLine -match 'stt_worker\.py|tts_worker\.py' }

if (-not $targets)
{
    Write-Host "No se encontraron procesos huerfanos." -ForegroundColor Green
    exit 0
}

foreach ($p in $targets)
{
    $line = "PID $($p.ProcessId): $($p.CommandLine)"
    if ($DryRun)
    {
        Write-Host "[DryRun] $line"
    } else
    {
        Write-Warn "Terminando $line"
        Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue
    }
}

if (-not $DryRun)
{
    Write-Host "`nListo." -ForegroundColor Green
}
