import { defineConfig, presetUno } from 'unocss'

export default defineConfig({
  presets: [presetUno()],
  theme: {
    colors: {
      zec: {
        yellow: '#f4b728',
        gold: '#c48a00',
        dark: '#0a0a0a',
        surface: '#141414',
        felt: '#0f1f0f',
        feltb: '#1a3a1a',
      },
    },
    fontFamily: {
      sans: ['Space Grotesk', 'sans-serif'],
      mono: ['IBM Plex Mono', 'monospace'],
    },
  },
  shortcuts: {
    'panel': 'bg-zec-surface border-2 border-zec-yellow shadow-[3px_3px_0_0_#f4b728]',
    'titlebar': 'bg-zec-surface border-b-2 border-zec-yellow px-3 py-2 flex items-center gap-2 text-11px font-bold uppercase tracking-widest',
    'btn': 'bg-zec-surface border-2 border-zec-yellow text-white px-4 py-2 font-sans text-11px font-semibold uppercase tracking-wide cursor-pointer transition-colors hover:bg-zec-yellow/10',
    'btn-primary': 'bg-zec-yellow! border-zec-gold! text-black! hover:bg-zec-gold!',
    'btn-danger': 'border-neutral-600! text-neutral-400! hover:border-red-800! hover:text-red-400! hover:bg-red-900/20!',
    'btn-allin': 'border-zec-yellow! text-zec-yellow! hover:bg-zec-yellow! hover:text-black!',
    'input-field': 'bg-zec-dark border-2 border-zec-yellow text-zec-yellow font-mono text-13px px-2 py-1.5 outline-none focus:shadow-[0_0_0_1px_#f4b728]',
  },
})
