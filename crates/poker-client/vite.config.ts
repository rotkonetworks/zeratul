import { defineConfig } from 'vite'
import solid from 'vite-plugin-solid'
import unocss from 'unocss/vite'

export default defineConfig({
  plugins: [unocss(), solid()],
  server: {
    proxy: { '/ws': { target: 'http://localhost:3000', ws: true } },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    rollupOptions: {
      external: ['/assets/poker_pvm.js', '/assets/poker_shuffle_wasm.js'],
    },
  },
})
