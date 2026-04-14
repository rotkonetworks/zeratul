/**
 * Pre-game negotiation as a state machine.
 *
 * Handles: rules proposal/acceptance, escrow setup, deposit skip.
 * This is a Service[Envelope, void] — it processes negotiation
 * wire messages and calls back when ready to play.
 *
 * Separated from game logic per Eriksen: "small, orthogonal,
 * reusable components that compose well."
 */

import type { WireMsg } from './service'
import type { GameRules } from './game'
import type { ServerMsg } from './types'

export interface NegotiateCallbacks {
  onMsg: (msg: ServerMsg) => void
  onLog: (text: string) => void
  onRulesProposed: (rules: GameRules, fromSelf: boolean) => void
  onRulesAccepted: (rules: GameRules) => void
  onEscrowReady: (address: string) => void
  /** called when negotiation is complete and game can begin */
  onReady: () => void
}

export interface NegotiateApi {
  /** handle an inbound negotiation message. returns true if handled. */
  handle: (msg: WireMsg) => boolean
  /** host initiates rules proposal */
  proposeRules: (proposed: Partial<GameRules>) => void
  /** accept proposed rules */
  acceptRules: () => void
  /** get current rules */
  rules: () => GameRules
}

const DEFAULT_RULES: GameRules = { buyin: 1000, smallBlind: 5, bigBlind: 10, turnTimeout: 30 }

export function createNegotiation(
  send: (msg: WireMsg) => void,
  isHost: boolean,
  cb: NegotiateCallbacks,
  initEngine: () => void,
  initialRules?: Partial<GameRules>,
): NegotiateApi {
  let rules: GameRules = { ...DEFAULT_RULES, ...initialRules }
  let agreed = false
  let gameStarted = false

  function proposeRules(proposed: Partial<GameRules>) {
    rules = { ...DEFAULT_RULES, ...proposed }
    send({ t: 'propose_rules', d: rules })
    cb.onRulesProposed(rules, true)
  }

  function acceptRules() {
    agreed = true
    send({ t: 'accept_rules', d: {} })
    cb.onRulesAccepted(rules)
    initEngine()
    setupEscrow()
  }

  function setupEscrow() {
    // TODO: frostito DKG via poker-sdk WASM
    const addr = 'u1mock' + Math.random().toString(36).slice(2, 20)
    cb.onEscrowReady(addr)
    send({ t: 'escrow_ready', d: { address: addr } })
    cb.onLog('deposits skipped (demo)')
    cb.onReady()
  }

  function handle(msg: WireMsg): boolean {
    switch (msg.t) {
      case 'propose_rules':
        if (gameStarted) return true // M8: ignore mid-game rule changes
        rules = msg.d as GameRules
        cb.onRulesProposed(rules, false)
        return true
      case 'accept_rules':
        if (gameStarted) return true
        agreed = true
        gameStarted = true
        cb.onRulesAccepted(rules)
        initEngine()
        return true
      case 'escrow_ready':
        cb.onEscrowReady((msg.d as any).address)
        cb.onLog('deposits skipped (demo)')
        cb.onReady()
        return true
      default:
        return false
    }
  }

  return {
    handle,
    proposeRules,
    acceptRules,
    rules: () => rules,
  }
}
