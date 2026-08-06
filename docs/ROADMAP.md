# Roadmap De Jarvis

La intención del producto está en [`PRODUCT.md`](PRODUCT.md), la estructura
técnica en [`ARCHITECTURE.md`](ARCHITECTURE.md) y las restricciones de
seguridad en [`SECURITY.md`](SECURITY.md).

## Estado Actual

*Última revisión: 2026-08-05.* La clasificación de abajo envejece con cada
cambio de comportamiento: si no coincide con lo que hace el código, vale el
código, y hay que actualizar esta sección junto al cambio.

### Producto Real

- STT local con motor nativo y camino de respaldo.
- TTS local con Piper y proveedores configurables.
- Conversación en español con streaming.
- Memoria y recordatorios locales.
- Modo texto para debugging y ambientes donde no se puede hablar.
- Auditoría local de acciones de riesgo.

### Experimental

- Wake word y gate de atención.
- Silencio explícito y reactivación por voz.
- Barge-in y conversación fluida durante TTS.
- Tools agénticas y cliente MCP.
- Verificación de hablante.
- Modo bienvenida por doble aplauso, dependiente de calibración de audio.

### Prototipo

- Página local de configuración React/Axum.
- Wizard y calibración visual de micrófono.
- Enrollment de voz desde la web.

### Eliminado

- TUI de terminal basada en Ratatui.

## Próximas Áreas

### Background Y Autostart

Ejecutar Jarvis al iniciar el equipo y mantenerlo residente sin depender de una
terminal abierta. Requiere decisiones específicas por plataforma, permisos,
actualización y apagado seguro.

### Wake Word Acústico

Evaluar detección acústica previa a Whisper para reducir activaciones falsas y
latencia. Debe conservar una UX clara, respetar el modo silencio y revisar
licencias, consumo y diferencias entre un modelo genérico y una voz personal.

### Aplicación De Escritorio

Evaluar un empaquetado nativo multiplataforma sin reintroducir la TUI. La página
local de configuración puede ser una base, pero el proceso de selección debe
considerar permisos de micrófono, ciclo de vida, actualización y aislamiento.

### Integraciones

Expandir MCP y tools con integraciones curadas, manteniendo autenticación
explícita, clasificación de riesgo, timeouts y límites de red.

## Criterio De Priorización

Una feature nueva debe mejorar conversación, control del equipo, privacidad,
seguridad o distribución. Las propuestas deben explicar qué problema resuelven,
qué superficie afectan, qué coste operativo introducen y cómo se pueden retirar
si el experimento falla.
