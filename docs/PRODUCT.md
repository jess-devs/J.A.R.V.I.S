# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Stack

Framework moderno, decidido por superficie:

- **Página de configuración local:** Vite + React, SPA de solo cliente. Buildea a HTML/JS/CSS estáticos que el propio binario Rust de Jarvis embebe y sirve (servidor HTTP local, `127.0.0.1` únicamente) — sin runtime Node en producción. Elegido sobre Astro porque la página es interactiva de punta a punta (estado compartido entre secciones, validación en vivo, estado en tiempo real), no un sitio de contenido mayormente estático.
- **Landing page:** framework aún sin decidir puntualmente; Astro es la primera opción a evaluar ya que es un sitio de contenido mayormente estático, pero queda abierto al empezar esa superficie.

## Users

Personas hispanohablantes que instalan y corren Jarvis en su propia máquina de escritorio. El desarrollo actual prioriza el uso personal real, pero el núcleo debe poder convertirse en una instalación distribuible y multiplataforma. Le hablan por voz (o usan el modo texto) para controlar su PC de forma manos-libres y pueden usar la página de configuración local para ajustar `config.yaml` sin editar YAML a mano.

## Product Purpose

Jarvis es un asistente de voz conversacional y agéntico en tiempo real (STT → LLM → TTS), en español, pensado para correr 100% local con opción explícita de usar servicios en la nube. Vive como proceso de fondo, responde cuando el usuario se dirige a él, puede guardar silencio y busca una conversación fluida con interrupciones naturales. Además de conversar, usa herramientas para controlar el sistema, archivos, apps, mouse/pantalla, traducir, crear recordatorios, definir sus propias tools, buscar en la web y recordar cosas entre sesiones — todo por voz, con confirmación hablada para las acciones peligrosas.

Éxito: alguien lo instala, le habla en español, y logra que ejecute tareas reales en su PC (abrir apps, buscar archivos, controlar el sistema) sin depender de la nube ni exponer sus datos, con un margen de seguridad claro para lo riesgoso.

## Positioning

Frente a asistentes de voz basados en nube (Alexa, Google Assistant, Siri) o wrappers de chat genéricos, Jarvis es agéntico (ejecuta acciones reales en el sistema, no solo responde), 100% local por defecto (STT/LLM/TTS corren en la máquina del usuario, sin API key) y nativamente en español. Ningún competidor cercano combina local-first, control real del sistema con niveles de riesgo deterministas, y español como idioma primario.

## Operating Context

- Se instala y corre en la máquina de escritorio del usuario (Windows hoy; Linux en desarrollo — build/tests verificados en WSL/Ubuntu, falta correr la app completa ahí; Mac no soportado todavía).
- Requiere micrófono/parlantes; el modo texto (`--text-mode`) es alternativa sin micrófono (sigue respondiendo por voz).
- Arquitectura: orquestador Rust (`src/`, todo el networking, config y pipeline) + dos workers Python de inferencia ML pura (`workers/`), hablados por stdio.
- La configuración personal vive en `config.yaml`, ignorado por git; `config.example.yaml` es la plantilla portable y `src/config.rs` define los defaults.
- La página de configuración local (`web/config-ui/`) es un prototipo React/Axum. La landing pública es una superficie futura.
- Acciones de riesgo requieren confirmación hablada o un código de aceptación pronunciado, verificado en Rust — nunca decidido por el LLM.

## Capabilities and Constraints

- Proveedores de LLM: Ollama y LM Studio (locales, sin API key) o Anthropic/OpenAI/DeepSeek (nube, requieren API key).
- Proveedores de TTS: Piper (local) o ElevenLabs/Cartesia (nube).
- Herramientas agénticas: estado del sistema, abrir/cerrar apps, buscar/abrir archivos, ejecutar comandos de shell, volumen, control de medios, captura de pantalla, mouse, traducción, recordatorios, tools personalizadas, búsqueda web, memoria persistente (SQLite local), y cliente MCP a servidores externos.
- Tres niveles de riesgo deterministas (lectura / confirmación / código), decididos en Rust, nunca por el LLM; el nivel código exige un código hablado que el modelo no puede ver ni confirmar por su cuenta.
- Sin aplicación de escritorio nativa todavía (roadmap); sin ejecución automática al arranque del equipo todavía (roadmap).
- La página de configuración local existe como prototipo; la landing pública todavía no existe.
- La TUI de terminal fue eliminada y no forma parte del producto.

## Brand Commitments

Dirección visual pinneada por el usuario para las superficies web (landing page y página de configuración local): estética SaaS minimalista corporativa — fondo blanco cálido con grandes áreas de espacio negativo, bordes sutiles gris claro, contenedor principal centrado con esquinas suavemente redondeadas. Paleta neutra (blanco, gris claro, gris oscuro casi negro para texto, gris medio para texto auxiliar) con un único acento saturado, azul celeste, usado con moderación en acciones primarias e indicadores; acentos verdes puntuales para estados positivos (sincronización, métricas favorables). Sidebar izquierda angosta con iconos de trazo fino y etiqueta breve, ítem activo resaltado con fondo tenue/borde ligero/sombra casi imperceptible; logotipo simple (símbolo geométrico azul + texto oscuro). Tipografía sans moderna tipo Inter/SF Pro/Helvetica Neue, jerarquía discreta (título seminegrita, subtítulo gris chico, tabs horizontales compactas). Controles flat: sliders de línea fina con manija circular chica, etiquetas en mayúsculas pequeñas, escalas conservador↔agresivo. Tarjetas de métricas con bordes grises muy claros, radio 6–10px, anillos de progreso, números grandes en negro; tablas con divisores horizontales delicados. Botones compactos, radio moderado; primario azul celeste sólido con texto blanco, secundarios blancos con borde gris claro. Sin gradientes, ilustraciones decorativas, texturas ni sombras fuertes.

Vara de calidad/fidelidad de referencia (craft bar), confirmada por el usuario: **Stripe Dashboard**.

## Evidence on Hand

- [`../README.md`](../README.md), [`CONFIGURACION.md`](CONFIGURACION.md),
  [`ARCHITECTURE.md`](ARCHITECTURE.md), [`SECURITY.md`](SECURITY.md) y
  [`ROADMAP.md`](ROADMAP.md) documentan capacidades, configuración, estructura,
  límites y evolución.
- No hay assets de marca, logo, ni contenido de landing page todavía (texto, capturas, testimonios). Trabajo de diseño futuro no debe inventar prueba social, métricas ni capturas que no existan.

## Product Principles

1. Local-first por defecto: nada sale de la máquina del usuario salvo que él mismo active un proveedor de nube.
2. Seguridad determinista: el riesgo de una acción lo clasifica Rust, nunca el LLM; lo más peligroso exige un código hablado que el modelo no puede ver ni confirmar por su cuenta.
3. Español como idioma primario de principio a fin (voz, prompts, mensajes), no una traducción de un producto en inglés.
4. Configuración declarativa y opcional: toda clave de `config.yaml` tiene un default razonable; nada es obligatorio para arrancar.
5. Separación de responsabilidades: Rust orquesta y decide, Python solo hace inferencia ML y captura de audio vía stdio.
6. Conversación respetuosa del contexto: Jarvis no responde a todo el audio ambiente, puede silenciarse y no toma acciones por iniciativa propia.

## Accessibility & Inclusion

Modo texto (`--text-mode`) como alternativa al micrófono para ambientes ruidosos, debugging, o cuando hablar en voz alta no es práctico. Sin requerimiento de accesibilidad adicional confirmado todavía para las futuras superficies web.
