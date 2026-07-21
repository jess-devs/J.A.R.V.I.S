#!/usr/bin/env bash

set -euo pipefail

ASSUME_YES=false
while getopts "y" opt; do
    case "$opt" in
        y) ASSUME_YES=true ;;
    esac
done

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

PYTHON_BIN="${PYTHON_BIN:-python3.12}"

step() { printf '\n==> %s\n' "$1"; }
warn() { printf 'AVISO: %s\n' "$1" >&2; }
err() { printf 'ERROR: %s\n' "$1" >&2; }

# Prerrequisitos: solo se verifica
step "Verificando prerrequisitos (Rust, Python 3.12, Ollama)..."

if ! command -v cargo >/dev/null 2>&1; then
    err "No se encontro 'cargo'. Instala Rust desde https://rustup.rs y volve a correr este script."
    exit 1
fi

if ! command -v "$PYTHON_BIN" >/dev/null 2>&1; then
    err "No se encontro '$PYTHON_BIN'. Instala Python 3.11/3.12 desde https://www.python.org/downloads/ (o seteá PYTHON_BIN)."
    exit 1
fi

if ! command -v ollama >/dev/null 2>&1; then
    err "No se encontro 'ollama'. Instalalo desde https://ollama.com y volve a correr este script."
    exit 1
fi

echo "OK: cargo, $PYTHON_BIN y ollama estan disponibles."

# Entorno Python
step "Creando/actualizando el entorno Python de los workers..."
"$REPO_ROOT/scripts/setup_python_env.sh"

# Hardware -> modelo de Ollama recomendado
step "Detectando hardware para recomendar un modelo de Ollama..."

ram_gb=0
if [[ "$(uname)" == "Darwin" ]]; then
    ram_bytes=$(sysctl -n hw.memsize 2>/dev/null || echo 0)
    ram_gb=$((ram_bytes / 1024 / 1024 / 1024))
else
    ram_gb=$(free -g 2>/dev/null | awk '/^Mem:/ { print $2 }')
    ram_gb=${ram_gb:-0}
fi

has_gpu=false
vram_gb=0
if command -v nvidia-smi >/dev/null 2>&1; then
    vram_mib=$(nvidia-smi --query-gpu=memory.total --format=csv,noheader,nounits 2>/dev/null | head -n1)
    if [[ -n "${vram_mib:-}" ]]; then
        vram_gb=$((vram_mib / 1024))
        has_gpu=true
    fi
fi

if $has_gpu; then
    echo "RAM total: ${ram_gb} GB. GPU CUDA: si, VRAM ${vram_gb} GB."
else
    echo "RAM total: ${ram_gb} GB. GPU CUDA: no detectada."
fi

if $has_gpu && [[ "$vram_gb" -ge 24 ]]; then
    recommended="qwen3:32b"
elif $has_gpu && [[ "$vram_gb" -ge 16 ]]; then
    recommended="qwen3:14b"
elif $has_gpu && [[ "$vram_gb" -ge 8 ]]; then
    recommended="qwen3:8b"
elif $has_gpu && [[ "$vram_gb" -ge 5 ]]; then
    # Margen de seguridad: una GPU de 4GB "nominales" casi nunca tiene esos
    # 4GB libres de verdad (compositor, otros procesos), así que el modelo
    # de 4B rara vez entra entero. Debe coincidir con model_select.rs.
    recommended="qwen3.5:4b"
elif ! $has_gpu && [[ "$ram_gb" -ge 16 ]]; then
    recommended="qwen3.5:4b"
else
    recommended="qwen3.5:0.8b"
    if ! $has_gpu && [[ "$ram_gb" -lt 8 ]]; then
        warn "RAM total por debajo de 8 GB: incluso '$recommended' puede rendir con lentitud."
    fi
fi

current_model=$(awk '
    /^  ollama:[[:space:]]*$/ { in_block=1; next }
    in_block && /^  [^ ]/ { in_block=0 }
    in_block && /model:/ && !done {
        line=$0
        sub(/.*model:[ \t]*"/, "", line)
        sub(/".*/, "", line)
        print line
        done=1
        exit
    }
' config.yaml)

if [[ -z "$current_model" ]]; then
    warn "No se pudo leer llm.ollama.model de config.yaml; se usa el recomendado ($recommended) solo para el pull, sin tocar el archivo."
    model_to_pull="$recommended"
elif [[ "$current_model" == "auto" ]]; then
    echo "config.yaml ya tiene 'auto': Jarvis elegira este mismo modelo solo al arrancar (y se re-adaptara si cambia el hardware)."
    model_to_pull="$recommended"
else
    echo "config.yaml tiene '$current_model' configurado; segun tu hardware se recomienda '$recommended' (via 'auto', que ademas se re-adapta solo si cambia el hardware)."
    apply_recommendation=$ASSUME_YES
    if ! $ASSUME_YES; then
        read -r -p "Cambiar config.yaml a 'auto'? [S/n] " answer
        [[ -z "$answer" || "$answer" =~ ^[sS] ]] && apply_recommendation=true || apply_recommendation=false
    fi
    if $apply_recommendation; then
        tmp_file=$(mktemp)
        awk -v new_model="auto" '
            /^  ollama:[[:space:]]*$/ { in_block=1; print; next }
            in_block && /^  [^ ]/ { in_block=0 }
            in_block && /model:/ && !done {
                line=$0
                sub(/model:[ \t]*"[^"]*"/, "model: \"" new_model "\"", line)
                print line
                done=1
                next
            }
            { print }
        ' config.yaml > "$tmp_file"
        mv "$tmp_file" config.yaml
        model_to_pull="$recommended"
        echo "config.yaml actualizado a 'auto'."
    else
        model_to_pull="$current_model"
    fi
fi

# Voz de Piper
step "Verificando la voz de Piper configurada..."

voice_name=$(sed -n 's/.*voice_path:[ \t]*"voices\/\([^"]*\)\.onnx".*/\1/p' config.yaml | head -n1)

if [[ -z "$voice_name" ]]; then
    warn "No se pudo leer tts.piper.voice_path de config.yaml; se omite la descarga automatica de voz."
else
    onnx_path="voices/${voice_name}.onnx"
    json_path="voices/${voice_name}.onnx.json"
    if [[ -f "$onnx_path" && -f "$json_path" ]]; then
        echo "OK: la voz '$voice_name' ya esta en voices/."
    else
        echo "Descargando la voz '$voice_name'..."
        "$REPO_ROOT/workers/.venv/bin/python" -m piper.download_voices "$voice_name"
        mv -f "${voice_name}.onnx" "$onnx_path" 2>/dev/null || true
        mv -f "${voice_name}.onnx.json" "$json_path" 2>/dev/null || true
        if [[ -f "$onnx_path" && -f "$json_path" ]]; then
            echo "OK: voz '$voice_name' descargada a voices/."
        else
            warn "No se pudo confirmar la descarga de la voz '$voice_name'. Revisa manualmente (ver README.md, seccion 'Voz de Piper')."
        fi
    fi
fi

# Modelo de reconocimiento de voz (Whisper small + Silero VAD)
step "Verificando el modelo de reconocimiento de voz..."

stt_model_dir="$REPO_ROOT/models/stt/sherpa-onnx-whisper-small"
stt_vad_path="$REPO_ROOT/models/stt/silero_vad.onnx"

stt_model_complete() {
    [[ -f "$stt_model_dir/small-encoder.onnx" && -f "$stt_model_dir/small-decoder.int8.onnx" \
        && -f "$stt_model_dir/small-tokens.txt" ]]
}

if stt_model_complete && [[ -f "$stt_vad_path" ]]; then
    echo "OK: el modelo de reconocimiento de voz ya esta en models/stt/."
else
    download_stt=$ASSUME_YES
    if ! $ASSUME_YES; then
        read -r -p "Descargar el modelo de reconocimiento de voz (~640MB)? [S/n] " answer
        [[ -z "$answer" || "$answer" =~ ^[sS] ]] && download_stt=true || download_stt=false
    fi
    if $download_stt; then
        mkdir -p "$REPO_ROOT/models/stt"

        if [[ ! -f "$stt_vad_path" ]]; then
            echo "Descargando silero_vad.onnx..."
            curl -sL -o "$stt_vad_path" "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx"
        fi

        if ! stt_model_complete; then
            echo "Descargando Whisper small (~640MB comprimido, puede tardar unos minutos)..."
            tar_path="$REPO_ROOT/models/stt/sherpa-onnx-whisper-small.tar.bz2"
            curl -sL -o "$tar_path" "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-whisper-small.tar.bz2"
            tar -xjf "$tar_path" -C "$REPO_ROOT/models/stt"
            rm -f "$tar_path"
            # El tarball trae ademas small-encoder.int8.onnx y
            # small-decoder.onnx (variantes que no usamos: preferimos encoder
            # fp32 + decoder int8) y una carpeta test_wavs vacia -- se
            # descartan para no duplicar ~300MB sin uso.
            rm -f "$stt_model_dir/small-encoder.int8.onnx" "$stt_model_dir/small-decoder.onnx"
            rm -rf "$stt_model_dir/test_wavs"
        fi

        if stt_model_complete && [[ -f "$stt_vad_path" ]]; then
            echo "OK: modelo de reconocimiento de voz listo en models/stt/."
        else
            warn "No se pudo confirmar la descarga del modelo de reconocimiento de voz. Revisa manualmente (ver README.md, seccion 'Modelo de reconocimiento de voz')."
        fi
    else
        warn "Sin el modelo de reconocimiento de voz, Jarvis no va a poder arrancar el STT. Volve a correr este script cuando quieras descargarlo."
    fi
fi

step "Descargando el modelo de Ollama ($model_to_pull)..."

# El pull necesita el servidor de Ollama corriendo, no solo el binario en
# PATH. Si no responde, se intenta levantarlo en segundo plano (queda
# corriendo tras el setup, que es lo deseable: el paso siguiente del usuario
# es arrancar Jarvis).
ollama_server_up() {
    curl -sf --max-time 2 "http://127.0.0.1:11434/api/version" >/dev/null 2>&1
}

if ! ollama_server_up; then
    echo "El servidor de Ollama no responde; levantando 'ollama serve' en segundo plano..."
    nohup ollama serve >/dev/null 2>&1 &
    deadline=$((SECONDS + 15))
    until ollama_server_up || [[ $SECONDS -ge $deadline ]]; do
        sleep 0.5
    done
    if ! ollama_server_up; then
        warn "No se pudo levantar el servidor de Ollama. Arrancalo a mano ('ollama serve')."
    fi
fi

if ! ollama pull "$model_to_pull"; then
    warn "No se pudo hacer pull de '$model_to_pull'. Confirma que Ollama este corriendo ('ollama serve') y volve a intentar con 'ollama pull $model_to_pull'."
fi

# .env
step "Verificando .env..."
if [[ -f ".env" ]]; then
    echo "OK: .env ya existe, no se toca."
else
    cp ".env.example" ".env"
    echo "Creado .env a partir de .env.example (solo hace falta completarlo si usas un proveedor en la nube)."
fi

step "Listo."
echo "Compila y corre Jarvis con: cargo run --release"
