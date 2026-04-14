/**
 * ZK-shuffle as a filter (mental poker ceremony).
 *
 * Intercepts deal/phase messages to perform the ristretto255
 * ElGamal shuffle ceremony. Neither player can see the other's
 * cards or rig the deck.
 *
 * The shuffle filter is stateful (per-hand), but its state is
 * isolated from the game engine. The engine is pure; the shuffle
 * filter handles the cryptographic card dealing.
 *
 * Wire messages handled: shuffle_pk, shuffle_init, shuffle_done, reveal
 * Wire messages emitted: shuffle_pk, shuffle_init, shuffle_done, reveal, phase
 */

import type { WireMsg } from './service'
import type { ServerMsg, CardJson } from './types'

const RANKS = ['2','3','4','5','6','7','8','9','T','J','Q','K','A']
const SUITS = ['s','h','d','c']
function cardToJson(idx: number): CardJson {
  return { rank: RANKS[idx % 13]!, suit: SUITS[Math.floor(idx / 13)]! }
}

export interface ShuffleCallbacks {
  onMsg: (msg: ServerMsg) => void
  onLog: (text: string) => void
  /** called when hole cards are ready and game can deal */
  onDeal: (myCards: [number, number], oppCards: [number, number], community: number[]) => void
  /** called when community cards for a phase are revealed */
  onCommunityRevealed: (phase: string, cards: CardJson[]) => void
}

export interface ShuffleApi {
  /** handle an inbound shuffle message. returns true if handled. */
  handle: (msg: WireMsg) => boolean
  /** start a new shuffle ceremony (host calls this) */
  beginDeal: () => void
  /** reveal community cards for a phase advance */
  revealCommunity: (phase: string) => void
  /** true if shuffle WASM is available */
  available: boolean
  /** the shuffleState (for engine deal with community placeholders) */
  community: () => number[]
}

export function createShuffle(
  send: (msg: WireMsg) => void,
  isHost: boolean,
  ShuffleKeysClass: any,
  ShuffleStateClass: any,
  cb: ShuffleCallbacks,
): ShuffleApi {
  const available = !!(ShuffleKeysClass && ShuffleStateClass)

  // per-hand state
  let shuffleKeys: any = null
  let shuffleState: any = null
  let myPkHex = ''
  let oppPkHex = ''
  let myShares = new Map<number, string>()
  let oppShares = new Map<number, string>()
  let shuffleReady = false
  let communityRevealed = 0
  let communityCards = [0, 0, 0, 0, 0]
  let handId = 0 // increments each deal to distinguish new hands

  function reset() {
    shuffleKeys = null
    shuffleState = null
    myPkHex = ''
    oppPkHex = ''
    myShares = new Map()
    oppShares = new Map()
    shuffleReady = false
    communityRevealed = 0
    communityCards = [0, 0, 0, 0, 0]
  }

  // ── shuffle ceremony ──────────────────────────────────────

  function beginDeal() {
    if (!available) return
    handId++
    reset()
    shuffleKeys = new ShuffleKeysClass()
    myPkHex = shuffleKeys.public_key_hex()
    cb.onLog('shuffling deck...')
    cb.onMsg({ type: 'Status', phase: 'shuffling', message: 'exchanging keys...' })
    send({ t: 'shuffle_pk', d: { pk: myPkHex, hand: handId } })
    maybeContinue()
  }

  function maybeContinue() {
    if (!shuffleKeys || !oppPkHex) return
    if (isHost) hostShuffle()
  }

  function hostShuffle() {
    shuffleState = new ShuffleStateClass(myPkHex, oppPkHex)
    const preDeck = shuffleState.deck_hex()
    cb.onMsg({ type: 'Status', phase: 'shuffling', message: 'shuffling deck...' })
    const result = JSON.parse(shuffleState.shuffle_and_prove(0))
    cb.onMsg({ type: 'Status', phase: 'shuffling', message: 'proving shuffle...' })
    send({ t: 'shuffle_init', d: { pk_a: myPkHex, pk_b: oppPkHex, pre_deck: preDeck, deck: result.deck, proof: result.proof } })
  }

  function onShuffleInit(d: any) {
    // guest creates state from the INITIAL (pre-shuffle) deck
    // so the transcript matches the host's state before proving
    shuffleState = ShuffleStateClass.from_initial_deck(d.pk_a, d.pk_b, d.pre_deck)

    // verify host's shuffle proof (Chaum-Pedersen)
    try {
      const valid = shuffleState.verify_and_apply(d.deck, d.proof)
      if (valid) {
        cb.onMsg({ type: 'Status', phase: 'shuffling', message: 'host shuffle verified' })
        cb.onLog('shuffle: host proof VERIFIED ✓')
      } else {
        cb.onMsg({ type: 'Error', message: 'deck verification failed — opponent may be cheating' })
        cb.onLog('shuffle: host proof FAILED — deck rejected')
        return // abort hand
      }
    } catch (e: any) {
      // verification threw — log the actual error for debugging
      cb.onLog(`shuffle: verify error: ${e}`)
      cb.onMsg({ type: 'Error', message: `shuffle verification error: ${e}` })
      return // abort hand
    }

    cb.onLog('shuffle: guest shuffling...')
    const result = JSON.parse(shuffleState.shuffle_and_prove(1))
    cb.onLog('shuffle: guest done')
    send({ t: 'shuffle_done', d: { deck: result.deck, proof: result.proof } })
    sendShares([0, 1, 2, 3])
    tryRevealHoleCards()
  }

  function onShuffleDone(d: any) {
    // host verifies guest's shuffle proof (Chaum-Pedersen)
    try {
      const valid = shuffleState.verify_and_apply(d.deck, d.proof)
      if (valid) {
        cb.onMsg({ type: 'Status', phase: 'shuffling', message: 'guest shuffle verified' })
        cb.onLog('shuffle: guest proof VERIFIED ✓')
      } else {
        cb.onMsg({ type: 'Error', message: 'deck verification failed — opponent may be cheating' })
        cb.onLog('shuffle: guest proof FAILED — deck rejected')
        return // abort hand
      }
    } catch (e: any) {
      cb.onLog(`shuffle: verify error: ${e}`)
      cb.onMsg({ type: 'Error', message: `shuffle verification error: ${e}` })
      return // abort hand
    }

    sendShares([0, 1, 2, 3])
    tryRevealHoleCards()
  }

  // ── share exchange ────────────────────────────────────────

  function sendShares(positions: number[]) {
    // C5 FIX: for hole cards (positions 0-3), only send shares for
    // the OPPONENT's cards. Never send your own hole card shares.
    // Each player keeps their own shares private.
    const mySeatPositions = isHost ? [0, 1] : [2, 3]

    const shares: Record<number, string> = {}
    for (const pos of positions) {
      const share = shuffleKeys.decrypt_share(shuffleState, pos)
      if (share) {
        myShares.set(pos, share) // always store locally
        // only SEND if it's not our own hole card position
        if (!mySeatPositions.includes(pos)) {
          shares[pos] = share
        }
      }
    }
    if (Object.keys(shares).length > 0) {
      send({ t: 'reveal', d: { shares } })
    }
  }

  function onRevealShares(d: any) {
    const shares = d.shares as Record<string, string>
    for (const [pos, share] of Object.entries(shares)) {
      oppShares.set(Number(pos), share)
    }
    tryRevealHoleCards()
    tryRevealCommunity()
  }

  function revealPositions(positions: number[]): number[] | null {
    for (const i of positions) {
      if (!myShares.has(i) || !oppShares.has(i)) return null
    }
    const revealed: number[] = []
    for (const i of positions) {
      const hostShare = isHost ? myShares.get(i)! : oppShares.get(i)!
      const guestShare = isHost ? oppShares.get(i)! : myShares.get(i)!
      const cardIdx = shuffleState.reveal_card(i, hostShare, guestShare)
      if (cardIdx < 0) return null
      revealed.push(cardIdx)
    }
    return revealed
  }

  function tryRevealHoleCards() {
    if (shuffleReady) return

    // C5 FIX: only reveal OUR hole cards, not opponent's
    // we have both shares for our positions (our own + opponent sent theirs)
    // we do NOT have both shares for opponent's positions (we only have our own)
    const myPositions: [number, number] = isHost ? [0, 1] : [2, 3]

    // check if we have both shares for our positions
    for (const p of myPositions) {
      if (!myShares.has(p) || !oppShares.has(p)) return
    }

    const myRevealed = revealPositions(myPositions)
    if (!myRevealed) return
    shuffleReady = true

    const myCards: [number, number] = [myRevealed[0]!, myRevealed[1]!]
    // opponent's cards are UNKNOWN until showdown — as it should be
    const oppCards: [number, number] = [255, 255] // sentinel: unknown

    cb.onLog('shuffle: hole cards revealed (zk)')
    cb.onMsg({ type: 'Status', phase: 'dealing', message: 'deck verified — dealing...' })
    setTimeout(() => cb.onDeal(myCards, oppCards, communityCards), 1500)
  }

  // ── community card reveals (per phase) ────────────────────

  function revealCommunity(phase: string) {
    if (!shuffleState || !shuffleKeys) return
    let positions: number[] = []
    if (phase === 'flop') positions = [4, 5, 6]
    else if (phase === 'turn') positions = [7]
    else if (phase === 'river') positions = [8]
    else return
    sendShares(positions)
    // also tell peer what phase we advanced to
    send({ t: 'phase', d: { phase } })
    tryRevealCommunity()
  }

  function tryRevealCommunity() {
    if (communityRevealed < 3 && myShares.has(4) && oppShares.has(4)) {
      const flop = revealPositions([4, 5, 6])
      if (flop) {
        communityCards[0] = flop[0]!; communityCards[1] = flop[1]!; communityCards[2] = flop[2]!
        communityRevealed = 3
        cb.onLog('shuffle: flop revealed')
        cb.onCommunityRevealed('flop', flop.map(cardToJson))
      }
    }
    if (communityRevealed >= 3 && communityRevealed < 4 && myShares.has(7) && oppShares.has(7)) {
      const turn = revealPositions([7])
      if (turn) {
        communityCards[3] = turn[0]!
        communityRevealed = 4
        cb.onLog('shuffle: turn revealed')
        cb.onCommunityRevealed('turn', communityCards.slice(0, 4).map(cardToJson))
      }
    }
    if (communityRevealed >= 4 && communityRevealed < 5 && myShares.has(8) && oppShares.has(8)) {
      const river = revealPositions([8])
      if (river) {
        communityCards[4] = river[0]!
        communityRevealed = 5
        cb.onLog('shuffle: river revealed')
        cb.onCommunityRevealed('river', communityCards.slice(0, 5).map(cardToJson))
      }
    }
  }

  // ── message routing ───────────────────────────────────────

  function handle(msg: WireMsg): boolean {
    switch (msg.t) {
      case 'shuffle_pk': {
        const d = msg.d as any
        const incomingHand = d.hand ?? 0
        const isNewHand = incomingHand > handId

        if (isNewHand) {
          // opponent initiated a new hand — reset and respond
          handId = incomingHand
          reset()
          oppPkHex = d.pk
          if (available) {
            shuffleKeys = new ShuffleKeysClass()
            myPkHex = shuffleKeys.public_key_hex()
            cb.onLog('shuffle: responding with key (hand ' + handId + ')')
            send({ t: 'shuffle_pk', d: { pk: myPkHex, hand: handId } })
          }
        } else {
          // reply to our own shuffle_pk (same hand) — just store their key
          oppPkHex = d.pk
          cb.onLog('shuffle: got opponent key')
        }
        maybeContinue()
        return true
      }
      case 'shuffle_init':
        onShuffleInit(msg.d)
        return true
      case 'shuffle_done':
        onShuffleDone(msg.d)
        return true
      case 'reveal':
        onRevealShares(msg.d)
        return true
      case 'phase': {
        // peer advanced phase — send our community shares
        const pd = msg.d as { phase: string }
        if (shuffleState && shuffleKeys) {
          let positions: number[] = []
          if (pd.phase === 'flop') positions = [4, 5, 6]
          else if (pd.phase === 'turn') positions = [7]
          else if (pd.phase === 'river') positions = [8]
          if (positions.length > 0) sendShares(positions)
          tryRevealCommunity()
          return true
        }
        return false // let game handle plaintext phase messages
      }
      default:
        return false
    }
  }

  return {
    handle,
    beginDeal,
    revealCommunity,
    available,
    community: () => communityCards,
  }
}
