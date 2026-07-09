#!/usr/bin/env bash
# Crea el entorno virtual de los workers Python e instala sus dependencias.
# Requiere Python 3.11 o 3.12 instalado.
set -euo pipefail

PYTHON_BIN="${PYTHON_BIN:-python3.12}"
VENV_PATH="workers/.venv"

if ! command -v "$PYTHON_BIN" >/dev/null 2>&1; then
    echo "No se encontró '$PYTHON_BIN'. Instalá Python 3.11 o 3.12, o seteá PYTHON_BIN." >&2
    exit 1
fi

echo "Creando venv en $VENV_PATH con $PYTHON_BIN..."
"$PYTHON_BIN" -m venv "$VENV_PATH"

echo "Instalando dependencias..."
"$VENV_PATH/bin/pip" install --upgrade pip
"$VENV_PATH/bin/pip" install -r workers/requirements.txt

echo "Listo. config.yaml necesita workers.python_executable: \"$VENV_PATH/bin/python\""
