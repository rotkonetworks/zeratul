/**
 * P2P game engine — delegates to poker-pvm WASM for deterministic execution.
 *
 * the same Rust game engine (poker-pvm) runs in:
 *   - browser via WASM (this file imports it)
 *   - PolkaVM via RISC-V (provable via WIM)
 *   - native (testing)
 *
 * TypeScript handles pre-game (negotiation, escrow, deposits)
 * and translates WASM results to ServerMsg for the UI.
 * Rust handles game logic (deterministic, provable).
 */

import type { ServerMsg, CardJson, ValidAction } from './types'
import type { WireMessage, TransportProvider } from './transport'

// ============================================================================
// Types
// ============================================================================

export interface GameRules {
  buyin: number
  smallBlind: number
  bigBlind: number
  turnTimeout: number
}

const DEFAULT_RULES: GameRules = { buyin: 1000, smallBlind: 5, bigBlind: 10, turnTimeout: 30 }

const RANKS = ['2','3','4','5','6','7','8','9','T','J','Q','K','A']
const SUITS = ['s','h','d','c']
function cardToJson(idx: number): CardJson { return { rank: RANKS[idx % 13]!, suit: SUITS[Math.floor(idx / 13)]! } }

// ============================================================================
// WASM game engine
// ============================================================================

let WasmGameClass: any = null

export async function loadWasmEngine(): Promise<boolean> {
  try {
    const mod = await import('/assets/poker_pvm.js')
    await mod.default()
    WasmGameClass = mod.WasmGame
    return true
  } catch (e) {
    console.warn('poker-pvm WASM not available:', e)
    return false
  }
}

// ============================================================================
// Callbacks
// ============================================================================

export interface GameCallbacks {
  onMsg: (msg: ServerMsg) => void
  onLog: (text: string) => void
  onRulesProposed: (rules: GameRules, fromSelf: boolean) => void
  onRulesAccepted: (rules: GameRules) => void
  onEscrowReady: (address: string) => void
  onDepositConfirmed: (seat: number, amount: number) => void
}

// ============================================================================
// Game
// ============================================================================

export function createGame(
  transport: TransportProvider,
  isHost: boolean,
  myName: string,
  cb: GameCallbacks,
) {
  const mySeat = isHost ? 0 : 1
  const oppSeat = isHost ? 1 : 0
  let rules: GameRules = { ...DEFAULT_RULES }
  let rulesAgreed = false
  let handNum = 0
  let button = 0
  let engine: any = null
  let myCards: [number, number] = [0, 0]
  let oppCards: [number, number] = [0, 0]
  let community: number[] = []
  let actionSeq = 0
  let jsPhase = 0 // 0=preflop 1=flop 2=turn 3=river 4=showdown
  let jsRoundActions = 0
  let jsBets = [0, 0]
  let jsStacks = [0, 0]
  let jsPot = 0

  function handComplete() {
    // auto-deal next hand after delay (host only)
    if (isHost) {
      setTimeout(() => {
        initEngine() // fresh engine for new hand
        dealHand()
      }, 2000)
    }
  }

  const ACTION_MAP: Record<string, number> = { fold: 0, check: 1, call: 2, bet: 3, raise: 4, allin: 5 }
  const PHASE_NAMES_JS = ['preflop','flop','turn','river','showdown']

  function initEngine() {
    if (WasmGameClass) {
      engine = new WasmGameClass(rules.buyin, rules.smallBlind, rules.bigBlind)
      cb.onLog('engine: poker-pvm WASM (deterministic)')
    } else {
      cb.onLog('engine: JS fallback')
    }
  }

  // ── negotiation ──────────────────────────────────────────────

  function proposeRules(proposed: Partial<GameRules>) {
    rules = { ...DEFAULT_RULES, ...proposed }
    transport.send({ t: 'propose_rules', d: rules })
    cb.onRulesProposed(rules, true)
  }

  function acceptRules() {
    rulesAgreed = true
    transport.send({ t: 'accept_rules', d: {} })
    cb.onRulesAccepted(rules)
    initEngine()
    setupEscrow()
  }

  // ── escrow ───────────────────────────────────────────────────

  function setupEscrow() {
    // TODO: frostito DKG via poker-sdk WASM
    const addr = 'u1mock' + Math.random().toString(36).slice(2, 20)
    cb.onEscrowReady(addr)
    transport.send({ t: 'escrow_ready', d: { address: addr } })
    skipDeposit()
  }

  function skipDeposit() {
    cb.onLog('deposits skipped (demo)')
    if (isHost) dealHand()
  }

  // ── dealing ──────────────────────────────────────────────────

  function dealHand() {
    const deck = [...Array(52).keys()]
    for (let i = 51; i > 0; i--) { const j = Math.random() * (i + 1) | 0; [deck[i], deck[j]] = [deck[j]!, deck[i]!] }

    myCards = [deck[0]!, deck[1]!]
    oppCards = [deck[2]!, deck[3]!]
    community = [deck[4]!, deck[5]!, deck[6]!, deck[7]!, deck[8]!]
    handNum++
    actionSeq = 0
    jsPhase = 0
    jsRoundActions = 0
    jsBets = [rules.smallBlind, rules.bigBlind]
    jsStacks = [rules.buyin - rules.smallBlind, rules.buyin - rules.bigBlind]
    jsPot = rules.smallBlind + rules.bigBlind

    if (engine) {
      engine.deal(myCards[0], myCards[1], oppCards[0], oppCards[1],
        community[0], community[1], community[2], community[3], community[4])
    }

    const stacks = engine ? [engine.stack(0), engine.stack(1)] : [rules.buyin, rules.buyin]
    const pot = engine ? engine.pot() : rules.smallBlind + rules.bigBlind

    cb.onMsg({ type: 'HandStarted', hand_number: handNum, button,
      your_cards: [cardToJson(myCards[0]), cardToJson(myCards[1])], stacks })
    cb.onMsg({ type: 'BlindsPosted', small_blind: [button, rules.smallBlind], big_blind: [1-button, rules.bigBlind] })
    cb.onMsg({ type: 'PotUpdate', pots: [{ amount: pot, eligible: [0,1] }] })

    transport.send({ t: 'deal', d: { cards: oppCards, community, stacks } })
    promptAction()
  }

  function promptAction() {
    const stack = engine ? engine.stack(mySeat) : rules.buyin
    cb.onMsg({ type: 'ActionRequired', seat: mySeat, valid_actions: [
      { kind: 'fold', min_amount: 0, max_amount: 0 },
      { kind: 'check', min_amount: 0, max_amount: 0 },
      { kind: 'call', min_amount: rules.bigBlind, max_amount: rules.bigBlind },
      { kind: 'bet', min_amount: rules.bigBlind, max_amount: stack },
    ]})
  }

  // ── actions ──────────────────────────────────────────────────

  function act(action: string, amount: number) {
    applyAction(mySeat, action, amount)
    transport.send({ t: 'action', d: { action, amount } })
  }

  function applyAction(seat: number, action: string, amount: number) {
    actionSeq++

    // host validates via WASM engine, guest trusts host
    if (engine && isHost) {
      console.log('[engine] before:', engine.debug_state())
      const [valid, handOver, winner, payout, advance] = engine.apply_action(seat, ACTION_MAP[action] ?? 0, amount, 0)
      console.log('[engine] after:', engine.debug_state(), {valid, handOver, winner, payout, advance})
      if (!valid) { cb.onLog(`engine rejected: seat=${seat} ${action} (${engine.debug_state()})`); return }

      cb.onMsg({ type: 'PlayerActed', seat, action, amount, new_stack: engine.stack(seat) })
      cb.onMsg({ type: 'PotUpdate', pots: [{ amount: engine.pot(), eligible: [0,1] }] })

      if (handOver && winner < 2) {
        cb.onMsg({ type: 'PotAwarded', seat: winner, amount: payout })
        handComplete(); cb.onMsg({ type: 'HandComplete', stacks: [engine.stack(0), engine.stack(1)] })
        return
      }

      if (advance) {
        const phase = engine.phase()
        const count = engine.community_count()
        if (count > 0 && phase < 6) {
          const cards = community.slice(0, count).map(cardToJson)
          cb.onMsg({ type: 'CommunityCards', phase: PHASE_NAMES[phase]!, cards })
          // tell guest about phase advance
          transport.send({ t: 'phase', d: { phase: PHASE_NAMES[phase], cards: community.slice(0, count) } })
        }
        if (phase === 6) {
          transport.send({ t: 'showdown', d: { cards: myCards } })
          return
        }
      }
      if (seat !== mySeat) promptAction()
    } else {
      // JS fallback — track state manually
      if (action === 'call') {
        const toCall = Math.max(0, jsBets[1 - seat] - jsBets[seat])
        const actual = Math.min(toCall, jsStacks[seat])
        jsStacks[seat] -= actual
        jsBets[seat] += actual
        jsPot += actual
      } else if (action === 'bet' || action === 'raise') {
        const actual = Math.min(amount, jsStacks[seat])
        jsStacks[seat] -= actual
        jsBets[seat] += actual
        jsPot += actual
      }

      jsRoundActions++
      cb.onMsg({ type: 'PlayerActed', seat, action, amount, new_stack: jsStacks[seat] })
      cb.onMsg({ type: 'PotUpdate', pots: [{ amount: jsPot, eligible: [0, 1] }] })

      if (action === 'fold') {
        const winner = seat === mySeat ? oppSeat : mySeat
        cb.onMsg({ type: 'PotAwarded', seat: winner, amount: jsPot })
        jsStacks[winner] += jsPot
        handComplete(); cb.onMsg({ type: 'HandComplete', stacks: [...jsStacks] })
        return
      }

      // check if round is complete (both acted, bets equal, passive action)
      const betsEqual = jsBets[0] === jsBets[1]
      const passive = action === 'check' || action === 'call'
      if (betsEqual && passive && jsRoundActions >= 2) {
        // advance phase
        jsPhase++
        jsRoundActions = 0
        jsBets = [0, 0]

        if (jsPhase <= 3) {
          const count = jsPhase === 1 ? 3 : jsPhase === 2 ? 4 : 5
          const cards = community.slice(0, count).map(cardToJson)
          cb.onMsg({ type: 'CommunityCards', phase: PHASE_NAMES_JS[jsPhase]!, cards })
          if (isHost) {
            transport.send({ t: 'phase', d: { phase: PHASE_NAMES_JS[jsPhase], cards: community.slice(0, count) } })
          }
          promptAction()
        } else {
          // showdown
          transport.send({ t: 'showdown', d: { cards: myCards } })
        }
      } else if (action === 'bet' || action === 'raise') {
        jsRoundActions = 1 // reset — opponent must respond
        if (seat !== mySeat) promptAction()
      } else if (seat !== mySeat) {
        promptAction()
      }
    }
  }

  // ── peer messages ────────────────────────────────────────────

  function onPeerMessage(msg: WireMessage) {
    switch (msg.t) {
      case 'seated':
        cb.onMsg({ type: 'OpponentJoined', seat: oppSeat, name: msg.d as string })
        if (isHost && !rulesAgreed) proposeRules(rules)
        break
      case 'propose_rules':
        rules = msg.d as GameRules
        cb.onRulesProposed(rules, false)
        break
      case 'accept_rules':
        rulesAgreed = true
        cb.onRulesAccepted(rules)
        initEngine()
        break
      case 'escrow_ready':
        cb.onEscrowReady((msg.d as any).address)
        skipDeposit()
        break
      case 'deal': {
        const d = msg.d as any
        myCards = d.cards; community = d.community
        handNum++; actionSeq = 0
        jsPhase = 0; jsRoundActions = 0
        jsBets = [rules.smallBlind, rules.bigBlind]
        jsStacks = d.stacks ? [...d.stacks] : [rules.buyin - rules.smallBlind, rules.buyin - rules.bigBlind]
        jsPot = rules.smallBlind + rules.bigBlind
        if (engine) engine.deal(myCards[0], myCards[1], 0, 0, community[0], community[1], community[2], community[3], community[4])
        cb.onMsg({ type: 'HandStarted', hand_number: handNum, button,
          your_cards: [cardToJson(myCards[0]), cardToJson(myCards[1])], stacks: d.stacks })
        cb.onMsg({ type: 'BlindsPosted', small_blind: [button, rules.smallBlind], big_blind: [1-button, rules.bigBlind] })
        cb.onMsg({ type: 'PotUpdate', pots: [{ amount: engine?.pot() ?? 15, eligible: [0,1] }] })
        promptAction()
        break
      }
      case 'action':
        applyAction(oppSeat, (msg.d as any).action, (msg.d as any).amount)
        break
      case 'phase': {
        // host tells us about phase advance (community cards)
        const pd = msg.d as { phase: string; cards: number[] }
        cb.onMsg({ type: 'CommunityCards', phase: pd.phase, cards: pd.cards.map(cardToJson) })
        promptAction()
        break
      }
      case 'showdown': {
        const d = msg.d as { cards: [number, number] }
        oppCards = d.cards
        cb.onMsg({ type: 'Showdown', hands: [[oppSeat, [cardToJson(d.cards[0]), cardToJson(d.cards[1])]]] })

        if (isHost) {
          // host evaluates and broadcasts result
          let winner: number
          if (engine) {
            winner = engine.showdown()
          } else {
            // JS: evaluate using seat 0 and seat 1 cards (host knows both)
            const best = (cards: number[]) => Math.max(...cards.map(c => c % 13), ...community.map(c => c % 13))
            const s0Best = best([...myCards])   // host is seat 0
            const s1Best = best([...oppCards])  // guest is seat 1
            winner = s0Best >= s1Best ? 0 : 1
          }
          const pot = engine ? engine.pot() : jsPot
          const stacks = engine
            ? [engine.stack(0), engine.stack(1)]
            : (jsStacks[winner] += pot, [...jsStacks])
          cb.onMsg({ type: 'PotAwarded', seat: winner, amount: pot })
          handComplete(); cb.onMsg({ type: 'HandComplete', stacks })
          // tell guest the result
          transport.send({ t: 'result', d: { winner, pot, stacks } })
        }
        // guest waits for 'result' message
        button = 1 - button
        break
      }
      case 'result': {
        // host tells us the winner
        const d = msg.d as { winner: number; pot: number; stacks: number[] }
        cb.onMsg({ type: 'PotAwarded', seat: d.winner, amount: d.pot })
        handComplete(); cb.onMsg({ type: 'HandComplete', stacks: d.stacks })
        break
      }
    }
  }

  function announce() { transport.send({ t: 'seated', d: myName }) }
  return { onPeerMessage, act, announce, dealHand, proposeRules, acceptRules, skipDeposit }
}
