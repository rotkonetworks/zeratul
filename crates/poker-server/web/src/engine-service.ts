/**
 * Game engine as a pure Service[GameAction, GameEvent[]].
 *
 * "operations describe what is computed; execution is handled separately"
 *   — Eriksen §4.1
 *
 * The engine is a value transformation: (state, action) → events.
 * No I/O, no transport, no crypto. Same code runs in:
 *   - browser WASM (this file)
 *   - PolkaVM RISC-V (provable via WIM)
 *   - native Rust (testing)
 *
 * Events are pure data. The caller (game.ts) dispatches them
 * to the UI, transport, shuffle filter, and transcript.
 */

import type { Service } from './service'
import type { ServerMsg, CardJson, ValidAction } from './types'

const RANKS = ['2','3','4','5','6','7','8','9','T','J','Q','K','A']
const SUITS = ['s','h','d','c']
const PHASE_NAMES: Record<number, string> = { 2: 'preflop', 3: 'flop', 4: 'turn', 5: 'river', 6: 'showdown' }
function cardToJson(idx: number): CardJson {
  return { rank: RANKS[idx % 13]!, suit: SUITS[Math.floor(idx / 13)]! }
}

// ============================================================================
// Event types returned by the engine service
// ============================================================================

export type EngineEvent =
  | { type: 'acted'; seat: number; action: string; amount: number; newStack: number; pot: number }
  | { type: 'fold_win'; winner: number; payout: number; stacks: [number, number] }
  | { type: 'phase_advance'; phase: string; communityCount: number }
  | { type: 'showdown_ready' }
  | { type: 'allin_showdown' }
  | { type: 'rejected'; seat: number; action: string; reason: string }
  | { type: 'prompt'; seat: number; validActions: ValidAction[] }

const ACTION_MAP: Record<string, number> = { fold: 0, check: 1, call: 2, bet: 3, raise: 4, allin: 5 }

// ============================================================================
// WASM engine service
// ============================================================================

export interface EngineApi {
  /** apply an action, returns pure events */
  apply: (seat: number, action: string, amount: number) => EngineEvent[]
  /** deal a new hand */
  deal: (myCards: [number, number], oppCards: [number, number], community: number[], isHost: boolean) => void
  /** compute valid actions for a seat */
  validActions: (seat: number) => EngineEvent | null
  /** update community cards without re-dealing (preserves pot) */
  updateCommunity: (community: number[]) => void
  /** update opponent cards for showdown eval */
  updateOppCards: (cards: [number, number]) => void
  /** sync stacks and button (guest catches up to host after showdown) */
  syncState: (stacks: [number, number], button: number) => void
  /** evaluate showdown, returns winner */
  showdown: () => number
  /** get current pot */
  pot: () => number
  /** get stacks */
  stacks: () => [number, number]
  /** get phase */
  phase: () => number
  /** get button position (0 or 1) */
  button: () => number
  /** has WASM engine */
  hasEngine: () => boolean
}

export function createEngineApi(WasmGameClass: any, buyin: number, sb: number, bb: number): EngineApi {
  let engine: any = null
  if (WasmGameClass) {
    engine = new WasmGameClass(buyin, sb, bb)
  }

  function apply(seat: number, action: string, amount: number): EngineEvent[] {
    if (!engine) return [] // no WASM, caller uses JS fallback

    const [valid, handOver, winner, payout, advance] =
      engine.apply_action(seat, ACTION_MAP[action] ?? 0, amount, 0)

    if (!valid) {
      return [{ type: 'rejected', seat, action, reason: engine.debug_state() }]
    }

    const events: EngineEvent[] = []

    events.push({
      type: 'acted',
      seat,
      action,
      amount,
      newStack: engine.stack(seat),
      pot: engine.pot(),
    })

    if (handOver && winner < 2) {
      events.push({
        type: 'fold_win',
        winner,
        payout,
        stacks: [engine.stack(0), engine.stack(1)],
      })
      return events
    }

    if (advance) {
      const phase = engine.phase()
      const phaseName = PHASE_NAMES[phase]
      if (phase < 6 && phaseName) {
        events.push({
          type: 'phase_advance',
          phase: phaseName,
          communityCount: engine.community_count(),
        })
      }
      if (phase === 6) {
        events.push({ type: 'showdown_ready' })
      }
    }

    return events
  }

  function deal(myCards: [number, number], oppCards: [number, number], community: number[], isHost: boolean) {
    if (!engine) return
    if (isHost) {
      engine.deal(myCards[0], myCards[1], oppCards[0], oppCards[1],
        community[0], community[1], community[2], community[3], community[4])
    } else {
      engine.deal(myCards[0], myCards[1], 0, 0,
        community[0], community[1], community[2], community[3], community[4])
    }
  }

  function validActions(seat: number): EngineEvent | null {
    if (!engine || engine.acting_seat() !== seat) return null
    const myStack = engine.stack(seat)
    const myBet = engine.bet(seat)
    const oppBet = engine.bet(1 - seat)
    const toCall = oppBet - myBet

    const minBet = bb // big blind is minimum bet/raise
    const actions: ValidAction[] = [{ kind: 'fold', min_amount: 0, max_amount: 0 }]

    if (toCall <= 0) {
      actions.push({ kind: 'check', min_amount: 0, max_amount: 0 })
      if (myStack > 0) actions.push({ kind: 'bet', min_amount: Math.min(minBet, myStack), max_amount: myStack })
    } else {
      actions.push({ kind: 'call', min_amount: Math.min(toCall, myStack), max_amount: Math.min(toCall, myStack) })
      if (myStack > toCall) actions.push({ kind: 'raise', min_amount: Math.min(toCall + minBet, myStack), max_amount: myStack })
    }
    if (myStack > 0) actions.push({ kind: 'allin', min_amount: myStack, max_amount: myStack })

    return { type: 'prompt', seat, validActions: actions }
  }

  function updateCommunity(community: number[]) {
    if (!engine) return
    engine.update_community(
      community[0] ?? 0, community[1] ?? 0, community[2] ?? 0,
      community[3] ?? 0, community[4] ?? 0,
    )
  }

  /** update opponent's hole cards. seat determined by who we are (host=0, opp=1) */
  function updateOppCards(cards: [number, number]) {
    if (!engine) return
    // host is seat 0, opponent is seat 1. update seat 1's cards.
    engine.update_opp_cards(cards[0], cards[1])
  }

  function syncState(stacks: [number, number], button: number) {
    if (!engine) return
    engine.set_state(stacks[0], stacks[1], button)
  }

  return {
    apply,
    deal,
    validActions,
    updateCommunity,
    updateOppCards,
    syncState,
    showdown: () => engine?.showdown() ?? 0,
    pot: () => engine?.pot() ?? 0,
    stacks: () => engine ? [engine.stack(0), engine.stack(1)] : [buyin, buyin],
    phase: () => engine?.phase() ?? 0,
    button: () => engine?.button() ?? 0,
    hasEngine: () => !!engine,
  }
}
