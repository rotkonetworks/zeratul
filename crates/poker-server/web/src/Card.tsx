import { Show } from 'solid-js'
import type { CardJson } from './types'

const suits: Record<string, string> = { c: '♣', d: '♦', h: '♥', s: '♠' }
// 4-color deck — modern pro-client standard (GG/party default)
const suitColor: Record<string, string> = {
  s: 'text-[#1d1d1f]',
  h: 'text-[#e0332c]',
  d: 'text-[#1f6fcb]',
  c: 'text-[#27a052]',
}

export function Card(p: { card?: CardJson; size?: 'sm' | 'lg'; dealIndex?: number }) {
  const lg = p.size === 'lg'
  // 0.7 aspect ratio, scales up on desktop
  const w = lg ? 'w-11 sm:w-14 lg:w-18' : 'w-9 sm:w-11 lg:w-14'
  const h = lg ? 'h-15.5 sm:h-20 lg:h-25' : 'h-12.5 sm:h-15.5 lg:h-20'
  const rank = lg ? 'text-15px sm:text-19px lg:text-24px' : 'text-12px sm:text-15px lg:text-19px'
  const suit = lg ? 'text-11px sm:text-14px lg:text-18px' : 'text-9px sm:text-11px lg:text-14px'
  const anim = p.dealIndex !== undefined ? 'card-deal' : ''
  const delay = p.dealIndex !== undefined ? `--i:${p.dealIndex}` : ''

  return (
    <Show
      when={p.card}
      fallback={
        <div class={`${w} ${h} ${anim} rounded-md border border-white/12 bg-zec-elevated inline-flex items-center justify-center shadow-[0_2px_6px_rgba(0,0,0,0.45)]`}
          style={`background-image: repeating-linear-gradient(45deg,transparent,transparent 3px,rgba(244,183,40,0.06) 3px,rgba(244,183,40,0.06) 4px); ${delay}`}>
        </div>
      }
    >
      <div class={`${w} ${h} ${anim} rounded-md inline-flex flex-col items-start justify-start pl-1 pt-0.5 sm:pl-1.5 sm:pt-1 font-sans font-700 leading-none ${suitColor[p.card!.suit]} shadow-[0_2px_6px_rgba(0,0,0,0.45)] relative`}
        style={`background: linear-gradient(180deg, #fdfdfd 0%, #f0f0f2 100%); ${delay}`}>
        <span class={rank}>{p.card!.rank}</span>
        <span class={suit}>{suits[p.card!.suit]}</span>
        {/* large center suit watermark */}
        <span class={`absolute bottom-0.5 right-1 sm:bottom-1 sm:right-1.5 ${rank} opacity-90`}>{suits[p.card!.suit]}</span>
      </div>
    </Show>
  )
}
