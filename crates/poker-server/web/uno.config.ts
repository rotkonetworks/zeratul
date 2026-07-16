import { defineConfig, presetUno } from 'unocss'
import presetIcons from '@unocss/preset-icons'

export default defineConfig({
  presets: [presetUno(), presetIcons({ scale: 1.2 })],
  theme: {
    colors: {
      zec: {
        yellow: '#f4b728',
        gold: '#c48a00',
        // yellow lifted for text-on-dark (saturated yellow text vibrates)
        text: '#f8c95c',
        dark: '#0a0a0a',
        surface: '#131316',
        elevated: '#1b1b1f',
        overlay: '#232329',
        felt: '#1e2126',
        feltb: '#101216',
      },
    },
    fontFamily: {
      sans: ['Space Grotesk', 'sans-serif'],
      mono: ['IBM Plex Mono', 'monospace'],
    },
  },
  shortcuts: {
    'panel': 'bg-zec-surface border border-white/8 rounded-xl',
    'titlebar': 'bg-zec-surface/70 backdrop-blur-md border-b border-white/8 px-4 py-2.5 flex items-center gap-2 text-12px font-semibold uppercase tracking-widest',
    // neutral ghost button — the default voice; accent is opt-in via btn-primary
    'btn': 'bg-white/6 border border-white/15 rounded-lg text-white/87 px-4 py-2 font-sans text-13px font-semibold cursor-pointer transition-all duration-150 hover:bg-white/12 hover:border-white/30 active:scale-97 focus-visible:shadow-[0_0_0_3px_rgba(244,183,40,0.35)] outline-none',
    'btn-primary': 'bg-zec-yellow! border-zec-yellow! text-black! shadow-[0_2px_16px_rgba(244,183,40,0.25)] hover:bg-zec-gold! hover:border-zec-gold!',
    'btn-secondary': 'border-zec-yellow/40! text-zec-text! hover:border-zec-yellow! hover:bg-zec-yellow/10!',
    'btn-call': 'border-green-600/50! text-green-400! hover:bg-green-500/15! hover:border-green-500!',
    'btn-danger': 'border-white/10! text-neutral-400! hover:border-red-800! hover:text-red-400! hover:bg-red-900/20!',
    'btn-allin': 'border-zec-yellow/60! text-zec-text! hover:bg-zec-yellow! hover:text-black! hover:shadow-[0_2px_16px_rgba(244,183,40,0.35)]!',
    'input-field': 'bg-black/40 border border-white/15 rounded-lg text-zec-text font-mono text-14px px-3 py-2 outline-none transition-all duration-150 focus:border-zec-yellow/70 focus:shadow-[0_0_0_3px_rgba(244,183,40,0.12)]',
    // small preset/filter chip
    'chip': 'px-3 py-1.5 rounded-lg border border-white/12 text-12px font-medium text-white/60 cursor-pointer transition-all duration-150 hover:text-white/87 hover:border-white/25',
    'chip-active': 'border-zec-yellow/50! text-zec-text! bg-zec-yellow/10!',
    // glass player pod plate
    'pod': 'bg-[rgba(15,18,24,0.88)] backdrop-blur-md border border-white/10 rounded-xl shadow-[0_4px_12px_rgba(0,0,0,0.4)]',
    // translucent pill for pot / bet amounts on the felt
    'felt-pill': 'bg-black/45 rounded-full px-3 py-1 font-mono tabular-nums',
  },
})
