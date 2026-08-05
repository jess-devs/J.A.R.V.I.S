#!/usr/bin/env bash
# Crea el entorno virtual de los workers Python e instala sus dependencias.
# Requiere Python 3.11 o 3.12 (3.13+ todavía no: PyAudio no publica wheels y
# la compilación desde fuente no está probada ahí).
set -euo pipefail

# El script usa rutas relativas al repo, así que se ancla a su propia
# ubicación: antes solo funcionaba si lo invocabas parado en la raíz, y desde
# cualquier otro directorio fallaba creando el venv en el lugar equivocado (o
# no creándolo, porque `workers/` no existía ahí).
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

VENV_PATH="workers/.venv"
FORCE=false

for arg in "$@"; do
    case "$arg" in
        --force) FORCE=true ;;
        *)
            echo "Uso: $0 [--force]" >&2
            echo "  --force  borra el venv existente y lo crea de nuevo." >&2
            exit 1
            ;;
    esac
done

err() { printf 'ERROR: %s\n' "$1" >&2; }

# Comando para instalar paquetes de sistema, según la distro. Solo se usa
# dentro de mensajes de error.
install_hint() {
    if command -v pacman >/dev/null 2>&1; then
        echo "sudo pacman -S --needed $1"
    elif command -v apt-get >/dev/null 2>&1; then
        echo "sudo apt install $2"
    elif command -v dnf >/dev/null 2>&1; then
        echo "sudo dnf install $3"
    else
        echo "instalá el paquete equivalente a '$1' en tu distro"
    fi
}

# ---------------------------------------------------------------------
# Intérprete
# ---------------------------------------------------------------------

# Se valida la versión ejecutando el intérprete, no por el nombre del binario:
# `python3` puede ser cualquier cosa según la distro, y en varias máquinas el
# 3.12 vive fuera del PATH estándar con otro nombre.
python_version_ok() {
    "$1" -c 'import sys; sys.exit(0 if (3, 11) <= sys.version_info < (3, 13) else 1)' >/dev/null 2>&1
}

PYTHON_BIN="${PYTHON_BIN:-}"

if [[ -n "$PYTHON_BIN" ]]; then
    # Elegido a mano: si no sirve se corta acá en vez de caer en otro
    # intérprete a espaldas del usuario.
    if ! command -v "$PYTHON_BIN" >/dev/null 2>&1; then
        err "PYTHON_BIN='$PYTHON_BIN' no existe o no está en el PATH."
        exit 1
    fi
    if ! python_version_ok "$PYTHON_BIN"; then
        err "PYTHON_BIN='$PYTHON_BIN' es $("$PYTHON_BIN" --version 2>&1), y hace falta Python 3.11 o 3.12."
        echo "       PyAudio (dependencia de RealtimeSTT) no publica wheels para 3.13+." >&2
        exit 1
    fi
else
    for candidate in python3.12 python3.11 python3 python; do
        if command -v "$candidate" >/dev/null 2>&1 && python_version_ok "$candidate"; then
            PYTHON_BIN="$candidate"
            break
        fi
    done
    if [[ -z "$PYTHON_BIN" ]]; then
        err "No se encontró Python 3.11 ni 3.12."
        echo "       Buscados en el PATH: python3.12, python3.11, python3, python." >&2
        if command -v python3 >/dev/null 2>&1; then
            echo "       El python3 del sistema es $(python3 --version 2>&1), que no sirve (PyAudio no tiene wheels para 3.13+)." >&2
        fi
        # Sin `install_hint` a propósito: en Arch los repos oficiales solo
        # traen el Python actual, así que la vía real es AUR o un manejador
        # de versiones, no un `pacman -S` que no existe.
        echo "       Instalá Python 3.12 (Debian/Ubuntu: sudo apt install python3.12; Arch/CachyOS: AUR 'python312', o pyenv/uv)" >&2
        echo "       o seteá PYTHON_BIN=/ruta/al/python3.12 si ya lo tenés fuera del PATH." >&2
        exit 1
    fi
fi

echo "Usando $PYTHON_BIN ($("$PYTHON_BIN" --version 2>&1))."

# ---------------------------------------------------------------------
# Prerrequisitos de compilación (solo Linux)
# ---------------------------------------------------------------------

# PyAudio no publica wheels para Linux (0.2.14 solo trae win32/win_amd64), así
# que pip lo compila desde fuente contra portaudio. Sin estas tres cosas, el
# error real queda enterrado en medio de la salida de pip como un traceback de
# gcc, que es mucho más difícil de accionar que un mensaje acá arriba.
if [[ "$(uname)" == "Linux" ]]; then
    missing=false

    if ! command -v cc >/dev/null 2>&1 && ! command -v gcc >/dev/null 2>&1; then
        err "Falta un compilador de C, necesario para compilar PyAudio."
        echo "       $(install_hint base-devel build-essential 'gcc make')" >&2
        missing=true
    fi

    if ! pkg-config --exists portaudio-2.0 2>/dev/null && [[ ! -f /usr/include/portaudio.h ]]; then
        err "Faltan los headers de portaudio, necesarios para compilar PyAudio."
        echo "       $(install_hint portaudio portaudio19-dev portaudio-devel)" >&2
        missing=true
    fi

    # Los headers del propio intérprete: presentes en las builds standalone y
    # en el paquete `python3-dev`/`python-devel` de las distros.
    include_dir="$("$PYTHON_BIN" -c 'import sysconfig; print(sysconfig.get_paths()["include"])' 2>/dev/null || true)"
    if [[ -z "$include_dir" || ! -f "$include_dir/Python.h" ]]; then
        err "Faltan los headers de Python (Python.h) de $PYTHON_BIN, necesarios para compilar PyAudio."
        echo "       $(install_hint python 'python3-dev' 'python3-devel')" >&2
        missing=true
    fi

    if $missing; then
        exit 1
    fi
fi

# ---------------------------------------------------------------------
# Venv
# ---------------------------------------------------------------------

if $FORCE && [[ -d "$VENV_PATH" ]]; then
    echo "Borrando el venv existente (--force)..."
    rm -rf "$VENV_PATH"
elif [[ -d "$VENV_PATH" && ! -x "$VENV_PATH/bin/python" ]]; then
    # Una corrida anterior que murió a mitad deja un directorio que parece un
    # venv pero no lo es; reusarlo hace fallar el pip de abajo con un error
    # confuso.
    echo "El venv en $VENV_PATH está incompleto; se recrea."
    rm -rf "$VENV_PATH"
fi

echo "Creando venv en $VENV_PATH con $PYTHON_BIN..."
"$PYTHON_BIN" -m venv "$VENV_PATH"

# RealtimeSTT depende de torch y torchaudio sin condiciones, y en Linux la
# build default de PyPI es la de CUDA. Avisarlo antes evita que una descarga
# larga se confunda con un cuelgue.
cat <<'EOF'

Instalando dependencias. La primera vez baja torch y torchaudio, que en Linux
vienen con CUDA por default (~2.5 GB con las librerías nvidia-*). Puede tardar
bastante según tu conexión.

Si no tenés GPU NVIDIA y preferís ahorrarte esa descarga, cortá con Ctrl+C e
instalá antes la variante CPU-only:

    workers/.venv/bin/pip install torch torchaudio --index-url https://download.pytorch.org/whl/cpu

y volvé a correr este script: pip respeta lo que ya está instalado.

EOF

"$VENV_PATH/bin/pip" install --upgrade pip
"$VENV_PATH/bin/pip" install -r workers/requirements.txt

echo "Listo. El default de workers.python_executable usa $VENV_PATH/bin/python en esta plataforma."
