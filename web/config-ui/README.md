# Jarvis Configuration UI

This is the local React/Vite configuration page for Jarvis. It is an optional
development surface: the production binary serves the generated static files
from `dist/` through the Rust/Axum server.

## Development

```bash
npm install
npm run dev
```

The Vite dev server is useful for frontend iteration. The Rust API must be
running separately when a component needs configuration data or calibration.

## Build And Lint

```bash
npm run build
npm run lint
```

The build output in `dist/` is ignored by git and must exist before the Rust
server can serve the complete UI. Without it, the Rust API still starts but
the static frontend is unavailable.

## Boundaries

- React owns presentation, form state, validation feedback, and browser-side
  WebSocket handling.
- Rust owns configuration parsing, persistence, provider status, calibration
  workers, and security decisions.
- The API is served by Jarvis on `127.0.0.1` only.
- Saved changes rewrite the local YAML configuration and normally require a
  Jarvis restart.
- Do not put API keys, personal paths, recordings, or other private data in
  this project.

## Conventions

- Use the existing components in `src/components/form/` before creating new
  controls.
- Keep section styling in CSS Modules and shared values in
  `src/styles/tokens.css`.
- Preserve the Spanish-first product language and the visual rules in
  [`docs/DESIGN.md`](../../docs/DESIGN.md).
- There is no frontend test runner configured yet; changes need build/lint and
  documented manual verification until that becomes a separate decision.
