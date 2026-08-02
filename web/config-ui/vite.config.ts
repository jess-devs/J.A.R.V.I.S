import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// El servidor real de la API es el binario de Jarvis (axum, ver
// src/config_ui/), no este dev server. En dev, Vite hace de proxy hacia
// 127.0.0.1:4756 (config.web_ui.port por defecto) para que el frontend
// pueda hacer fetch('/api/...') igual que en producción, donde el mismo
// binario de Rust sirve los estáticos del build.
export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:4756',
        changeOrigin: true,
      },
    },
  },
})
