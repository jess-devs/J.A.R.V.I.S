# Diseño De Jarvis

## Dirección

Jarvis debe transmitir confianza, control y privacidad, con una referencia de
ciencia ficción sutil inspirada en un mayordomo técnico. La interfaz no debe
parecer una copia literal de una película ni depender de assets protegidos.

La vara de calidad visual es Stripe Dashboard: claridad, ritmo contenido,
jerarquía precisa y estados comprensibles antes que decoración.

## Superficies

La superficie implementada hoy es la página local de configuración en
`web/config-ui/`. Es una SPA React/Vite que el binario Rust sirve como estáticos
locales. La landing pública es una superficie futura y no debe inventar
testimonios, métricas, capturas o integraciones que todavía no existan.

## Principios De UX

- Mostrar qué está configurado, qué está activo y qué requiere reiniciar.
- Hacer visible el estado de conexión, guardado, error y recuperación.
- Tratar las acciones peligrosas con lenguaje claro y consecuencias explícitas.
- Mantener el español como idioma principal de la interfaz y del producto.
- Permitir guardar silencio, recalibrar y corregir configuración sin editar YAML
  cuando la UI cubra esa sección.
- No convertir decisiones de seguridad en una preferencia visual ambigua.
- Mantener una experiencia útil en pantallas pequeñas sin perder el orden de
  las secciones.

## Lenguaje Visual

La UI usa una estética SaaS corporativa minimalista:

- Fondo blanco cálido y superficies blancas.
- Bordes grises sutiles y radios moderados.
- Texto oscuro, texto auxiliar gris y un acento azul celeste.
- Verde reservado para estados positivos.
- Sombras suaves, sin efectos fuertes.
- Tipografía Inter y fallback sans del sistema.
- Iconos de trazo fino, compactos y consistentes.
- Sin gradientes, texturas, ilustraciones decorativas ni exceso de cards.

Los tokens efectivos viven en
[`web/config-ui/src/styles/tokens.css`](../web/config-ui/src/styles/tokens.css).
Ese archivo es la fuente de los valores concretos; este documento define la
intención y las restricciones.

## Layout Actual

La navegación usa un sidebar con cuatro grupos:

- Entrada de voz.
- Salida de voz.
- Inteligencia y acciones.
- Sistema.

Las secciones actuales incluyen micrófono, STT, activación, interrupción, TTS,
audio, pipeline, LLM, agente, MCP, workers y bienvenida.

Los componentes de formulario, toasts, modal, barra de guardado, calibración y
enrollment deben reutilizarse antes de crear componentes equivalentes.

## Estados Y Accesibilidad

- Todo control interactivo debe tener estado de foco visible.
- Los errores deben aparecer cerca del control y no depender solo del color.
- Los estados de carga deben explicar qué está ocurriendo y no bloquear sin
  salida.
- Las acciones irreversibles deben tener confirmación explícita.
- Los medidores de audio deben tener una explicación textual y no ser la única
  señal de estado.
- El modo texto sigue siendo una alternativa válida cuando hablar no es viable.

## Restricciones De Implementación

- Mantener CSS Modules y los componentes existentes.
- Evitar nuevas dependencias sin una necesidad concreta y aprobada.
- No introducir una segunda aplicación frontend para una superficie existente.
- No autoabrir ventanas desde el proceso normal de background.
- La API de configuración continúa limitada a localhost.
