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
import { getRoomFromUrl } from './transport'
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

/** hex SHA-256 of a UTF-8 string (WebCrypto). Used for the canonical settlement log hash. */
async function sha256Hex(s: string): Promise<string> {
  const buf = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(s))
  return Array.from(new Uint8Array(buf)).map(b => b.toString(16).padStart(2, '0')).join('')
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
  staked?: boolean,
) {
  const mySeat = isHost ? 0 : 1
  const oppSeat = isHost ? 1 : 0
  const send = (msg: WireMsg) => transport.send(msg)

  // ── staked-settlement state ─────────────────────────────
  // Per-seat on-chain payout addresses, pinned in each player's deposit memo and
  // surfaced to BOTH clients by the server (DepositStatus.seat_payout_addresses).
  // Both peers must see the SAME [seat0, seat1] ordering so they build byte-for-byte
  // identical settlement messages. Set via setPayoutAddresses().
  let seatPayoutAddresses: (string | null)[] = []
  // guard: fire the co-signed settlement exactly once at match end.
  let settlementSent = false

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
  }, ensureEngine, initialRules, !!staked)

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
    onShowdownReveal: (opp) => {
      // opponent's cards are now cryptographically bound to the committed deck.
      // this is the ONLY source of oppCards on a ZK table — never the wire claim.
      oppCards = opp
      cb.onMsg({ type: 'Showdown', hands: [[oppSeat, [cardToJson(opp[0]), cardToJson(opp[1])]]] })
      evalShowdown()
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

  /** Evaluate the showdown through the engine and award the pot. oppCards must
   *  already be set — either from the verified ZK reveal (onShowdownReveal) or,
   *  on a non-ZK table, from the asserted wire message. Waits for community
   *  cards if the ZK community reveal is still in flight (all-in async path). */
  function evalShowdown() {
    let showdownRetries = 0
    const doShowdown = () => {
      const sc = shuffle.community()
      if (sc.some(c => c > 0)) community = [...sc]

      if (!community.some(c => c > 0)) {
        // community not revealed yet — retry
        if (showdownRetries++ < 100) {
          setTimeout(doShowdown, 100)
          return
        }
        cb.onLog('showdown timeout — community cards unavailable')
      }

      // update engine with final state
      engineApi!.updateCommunity(community)
      engineApi!.updateOppCards(oppCards)

      const pot = engineApi!.pot()
      const winner = engineApi!.showdown()
      const stacks = engineApi!.stacks()
      lastStacks = [...stacks] as [number, number]

      console.log('[showdown] community=', JSON.stringify(community),
        'winner=', winner, 'pot=', pot, 'stacks=', JSON.stringify(stacks))

      cb.onMsg({ type: 'PotAwarded', seat: winner, amount: pot })
      cb.onMsg({ type: 'HandComplete', stacks: [...stacks] })
      handComplete()
    }
    doShowdown()
  }

  function startHand() {
    const rules = negotiation.rules()
    handNum++
    actionSeq = 0
    desyncHandled = false
    handGeneration++
    transcript.reset()
    console.log('[deal] pre-deal stacks=', JSON.stringify(engineApi!.stacks()), 'btn=', engineApi!.button())
    engineApi!.deal(myCards, oppCards, community, isHost)
    console.log('[deal] post-deal stacks=', JSON.stringify(engineApi!.stacks()), 'pot=', engineApi!.pot())

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
  // reconnect-resync dedup: a duplicate/re-delivered `seated` must NOT re-run the resync (it
  // nulls engineApi + re-schedules beginDeal → double-deal + desync). Track the handNum we last
  // resynced at and suppress a repeat within a short in-flight window.
  let lastResyncHandNum = -1
  let resyncInFlight = false
  // guard: handle a mid-hand desync exactly once per hand (reset in startHand).
  let desyncHandled = false

  /** A dropped/reordered/duplicated action was detected (seq gap) or the engine rejected an
   *  in-order peer action — the two engines have forked. Continuing would let each side march to
   *  a different winner, which the escrow can't co-sign, so it refunds instead of paying the pot.
   *  Stop the hand and surface a retry instead of silently corrupting state. */
  function handleDesync(reason: string) {
    if (desyncHandled) return
    desyncHandled = true
    clearTurnTimer()
    clearOppTimer()
    cb.onLog(`desync detected — ${reason}`)
    if (staked) {
      // real money: never continue on a forked state. The escrow holds both deposits and pays
      // only on a clean co-signed game, else refunds each their own — so voiding here is safe.
      cb.onMsg({ type: 'Status', phase: 'desync',
        message: 'connection desync — this hand is void and your deposit is safe. start a new hand to retry.' })
    } else {
      // free play: rebuild a fresh hand via the existing reconnect/resync handshake.
      cb.onMsg({ type: 'Status', phase: 'desync', message: 'connection desync — resyncing for a fresh hand…' })
      send({ t: 'seated', d: { name: myName, mode: 'anon' } })
    }
  }

  function handComplete() {
    clearTurnTimer()
    clearOppTimer()
    // stop any ceremony/readiness retry loops still firing for the hand just finished so they
    // don't leak or bleed into the next hand (beginDeal→shuffle.reset() also clears shuffle's,
    // but a folded/settled hand may not deal again).
    shuffle.cleanup()
    negotiation.cleanup()
    // check if someone is busted using the last known stacks
    // (not engine stacks — guest engine doesn't call showdown)
    const [s0, s1] = lastStacks
    if (s0 <= 0 || s1 <= 0) {
      const winner = s0 > 0 ? 0 : 1
      cb.onLog(`game over — ${winner === mySeat ? 'you win!' : 'opponent wins'}`)
      // match result is not an error — route as Status so it doesn't render "err: You win the match!"
      cb.onMsg({ type: 'Status', phase: 'over', message: winner === mySeat ? 'You win the match!' : 'Opponent wins the match.' })
      // staked table: co-sign the agreed final outcome so the escrow can pay the
      // real winner (server-side refund is the abandonment fallback only).
      void sendSettlement(s0, s1)
      return // no more hands
    }
    cb.onLog(isHost ? 'next hand in 2s...' : 'waiting for next hand...')
    if (isHost) {
      setTimeout(() => beginDeal(), 2000)
    }
  }

  /** Per-seat payout addresses arrived from the server (DepositStatus). Both peers
   *  get the SAME [seat0, seat1] list, so both build the identical settlement message. */
  function setPayoutAddresses(addrs: (string | null)[]) {
    if (Array.isArray(addrs) && addrs.length) seatPayoutAddresses = addrs
  }

  /** Build + sign + send this seat's half of the co-signed settlement (staked tables only).
   *
   *  The signed string MUST match poker-escrow's `settlement_message(..)` byte-for-byte:
   *      zk.poker/settle/v1:{code}:{a_stack}:{b_stack}:{a_addr}:{b_addr}:{log_hash}
   *  where seat 0 == player A (host) and seat 1 == player B (guest), regardless of "me".
   *
   *  Each client signs with its SESSION Ed25519 key — the same key pinned on-chain via the
   *  deposit memo `;id:<sessionPubKey>` — so the escrow verifies each sig against the pinned
   *  identity. Both peers submit independently; the server collects both sigs and calls /settle.
   */
  async function sendSettlement(s0: number, s1: number) {
    if (!staked || !identity || settlementSent) return
    const code = getRoomFromUrl()
    const aAddr = seatPayoutAddresses[0]
    const bAddr = seatPayoutAddresses[1]
    if (!code || !aAddr || !bAddr) {
      cb.onLog('[settle] skipped — missing room code or seat payout addresses')
      return
    }
    // Seat-indexed final chip stacks (NOT reordered to "me/opp"): seat 0 = A, seat 1 = B.
    // The escrow distributes the pot proportionally to these, so only the ratio matters and
    // both peers hold the identical [s0, s1] from the shared engine outcome.
    const aStack = Math.max(0, Math.floor(s0))
    const bStack = Math.max(0, Math.floor(s1))
    // Canonical action-log hash both peers can derive identically. The per-hand transcript
    // diverges between peers (each fills its own sessionPub / relayTs), so it is NOT usable
    // here; instead we hash the agreed, public match-end facts that both sides share exactly.
    const logHash = await sha256Hex(`zk.poker/log/v1:${code}:${aStack}:${bStack}:${aAddr}:${bAddr}`)
    const msg = `zk.poker/settle/v1:${code}:${aStack}:${bStack}:${aAddr}:${bAddr}:${logHash}`
    const sig = await identity.sign(new TextEncoder().encode(msg))
    settlementSent = true
    cb.onLog(`[settle] co-signing outcome a_stack=${aStack} b_stack=${bStack}`)
    transport.sendServer({
      type: 'Settlement',
      a_stack: aStack,
      b_stack: bStack,
      a_addr: aAddr,
      b_addr: bAddr,
      log_hash: logHash,
      sig,
    })
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
            // release our own hole-card shares; the opponent verifies them
            // against the committed deck (and we verify theirs). No plaintext
            // card claim crosses the wire on a ZK table.
            shuffle.revealShowdown()
          } else {
            send({ t: 'showdown', d: { cards: myCards } })
          }
          return // stop processing — showdown handled by verified reveal / peer message

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
      // opponent timed out — claim the win with evidence.
      // A timeout-fold IS an action: it must advance the SHARED per-hand sequence exactly like a
      // wire `action` would, so the timed-out peer can replay it at the same seq on ITS engine
      // (see the `timeout_claim` handler). Feed seq through the guard and only commit on success.
      const evidence = transcript.checkTimeout(turnTimeoutMs())
      cb.onLog(`opponent timed out (${Math.round(evidence.elapsed/1000)}s, relay_ts: ${evidence.lastRelayTs})`)
      cb.onMsg({ type: 'ActionTimeout', seat: oppSeat })
      const seq = actionSeq + 1
      const events = engineApi.apply(oppSeat, 'fold', 0, seq)
      if (!events.some(e => e.type === 'rejected')) {
        actionSeq = seq // commit: the timeout-fold now occupies this slot in the shared sequence
        send({ t: 'timeout_claim', d: {
          seat: oppSeat,
          seq,
          elapsed: evidence.elapsed,
          lastRelayTs: evidence.lastRelayTs,
          timeoutMs: turnTimeoutMs(),
        }})
        dispatch(events, oppSeat)
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

    // GLOBAL action index (both seats share one monotonic counter per hand, mirroring the
    // engine's internal action_count which resets to 0 on deal()). Feeding it to the engine
    // turns on its built-in "wrong sequence" guard so a dropped/reordered/duplicated action is
    // detected instead of silently forking the two engines.
    const seq = actionSeq + 1
    const events = engineApi.apply(mySeat, action, amount, seq)
    if (events.some(e => e.type === 'rejected')) {
      dispatch(events, mySeat)
      return
    }
    actionSeq = seq // commit: this action is now applied on our engine

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
        const oppName = (typeof d === 'string' ? d : d?.name) || 'anon'
        const oppMode = (typeof d === 'object' ? d?.mode : undefined) || 'anon'
        cb.onMsg({ type: 'OpponentJoined', seat: oppSeat, name: `${oppName} (${oppMode})` })
        if (isHost) negotiation.proposeRules(negotiation.rules())
        // Bidirectional reconnect resync (free play). A peer that (re)announces `seated` AFTER
        // we've already dealt a hand (handNum > 0) has rebuilt a FRESH game — a page reload
        // wipes its in-memory state — and would otherwise be stuck on "getting ready", because
        // our negotiation's `gameStarted` guard swallows the re-handshake. Whoever stayed
        // connected owns the authoritative hand counter + rules, so we hand them to the
        // reconnecting peer via `resync`; the HOST (seat 0 — the STABLE shuffle initiator, kept
        // stable across reloads by the seat pin in ws.ts) then deals a fresh hand at a handId
        // above the established one. Reset our engine so both sides agree on stacks. Staked
        // tables are excluded — a mid-hand reconnect there must be resolved against the escrow/
        // server hand-state authority, not silently re-dealt with real money committed.
        if (handNum > 0 && !staked) {
          // dedup: ignore a repeat `seated` for a resync we already drove (or one in flight).
          // Without this a duplicate/re-delivered frame nulls engineApi + re-schedules beginDeal
          // → the host double-deals and the two sides desync.
          if (resyncInFlight || lastResyncHandNum === handNum) {
            cb.onLog('duplicate seated ignored (resync already in progress)')
            break
          }
          resyncInFlight = true
          lastResyncHandNum = handNum
          cb.onLog('opponent rejoined — resyncing for a fresh hand')
          clearTurnTimer()
          clearOppTimer()
          shuffle.cleanup()
          negotiation.cleanup()
          send({ t: 'resync', d: { handId: shuffle.currentHandId(), rules: negotiation.rules() } })
          engineApi = null
          ensureEngine()
          if (isHost) setTimeout(() => { resyncInFlight = false; beginDeal() }, 500)
          else resyncInFlight = false
        }
        break
      }

      case 'resync': {
        // Counterpart to 'seated' above: a still-connected peer is resyncing us after OUR
        // reload. Adopt their hand counter (so our next deal's handId lands above theirs and
        // their `isNewHand` check accepts it) + rules, reset our engine to match their stacks,
        // and — if we're the host (the stable shuffle initiator) — drive the fresh deal.
        if (staked) break
        const rd = msg.d as any
        clearTurnTimer()
        clearOppTimer()
        shuffle.cleanup()
        negotiation.cleanup()
        if (typeof rd?.handId === 'number') shuffle.setHandIdBaseline(rd.handId)
        if (rd?.rules && isHost) negotiation.proposeRules(rd.rules)
        engineApi = null
        ensureEngine()
        if (isHost) setTimeout(() => beginDeal(), 300)
        break
      }

      case 'rename': {
        // peer changed their nick mid-game — update only the displayed opponent name. No engine
        // or resync side effects (that's why it's a dedicated frame, not a re-`seated`).
        const d = msg.d as any
        const nm = ((typeof d === 'string' ? d : d?.name) || 'anon').toString().slice(0, 20)
        cb.onMsg({ type: 'OpponentJoined', seat: oppSeat, name: nm })
        break
      }

      case 'deal': {
        // plaintext fallback: guest receives cards from host.
        // On a ZK table cards come ONLY from the shuffle ceremony (onDeal) — a
        // peer must not be able to inject hole/board values out of band, so we
        // ignore an unsolicited `deal` when the shuffle ceremony is active.
        if (shuffle.available) break
        const d = msg.d as any
        myCards = d.cards; community = d.community
        ensureEngine()
        startHand()
        break
      }

      case 'action': {
        if (!engineApi) break
        const d = msg.d as any
        const expected = actionSeq + 1
        const wireSeq = typeof d.seq === 'number' ? d.seq : expected
        // dedup: a re-delivered/duplicate frame carries a seq we've already applied. Applying it
        // again would double-move chips (H1). Ignore anything at or below our applied index.
        if (wireSeq <= actionSeq) {
          cb.onLog(`duplicate action ignored (seq ${wireSeq} <= ${actionSeq})`)
          break
        }
        // gap: a message was dropped or reordered (reconnect, re-key, crypto race). Applying past
        // a hole silently forks the two engines → each side thinks a different player won → the
        // escrow can't verify a matching co-signed outcome and refunds instead of paying the pot.
        // Halt and resync rather than corrupt state.
        if (wireSeq !== expected) {
          handleDesync(`action gap: got seq ${wireSeq}, expected ${expected}`)
          break
        }
        // C1: validate opponent action before applying. Pass the expected seq so the ENGINE also
        // enforces order (defense in depth: it rejects if its internal action_count disagrees).
        const events = engineApi.apply(oppSeat, d.action, d.amount ?? 0, expected)
        if (events.some(e => e.type === 'rejected')) {
          // an in-order action the engine still rejects means the two engines have already
          // diverged. Do NOT swallow-and-continue (that is what produced conflicting winners and
          // the refund) — surface it and resync.
          handleDesync(`engine rejected in-order peer action: ${d.action}`)
          break
        }
        actionSeq = expected // commit: peer action applied, advance the shared counter
        clearOppTimer() // M4: only clear after valid action
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
        // P2P: BOTH sides evaluate showdown through the engine.
        // no "host evaluates and tells guest" — both are equal peers.
        if (!engineApi) break
        // On a ZK table the opponent's hole cards are trusted ONLY after their
        // decryption shares verify against the committed deck (onShowdownReveal).
        // The plaintext `cards` in this message are unauthenticated — a losing
        // player could assert any winning hand — so we ignore them entirely and
        // let the verified reveal drive the pot. Plaintext/bot tables (no ZK
        // ceremony) still fall back to the asserted cards.
        if (shuffle.available) break
        const d = msg.d as { cards: [number, number] }
        oppCards = d.cards
        cb.onMsg({ type: 'Showdown', hands: [[oppSeat, [cardToJson(d.cards[0]), cardToJson(d.cards[1])]]] })
        evalShowdown()
        break
      }

      case 'timeout_claim': {
        // Opponent claims a seat (usually us) timed out and folded it on THEIR engine. Desync-H2:
        // the old handler was a no-op on engine state, so our engine never folded that seat → the
        // two engines disagreed on who folded → conflicting winners → escrow refund. Fix: apply the
        // SAME fold, at the SAME shared seq, so both engines end in the identical state.
        if (!engineApi) break
        const d = msg.d as any
        const foldSeat = typeof d.seat === 'number' ? d.seat : mySeat
        cb.onLog(`opponent claims timeout (seat ${foldSeat}, ${d.elapsed}ms)`)
        clearTurnTimer() // if it's us being folded, stop our own turn timer/auto-fold

        // Already folded (our own auto-fold path fired first)? Then the fold is applied and the
        // seqs are reconciled through the normal `action` frame — nothing to replay here.
        if (engineApi.seatState(foldSeat) === 3 /* folded */) {
          cb.onLog('timeout_claim redundant — seat already folded')
          break
        }

        // Replay the timeout-fold as a sequenced action, mirroring the `action` handler so the
        // shared counter stays lock-step across peers.
        const expected = actionSeq + 1
        const wireSeq = typeof d.seq === 'number' ? d.seq : expected
        if (wireSeq <= actionSeq) {
          cb.onLog(`duplicate timeout_claim ignored (seq ${wireSeq} <= ${actionSeq})`)
          break
        }
        if (wireSeq !== expected) {
          // a message was dropped/reordered ahead of this claim — replaying past the hole would
          // fork the engines, so halt and resync exactly like a gapped action.
          handleDesync(`timeout_claim gap: got seq ${wireSeq}, expected ${expected}`)
          break
        }
        const events = engineApi.apply(foldSeat, 'fold', 0, expected)
        if (events.some(e => e.type === 'rejected')) {
          // engine won't fold that seat at this seq → the two engines already diverged. Don't
          // silently continue — surface it and resync (matches the `action`-reject path).
          handleDesync(`timeout_claim fold rejected for seat ${foldSeat}`)
          break
        }
        actionSeq = expected // commit: timeout-fold occupies this slot on both engines
        clearOppTimer()
        dispatch(events, foldSeat)
        break
      }

    }
  }

  // ── public API ──────────────────────────────────────────

  function announce() {
    send({ t: 'seated', d: identity ? {
      name: myName || 'anon',
      sessionPub: identity.sessionPubKey,
      mode: identity.mode || 'anon',
      zafuPub: identity.zafuPubKey,
      delegation: identity.delegation,
    } : (myName || 'anon') })
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
    setPayoutAddresses,
    transcript,
  }
}
