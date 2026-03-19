/**
 * P2P poker — Service/Filter composition (Eriksen 2013).
 *
 * Architecture:
 *   negotiation  → Service: handles rules, escrow
 *   shuffle      → Filter:  intercepts deal/phase for zk-shuffle
 *   engine       → Service: pure (state, action) → events
 *   transcript   → Filter:  records signed action log
 *
 * The game orchestrator composes these. Each concern is orthogonal:
 *   "services are constructed piecemeal by composing smaller parts"
 *   — Eriksen §6
 *
 * Event flow:
 *   act() → engine.apply() → [EngineEvent] → dispatch() → UI + transport
 *   peer msg → route → engine.apply() → [EngineEvent] → dispatch() → UI
 */

import type { ServerMsg, CardJson } from './types'
import type { WireMsg, TransportProvider } from './transport'
import type { SessionIdentity } from './identity'
import { signAction } from './identity'
import { createNegotiation } from './negotiate'
import type { NegotiateApi } from './negotiate'
import { createShuffle } from './shuffle-filter'
import type { ShuffleApi } from './shuffle-filter'
import { createTranscript } from './transcript-filter'
import type { Transcript } from './transcript-filter'
import { createEngineApi } from './engine-service'
import type { EngineApi, EngineEvent } from './engine-service'
import type { Service, Filter } from './service'

// ============================================================================
// Types
// ============================================================================

export interface GameRules {
  buyin: number
  smallBlind: number
  bigBlind: number
  turnTimeout: number
}

export interface GameCallbacks {
  onMsg: (msg: ServerMsg) => void
  onLog: (text: string) => void
  onRulesProposed: (rules: GameRules, fromSelf: boolean) => void
  onRulesAccepted: (rules: GameRules) => void
  onEscrowReady: (address: string) => void
  onDepositConfirmed: (seat: number, amount: number) => void
  /** called every second with remaining turn time (seconds). -1 = no active timer. */
  onTimerTick?: (secondsLeft: number) => void
}

// ============================================================================
// WASM loaders
// ============================================================================

let WasmGameClass: any = null
let ShuffleKeysClass: any = null
let ShuffleStateClass: any = null

export async function loadWasmEngine(): Promise<boolean> {
  try {
    const mod = await import('/assets/poker_pvm.js')
    await mod.default()
    WasmGameClass = mod.WasmGame
  } catch (e) { console.warn('poker-pvm WASM not available:', e) }
  try {
    const mod = await import('/assets/poker_shuffle_wasm.js')
    await mod.default()
    ShuffleKeysClass = mod.ShuffleKeys
    ShuffleStateClass = mod.ShuffleState
    console.log('[shuffle] WASM loaded')
  } catch (e) { console.warn('poker-shuffle WASM not available:', e) }
  return !!WasmGameClass
}

// ============================================================================
// Card helpers
// ============================================================================

const RANKS = ['2','3','4','5','6','7','8','9','T','J','Q','K','A']
const SUITS = ['s','h','d','c']
function cardToJson(idx: number): CardJson {
  return { rank: RANKS[idx % 13]!, suit: SUITS[Math.floor(idx / 13)]! }
}

// ============================================================================
// Game orchestrator: composes services + filters
// ============================================================================

export function createGame(
  transport: TransportProvider,
  isHost: boolean,
  myName: string,
  cb: GameCallbacks,
  identity?: SessionIdentity,
  initialRules?: Partial<GameRules>,
) {
  const mySeat = isHost ? 0 : 1
  const oppSeat = isHost ? 1 : 0
  const send = (msg: WireMsg) => transport.send(msg)

  // ── mutable hand state (reset each hand) ────────────────
  let handNum = 0
  let myCards: [number, number] = [0, 0]
  let oppCards: [number, number] = [0, 0]
  let community: number[] = [0, 0, 0, 0, 0]
  let actionSeq = 0

  // ── timer state ──────────────────────────────────────────
  // dual-side: we track OUR timer (auto-fold) AND opponent's timer (claim timeout).
  // relay timestamps serve as neutral evidence for disputes.
  // neither player can disable the OTHER's enforcement.
  let turnTimer: ReturnType<typeof setTimeout> | null = null
  let turnTickInterval: ReturnType<typeof setInterval> | null = null
  let turnTimerPaused = false
  let turnTimerRemaining = 0
  let turnTimerStart = 0
  let oppTimer: ReturnType<typeof setTimeout> | null = null
  let oppTimerStart = 0
  const NETWORK_BUFFER_MS = 5000 // grace period for network latency
  function turnTimeoutMs() { return (negotiation.rules().turnTimeout || 30) * 1000 }

  // ── compose services ────────────────────────────────────

  let engineApi: EngineApi | null = null

  /** create engine once. subsequent hands reuse it (stacks persist). */
  function ensureEngine() {
    if (!engineApi) {
      const r = negotiation.rules()
      engineApi = createEngineApi(WasmGameClass, r.buyin, r.smallBlind, r.bigBlind)
      cb.onLog(WasmGameClass ? 'engine: poker-pvm WASM' : 'engine: JS fallback')
    }
  }

  // 1. negotiation
  const negotiation: NegotiateApi = createNegotiation(send, isHost, {
    onMsg: cb.onMsg,
    onLog: cb.onLog,
    onRulesProposed: cb.onRulesProposed,
    onRulesAccepted: cb.onRulesAccepted,
    onEscrowReady: cb.onEscrowReady,
    onReady: () => { if (isHost) beginDeal() },
  }, ensureEngine, initialRules)

  // 2. shuffle
  const shuffle: ShuffleApi = createShuffle(send, isHost, ShuffleKeysClass, ShuffleStateClass, {
    onMsg: cb.onMsg,
    onLog: cb.onLog,
    onDeal: (my, opp, comm) => {
      myCards = my; oppCards = opp; community = comm
      ensureEngine() // fresh engine for each hand (guest side)
      startHand()
    },
    onCommunityRevealed: (phase, cards) => {
      cb.onMsg({ type: 'CommunityCards', phase, cards })
      // only prompt if hand is still active (not showdown/settled from all-in)
      const p = engineApi?.phase() ?? 0
      if (p >= 2 && p <= 5) promptAction()
    },
  })

  // 3. transcript
  const transcript: Transcript = createTranscript()

  // ── deal (starts shuffle ceremony or plaintext) ─────────

  function beginDeal() {
    cb.onLog('dealing new hand...')
    ensureEngine()
    if (shuffle.available) {
      shuffle.beginDeal()
    } else {
      dealPlaintext()
    }
  }

  function dealPlaintext() {
    const deck = [...Array(52).keys()]
    for (let i = 51; i > 0; i--) {
      const j = Math.random() * (i + 1) | 0;
      [deck[i], deck[j]] = [deck[j]!, deck[i]!]
    }
    myCards = [deck[0]!, deck[1]!]
    oppCards = [deck[2]!, deck[3]!]
    community = [deck[4]!, deck[5]!, deck[6]!, deck[7]!, deck[8]!]
    if (isHost) {
      send({ t: 'deal', d: { cards: oppCards, community, stacks: engineApi!.stacks() } })
    }
    startHand()
  }

  function startHand() {
    const rules = negotiation.rules()
    handNum++
    actionSeq = 0
    handGeneration++
    transcript.reset()
    console.log('[deal] pre-deal stacks=', engineApi!.stacks(), 'btn=', engineApi!.button())
    engineApi!.deal(myCards, oppCards, community, isHost)
    console.log('[deal] post-deal stacks=', engineApi!.stacks(), 'pot=', engineApi!.pot())

    const btn = engineApi!.button()
    cb.onMsg({ type: 'HandStarted', hand_number: handNum, button: btn,
      your_cards: [cardToJson(myCards[0]), cardToJson(myCards[1])],
      stacks: [...engineApi!.stacks()] })
    cb.onMsg({ type: 'BlindsPosted',
      small_blind: [btn, rules.smallBlind],
      big_blind: [1 - btn, rules.bigBlind] })
    cb.onMsg({ type: 'PotUpdate', pots: [{ amount: engineApi!.pot(), eligible: [0, 1] }] })
    // clear any stale actions from previous hand before prompting
    cb.onMsg({ type: 'ActionRequired', seat: -1, valid_actions: [] })
    promptAction()
  }

  let lastStacks: [number, number] = [0, 0]
  let handGeneration = 0 // increments each hand, stale prompts from previous hands are ignored

  function handComplete() {
    clearTurnTimer()
    clearOppTimer()
    // check if someone is busted using the last known stacks
    // (not engine stacks — guest engine doesn't call showdown)
    const [s0, s1] = lastStacks
    if (s0 <= 0 || s1 <= 0) {
      const winner = s0 > 0 ? 0 : 1
      cb.onLog(`game over — ${winner === mySeat ? 'you win!' : 'opponent wins'}`)
      cb.onMsg({ type: 'Error', message: winner === mySeat ? 'You win the match!' : 'Opponent wins the match.' })
      return // no more hands
    }
    cb.onLog(isHost ? 'next hand in 2s...' : 'waiting for next hand...')
    if (isHost) {
      setTimeout(() => beginDeal(), 2000)
    }
  }

  // ── engine event dispatch ───────────────────────────────
  //
  // "declarative programming: operations describe what is computed;
  //  execution is handled separately" — Eriksen §4.1
  //
  // engine.apply() returns pure events. dispatch() handles them.

  function dispatch(events: EngineEvent[], fromSeat: number) {
    for (const ev of events) {
      switch (ev.type) {
        case 'acted':
          cb.onMsg({ type: 'PlayerActed', seat: ev.seat, action: ev.action, amount: ev.amount, new_stack: ev.newStack })
          cb.onMsg({ type: 'PotUpdate', pots: [{ amount: ev.pot, eligible: [0, 1] }] })
          break

        case 'fold_win':
          lastStacks = ev.stacks
          cb.onMsg({ type: 'ActionRequired', seat: -1, valid_actions: [] }) // clear buttons immediately
          cb.onMsg({ type: 'PotAwarded', seat: ev.winner, amount: ev.payout })
          cb.onMsg({ type: 'HandComplete', stacks: [...ev.stacks] })
          handComplete()
          break

        case 'phase_advance':
          if (shuffle.available) {
            shuffle.revealCommunity(ev.phase)
          } else {
            const cards = community.slice(0, ev.communityCount).map(cardToJson)
            cb.onMsg({ type: 'CommunityCards', phase: ev.phase, cards })
            if (isHost) send({ t: 'phase', d: { phase: ev.phase, cards: community.slice(0, ev.communityCount) } })
          }
          break

        case 'showdown_ready':
          if (shuffle.available) {
            shuffle.revealCommunity('flop')
            shuffle.revealCommunity('turn')
            shuffle.revealCommunity('river')
          }
          send({ t: 'showdown', d: { cards: myCards } })
          return // stop processing — showdown handled by peer message

        case 'rejected':
          cb.onLog(`rejected: ${ev.action} (${ev.reason})`)
          // re-prompt with correct valid actions
          promptAction()
          return

        case 'prompt':
          cb.onMsg({ type: 'ActionRequired', seat: ev.seat, valid_actions: ev.validActions })
          break
      }
    }
    // after processing events, check whose turn it is
    const phase = engineApi?.phase() ?? 0
    if (phase >= 2 && phase <= 5) {
      if (fromSeat !== mySeat) {
        promptAction()  // our turn — show buttons, start our timer
      } else {
        startOppTimer() // opponent's turn — start tracking their time
      }
    }
  }

  function clearTurnTimer() {
    if (turnTimer) { clearTimeout(turnTimer); turnTimer = null }
    if (turnTickInterval) { clearInterval(turnTickInterval); turnTickInterval = null }
    turnTimerPaused = false
    turnTimerRemaining = 0
    cb.onTimerTick?.(-1)
  }

  function clearOppTimer() {
    if (oppTimer) { clearTimeout(oppTimer); oppTimer = null }
  }

  /** start tracking opponent's turn time. if they don't act within
   *  timeout + network buffer, we claim a timeout (they forfeit). */
  function startOppTimer() {
    clearOppTimer()
    const ms = turnTimeoutMs() + NETWORK_BUFFER_MS
    oppTimerStart = Date.now()
    const gen = handGeneration
    oppTimer = setTimeout(() => {
      if (gen !== handGeneration) return
      if (!engineApi) return
      const phase = engineApi.phase()
      if (phase < 2 || phase > 5) return
      // opponent timed out — claim the win with evidence
      const evidence = transcript.checkTimeout(turnTimeoutMs())
      cb.onLog(`opponent timed out (${Math.round(evidence.elapsed/1000)}s, relay_ts: ${evidence.lastRelayTs})`)
      cb.onMsg({ type: 'ActionTimeout', seat: oppSeat })
      const events = engineApi.apply(oppSeat, 'fold', 0)
      if (!events.some(e => e.type === 'rejected')) {
        dispatch(events, oppSeat)
        send({ t: 'timeout_claim', d: {
          seat: oppSeat,
          elapsed: evidence.elapsed,
          lastRelayTs: evidence.lastRelayTs,
          timeoutMs: turnTimeoutMs(),
        }})
      }
    }, ms)
  }

  function startTurnTimer() {
    clearTurnTimer()
    const ms = turnTimeoutMs()
    turnTimerRemaining = ms
    turnTimerStart = Date.now()
    const gen = handGeneration
    turnTimer = setTimeout(() => onTurnTimeout(gen), ms)
    // T1: authoritative tick drives UI
    cb.onTimerTick?.(Math.ceil(ms / 1000))
    turnTickInterval = setInterval(() => {
      if (turnTimerPaused) return
      const elapsed = Date.now() - turnTimerStart
      const left = Math.max(0, Math.ceil((turnTimerRemaining - elapsed) / 1000))
      cb.onTimerTick?.(left)
    }, 1000)
  }

  function onTurnTimeout(gen: number) {
    if (gen !== handGeneration) return // H3: stale timer
    // double-check it's actually our turn
    if (!engineApi) return
    const phase = engineApi.phase()
    if (phase < 2 || phase > 5) return // T5: not in active betting phase
    cb.onLog('turn timeout — auto-fold')
    cb.onMsg({ type: 'ActionTimeout', seat: mySeat })
    act('fold', 0)
  }

  function pauseTurnTimer() {
    if (!turnTimer || turnTimerPaused) return
    turnTimerPaused = true
    turnTimerRemaining = Math.max(0, turnTimerRemaining - (Date.now() - turnTimerStart))
    clearTimeout(turnTimer)
    turnTimer = null
  }

  function resumeTurnTimer() {
    if (!turnTimerPaused || turnTimerRemaining <= 0) return
    turnTimerPaused = false
    turnTimerStart = Date.now()
    const gen = handGeneration // T4: pass generation on resume
    turnTimer = setTimeout(() => onTurnTimeout(gen), turnTimerRemaining)
  }

  function promptAction() {
    if (!engineApi) return
    const ev = engineApi.validActions(mySeat)
    console.log('[game] promptAction: mySeat=', mySeat, 'result=', ev ? 'prompting' : 'not my turn')
    if (ev) {
      dispatch([ev], mySeat)
      startTurnTimer()
    }
  }

  // ── outbound: act → sign → engine → dispatch → send ────

  function act(action: string, amount: number) {
    clearTurnTimer()
    if (!engineApi) return

    // validate against valid actions BEFORE touching engine state.
    // the UI should never offer invalid actions, but this is the gate.
    const va = engineApi.validActions(mySeat)
    if (!va) return // not our turn
    const allowed = va.validActions.some(a => a.kind === action)
    if (!allowed) {
      cb.onLog(`invalid: ${action} not in [${va.validActions.map(a => a.kind).join(',')}]`)
      promptAction() // re-show correct buttons
      return
    }

    const events = engineApi.apply(mySeat, action, amount)
    if (events.some(e => e.type === 'rejected')) {
      dispatch(events, mySeat)
      return
    }

    actionSeq++
    const seq = actionSeq // capture before async

    // M2: send wire message BEFORE dispatch to prevent reordering
    // dispatch is synchronous but signing is async — send unsigned first,
    // then record signed version in transcript when sig resolves
    send({ t: 'action', d: { action, amount, seq } })
    dispatch(events, mySeat)

    if (identity) {
      signAction(identity, mySeat, action, amount, seq).then(sig => {
        transcript.record({ seq, seat: mySeat, action, amount, sig, sessionPub: identity.sessionPubKey, relayTs: 0 })
      })
    }
  }

  // ── inbound pipeline: filters composed via andThen (Eriksen §3) ──
  //
  // each filter either handles the message or passes to next service.
  // "filters provide a combinator, andThen, which is used to combine
  //  filters with other filters — or with services" — Eriksen §3

  /** filter: if negotiation handles it, stop. otherwise pass through. */
  const negotiationFilter: Filter<WireMsg, void> = async (msg, next) => {
    if (!negotiation.handle(msg)) return next(msg)
  }

  /** filter: if shuffle handles it, stop. otherwise pass through. */
  const shuffleFilter: Filter<WireMsg, void> = async (msg, next) => {
    if (!shuffle.handle(msg)) return next(msg)
  }

  /** terminal service: route game messages to engine */
  const gameService: Service<WireMsg, void> = async (msg) => {
    routeGameMessage(msg)
  }

  /** composed inbound pipeline: negotiate → shuffle → game */
  const inbound: Service<WireMsg, void> = (msg: WireMsg) =>
    negotiationFilter(msg, (m) => shuffleFilter(m, gameService))

  function onPeerMessage(msg: WireMsg) {
    inbound(msg)
  }

  function routeGameMessage(msg: WireMsg) {
    switch (msg.t) {
      case 'seated': {
        const d = msg.d as any
        const oppName = typeof d === 'string' ? d : d.name
        const oppMode = typeof d === 'object' ? d.mode : 'anon'
        cb.onMsg({ type: 'OpponentJoined', seat: oppSeat, name: `${oppName} (${oppMode})` })
        if (isHost) negotiation.proposeRules(negotiation.rules())
        break
      }

      case 'deal': {
        // plaintext fallback: guest receives cards from host
        const d = msg.d as any
        myCards = d.cards; community = d.community
        ensureEngine()
        startHand()
        break
      }

      case 'action': {
        if (!engineApi) break
        clearOppTimer() // opponent acted — cancel their timeout
        const d = msg.d as any
        // C1: validate opponent action before applying
        const events = engineApi.apply(oppSeat, d.action, d.amount ?? 0)
        if (events.some(e => e.type === 'rejected')) {
          // opponent sent invalid action — log but don't disrupt our state
          cb.onLog(`opponent sent invalid: ${d.action}`)
          break
        }
        // transcript filter: record opponent's signed action
        if (d.sig && d.seq) {
          transcript.record({ seq: d.seq, seat: oppSeat, action: d.action, amount: d.amount ?? 0, sig: d.sig, sessionPub: '', relayTs: msg.relayTs ?? 0 })
        }
        dispatch(events, oppSeat)
        break
      }

      case 'phase': {
        // plaintext fallback (no shuffle): show community cards
        const pd = msg.d as { phase: string; cards?: number[] }
        if (!shuffle.available && pd.cards) {
          cb.onMsg({ type: 'CommunityCards', phase: pd.phase, cards: pd.cards.map(cardToJson) })
          promptAction()
        }
        break
      }

      case 'showdown': {
        const d = msg.d as { cards: [number, number] }
        oppCards = d.cards
        cb.onMsg({ type: 'Showdown', hands: [[oppSeat, [cardToJson(d.cards[0]), cardToJson(d.cards[1])]]] })

        if (isHost && engineApi) {
          const sc = shuffle.community()
          if (sc.some(c => c > 0)) community = sc
          engineApi.updateCommunity(community)
          engineApi.updateOppCards(oppCards)
          const potBefore = engineApi.pot()
          const stacksBefore = engineApi.stacks()
          console.log('[showdown] BEFORE: pot=', potBefore, 'stacks=', stacksBefore, 'phase=', engineApi.phase())
          const winner = engineApi.showdown()
          const stacks = engineApi.stacks()
          const pot = potBefore // use pre-showdown pot value
          lastStacks = [...stacks] as [number, number]
          console.log('[showdown] AFTER: winner=', winner, 'stacks=', stacks, 'pot awarded=', pot)
          cb.onMsg({ type: 'PotAwarded', seat: winner, amount: pot })
          cb.onMsg({ type: 'HandComplete', stacks: [...stacks] })
          send({ t: 'result', d: { winner, pot, stacks: [...stacks], button: engineApi.button() } })
          handComplete()
        }
        // button rotated by engine in showdown()/fold
        break
      }

      case 'timeout_claim': {
        // opponent claims we timed out. verify: did we actually exceed the timeout?
        // if our timer already fired (auto-fold sent), this is redundant.
        // if we were genuinely slow, accept the forfeit.
        const d = msg.d as any
        cb.onLog(`opponent claims timeout (${d.elapsed}ms)`)
        // if we haven't acted and our timer is expired, accept
        if (!turnTimer && !turnTimerPaused) {
          cb.onLog('timeout accepted — opponent wins hand')
        }
        // the fold was already applied on their side;
        // if they're host, they'll send the result.
        break
      }

      case 'result': {
        const d = msg.d as { winner: number; pot: number; stacks: number[]; button?: number }
        lastStacks = [d.stacks[0] ?? 0, d.stacks[1] ?? 0]
        // sync guest engine with host's authoritative state
        if (engineApi) {
          const btn = d.button ?? (engineApi.button() === 0 ? 1 : 0)
          engineApi.syncState(lastStacks, btn)
        }
        cb.onMsg({ type: 'PotAwarded', seat: d.winner, amount: d.pot })
        cb.onMsg({ type: 'HandComplete', stacks: d.stacks })
        handComplete()
        break
      }
    }
  }

  // ── public API ──────────────────────────────────────────

  function announce() {
    send({ t: 'seated', d: identity ? {
      name: myName,
      sessionPub: identity.sessionPubKey,
      mode: identity.mode,
      zafuPub: identity.zafuPubKey,
      delegation: identity.delegation,
    } : myName })
  }

  return {
    onPeerMessage,
    act,
    announce,
    dealHand: beginDeal,
    proposeRules: negotiation.proposeRules,
    acceptRules: negotiation.acceptRules,
    skipDeposit: () => {},
    pauseTimer: () => { pauseTurnTimer(); clearOppTimer() },
    resumeTimer: () => { resumeTurnTimer(); /* opp timer restarts when they act or we prompt */ },
    transcript,
  }
}
