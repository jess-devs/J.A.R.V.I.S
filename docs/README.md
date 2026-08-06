# Documentación de Jarvis

Índice de `docs/`. La instalación y el uso están en el
[`README.md`](../README.md) de la raíz; acá está todo lo demás.

## Si querés usar Jarvis

- [`CONFIGURACION.md`](CONFIGURACION.md) — referencia completa de
  `config.yaml`, clave por clave: qué controla, qué valores acepta y cuándo
  tiene sentido tocarla. Es el **único** lugar donde se documenta la
  configuración, los YAML no llevan comentarios.

## Si querés entender el proyecto

- [`PRODUCT.md`](PRODUCT.md) — qué es Jarvis, para quién, cómo se posiciona y
  con qué principios se decide.
- [`ROADMAP.md`](ROADMAP.md) — qué está realmente terminado, qué es
  experimental, qué es prototipo, y hacia dónde va.
- [`ARCHITECTURE.md`](ARCHITECTURE.md) — cómo está armado por dentro:
  orquestador Rust, workers Python, IPC, flujo de conversación, límites por
  plataforma.
- [`SECURITY.md`](SECURITY.md) — modelo de confianza, niveles de riesgo de las
  tools, reglas de red y qué datos quedan en la máquina.

## Si vas a tocar el código

- [`CONTRIBUTING.md`](CONTRIBUTING.md) — qué leer antes de un cambio, los
  límites entre Rust/Python/React, y los comandos de verificación.
- [`DESIGN.md`](DESIGN.md) — lenguaje visual y reglas de UX de las superficies
  web.
- [`../AGENTS.md`](../AGENTS.md) — las mismas reglas, escritas para agentes de
  código.
- [`../workers/README.md`](../workers/README.md) — protocolo NDJSON, setup del
  venv y flags de debug de los workers Python.
- [`../web/config-ui/README.md`](../web/config-ui/README.md) — frontend
  React/Vite de la página de configuración.

## `planning/`

[`planning/`](planning/) guarda planes de features y el razonamiento detrás de
ellos. **No es fuente de verdad del comportamiento actual** — para eso están los
documentos de arriba. Ver [`planning/README.md`](planning/README.md).
