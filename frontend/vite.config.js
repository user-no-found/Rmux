import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

export default defineConfig({
  plugins: [vue()],
  base: './',
  server: {
    port: 5173,
    proxy: {
      '/api': 'http://localhost:18732',
      '/ws': { target: 'ws://localhost:18732', ws: true },
    },
  },
  build: { outDir: '../ui', emptyOutDir: true },
})
