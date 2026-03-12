import type { CardJson } from './types'

const suits: Record<string, string> = { c: '\u2663', d: '\u2666', h: '\u2665', s: '\u2660' }
const red = (s: string) => s === 'h' || s === 'd'

export function Card(p: { card?: CardJson; size?: 'sm' | 'lg' }) {
  const lg = p.size === 'lg'
  const w = lg ? 'w-12' : 'w-10'
  const h = lg ? 'h-17' : 'h-14'
  const text = lg ? 'text-17px' : 'text-14px'

  if (!p.card) {
    return (
      <div class={`${w} ${h} rounded-sm border border-neutral-700 bg-zec-surface inline-flex items-center justify-center`}
        style="background-image: repeating-linear-gradient(45deg,transparent,transparent 3px,rgba(244,183,40,0.05) 3px,rgba(244,183,40,0.05) 4px)">
      </div>
    )
  }

  const c = p.card
  const color = red(c.suit) ? 'text-red-500' : 'text-neutral-900'

  return (
    <div class={`${w} ${h} rounded-sm border border-stone-300 bg-stone-100 inline-flex items-center justify-center font-mono font-medium ${text} ${color}`}>
      {c.rank}{suits[c.suit]}
    </div>
  )
}
