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
  /** cancel any in-flight readiness retry timers (hand complete / disconnect) */
  cleanup: () => void
}

const DEFAULT_RULES: GameRules = { buyin: 1000, smallBlind: 5, bigBlind: 10, turnTimeout: 30 }

/**
 * Repeat a fire-and-forget send over the lossy blind relay until a predicate holds
 * (i.e. the ceremony step it drives has completed) or a bounded number of tries elapse.
 * Returns a cancel fn — call it on hand completion / disconnect so no timer leaks or
 * double-fires. Sends once immediately, then every `intervalMs` until `done()` is true.
 */
export function repeatUntil(
  send: () => void,
  done: () => boolean,
  intervalMs = 800,
  maxTries = 10,
): () => void {
  if (done()) return () => {}
  send()
  let tries = 1
  let timer: ReturnType<typeof setInterval> | null = setInterval(() => {
    if (done() || tries++ >= maxTries) {
      if (timer) { clearInterval(timer); timer = null }
      return
    }
    send()
  }, intervalMs)
  return () => { if (timer) { clearInterval(timer); timer = null } }
}

export function createNegotiation(
  send: (msg: WireMsg) => void,
  isHost: boolean,
  cb: NegotiateCallbacks,
  initEngine: () => void,
  initialRules?: Partial<GameRules>,
  staked?: boolean,
): NegotiateApi {
  let rules: GameRules = { ...DEFAULT_RULES, ...initialRules }
  let agreed = false
  let gameStarted = false
  // free-play readiness self-heal: the guest's single `escrow_ready` is what triggers the
  // HOST's first deal (host `onReady` → beginDeal). A dropped frame strands the table on
  // "agreement ✓" forever. So the guest keeps re-sending until the host acks (`escrow_ack`).
  let escrowAcked = false
  // guard so a duplicate/re-delivered escrow_ready can't re-fire the host's deal
  let readyFired = false
  let cancelEscrowRetry: (() => void) | null = null
  function stopEscrowRetry() { if (cancelEscrowRetry) { cancelEscrowRetry(); cancelEscrowRetry = null } }

  function proposeRules(proposed: Partial<GameRules>) {
    rules = { ...DEFAULT_RULES, ...proposed }
    send({ t: 'propose_rules', d: rules })
    cb.onRulesProposed(rules, true)
  }

  function acceptRules() {
    const firstAccept = !agreed
    agreed = true
    // Always (re)send the accept — a host self-heal re-proposal will call this again, and
    // re-sending heals a previously dropped accept frame. But only initialize the engine /
    // escrow ONCE: initEngine()/setupEscrow() are not idempotent (setupEscrow mocks a fresh
    // free-play address and fires onReady, which would otherwise double-start the hand).
    send({ t: 'accept_rules', d: {} })
    if (firstAccept) {
      cb.onRulesAccepted(rules)
      initEngine()
      setupEscrow()
    }
  }

  function setupEscrow() {
    // STAKED tables: the real escrow address arrives from the SERVER (RoomInfo with a
    // real `escrow` UA + `frost_relay_url` + `frost_room_code`), which App.tsx handles to
    // trigger the FROST DKG. We must NOT fabricate an address here and must NOT auto-proceed
    // as if deposits are done — the first hand is gated on the server's DepositStatus (both
    // seats funded on-chain). So for staked tables setupEscrow is a no-op.
    if (staked) {
      cb.onLog('waiting for escrow + deposits…')
      return
    }
    // FREE-PLAY / demo: no escrow, no wallet. Mock an address and start immediately.
    const addr = 'u1mock' + Math.random().toString(36).slice(2, 20)
    cb.onEscrowReady(addr)
    cb.onLog('deposits skipped (demo)')
    cb.onReady()
    // Resend `escrow_ready` (the frame that triggers the HOST's deal) until the host acks,
    // so a single dropped frame can't wedge the hand. Cancelable via stopEscrowRetry().
    stopEscrowRetry()
    cancelEscrowRetry = repeatUntil(
      () => send({ t: 'escrow_ready', d: { address: addr } }),
      () => escrowAcked,
    )
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
      case 'escrow_ready': {
        // staked tables never take the mock escrow path; ignore any stray peer signal so
        // the hand can't start before the server confirms on-chain deposits.
        if (staked) return true
        // Always ACK (idempotent) so the peer's escrow_ready retry loop can stop — even on a
        // duplicate/re-delivered frame. But only fire onReady ONCE: it starts the host's deal,
        // and a second beginDeal() would double-deal the hand.
        send({ t: 'escrow_ack', d: {} })
        if (!readyFired) {
          readyFired = true
          cb.onEscrowReady((msg.d as any).address)
          cb.onLog('deposits skipped (demo)')
          cb.onReady()
        }
        return true
      }
      case 'escrow_ack':
        // host confirmed it got our escrow_ready — stop the resend loop.
        escrowAcked = true
        stopEscrowRetry()
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
    cleanup: stopEscrowRetry,
  }
}
