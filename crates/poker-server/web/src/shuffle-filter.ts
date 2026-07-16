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
 * Trustless properties (enforced by the wasm layer):
 *  - each player proves possession of their shuffle key (rogue-key defence)
 *  - the initial deck is canonical and verified equal on both sides
 *  - every shuffle carries a Chaum-Pedersen proof that is verified
 *  - every decryption share carries a DLEQ proof that is verified at reveal
 *
 * Wire messages handled: shuffle_pk, shuffle_init, shuffle_done, reveal, phase
 * Wire messages emitted:  shuffle_pk, shuffle_init, shuffle_done, reveal, phase
 */

import type { WireMsg } from './service'
import type { ServerMsg, CardJson } from './types'

const RANKS = ['2','3','4','5','6','7','8','9','T','J','Q','K','A']
const SUITS = ['s','h','d','c']
function cardToJson(idx: number): CardJson {
  return { rank: RANKS[idx % 13]!, suit: SUITS[Math.floor(idx / 13)]! }
}

/** a decryption share together with its DLEQ proof (hex) */
interface Share {
  share: string
  proof: string
}

/** parse the JSON returned by wasm decrypt_share, or null if it failed */
function parseShare(json: string | undefined | null): Share | null {
  if (!json) return null
  try {
    const o = JSON.parse(json)
    if (typeof o?.share === 'string' && typeof o?.proof === 'string') return o as Share
  } catch { /* fall through */ }
  return null
}

export interface ShuffleCallbacks {
  onMsg: (msg: ServerMsg) => void
  onLog: (text: string) => void
  /** called when hole cards are ready and game can deal */
  onDeal: (myCards: [number, number], oppCards: [number, number], community: number[]) => void
  /** called when community cards for a phase are revealed */
  onCommunityRevealed: (phase: string, cards: CardJson[]) => void
  /** called at showdown once the opponent's hole cards are revealed AND their
   *  DLEQ proofs verify against the committed deck. NOT the asserted plaintext —
   *  a forged share never reaches this callback (reveal returns -1 → abort). */
  onShowdownReveal: (oppCards: [number, number]) => void
}

export interface ShuffleApi {
  /** handle an inbound shuffle message. returns true if handled. */
  handle: (msg: WireMsg) => boolean
  /** start a new shuffle ceremony (host calls this) */
  beginDeal: () => void
  /** reveal community cards for a phase advance */
  revealCommunity: (phase: string) => void
  /** at showdown: release our own hole-card shares so the opponent can verify
   *  our hand against the committed deck. Their cards arrive (verified) via the
   *  onShowdownReveal callback once they do the same. */
  revealShowdown: () => void
  /** true if shuffle WASM is available */
  available: boolean
  /** the shuffleState (for engine deal with community placeholders) */
  community: () => number[]
  /** current monotonic hand counter — sent to a reconnecting peer so it re-syncs */
  currentHandId: () => number
  /** raise the local hand counter to at least `n` (reconnect resync: the reconnecting
   *  host must deal at a handId ABOVE the still-connected peer's, or the peer's
   *  `isNewHand` check rejects the fresh deal). No-op if `n` is not higher. */
  setHandIdBaseline: (n: number) => void
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
  let myPopHex = ''
  let oppPkHex = ''
  let oppPopHex = ''
  let myShares = new Map<number, Share>()
  let oppShares = new Map<number, Share>()
  let shuffleReady = false
  let oppHoleRevealed = false
  let communityRevealed = 0
  let communityCards = [0, 0, 0, 0, 0]
  let handId = 0 // increments each deal to distinguish new hands

  function reset() {
    shuffleKeys = null
    shuffleState = null
    myPkHex = ''
    myPopHex = ''
    oppPkHex = ''
    oppPopHex = ''
    myShares = new Map()
    oppShares = new Map()
    shuffleReady = false
    oppHoleRevealed = false
    communityRevealed = 0
    communityCards = [0, 0, 0, 0, 0]
  }

  // own vs opponent hole-card deck positions (heads-up: host=A holds 0,1)
  const myHolePositions: [number, number] = isHost ? [0, 1] : [2, 3]
  const oppHolePositions: [number, number] = isHost ? [2, 3] : [0, 1]

  /** create a fresh per-hand keypair + proof of possession */
  function freshKeys() {
    shuffleKeys = new ShuffleKeysClass()
    myPkHex = shuffleKeys.public_key_hex()
    myPopHex = shuffleKeys.prove_possession()
  }

  function abort(reason: string) {
    cb.onLog(`shuffle: ${reason}`)
    cb.onMsg({ type: 'Error', message: reason })
  }

  // ── shuffle ceremony ──────────────────────────────────────

  function beginDeal() {
    if (!available) return
    handId++
    reset()
    freshKeys()
    cb.onLog('shuffling deck...')
    cb.onMsg({ type: 'Status', phase: 'shuffling', message: 'exchanging keys...' })
    send({ t: 'shuffle_pk', d: { pk: myPkHex, pop: myPopHex, hand: handId } })
    maybeContinue()
  }

  function maybeContinue() {
    if (!shuffleKeys || !oppPkHex || !oppPopHex) return
    if (isHost) hostShuffle()
  }

  function hostShuffle() {
    // constructor verifies both proofs of possession — a rogue key throws here
    try {
      shuffleState = new ShuffleStateClass(myPkHex, oppPkHex, myPopHex, oppPopHex)
    } catch (e: any) {
      abort(`key verification failed — opponent may be cheating: ${e}`)
      return
    }
    const preDeck = shuffleState.deck_hex()
    cb.onMsg({ type: 'Status', phase: 'shuffling', message: 'shuffling deck...' })
    const result = JSON.parse(shuffleState.shuffle_and_prove(0))
    cb.onMsg({ type: 'Status', phase: 'shuffling', message: 'proving shuffle...' })
    send({ t: 'shuffle_init', d: {
      pk_a: myPkHex, pk_b: oppPkHex, pop_a: myPopHex, pop_b: oppPopHex,
      pre_deck: preDeck, deck: result.deck, proof: result.proof,
    } })
  }

  function onShuffleInit(d: any) {
    // guest creates state from the INITIAL (pre-shuffle) deck so the
    // transcript matches the host's before proving. the constructor also
    // verifies both proofs of possession and that the initial deck is canonical.
    try {
      shuffleState = ShuffleStateClass.from_initial_deck(d.pk_a, d.pk_b, d.pop_a, d.pop_b, d.pre_deck)
    } catch (e: any) {
      abort(`deck/key verification failed — opponent may be cheating: ${e}`)
      return
    }

    // verify host's shuffle proof (Chaum-Pedersen)
    try {
      const valid = shuffleState.verify_and_apply(d.deck, d.proof)
      if (!valid) {
        abort('host shuffle proof FAILED — deck rejected')
        return
      }
      cb.onMsg({ type: 'Status', phase: 'shuffling', message: 'host shuffle verified' })
      cb.onLog('shuffle: host proof VERIFIED ✓')
    } catch (e: any) {
      abort(`shuffle verification error: ${e}`)
      return
    }

    cb.onLog('shuffle: guest shuffling...')
    const result = JSON.parse(shuffleState.shuffle_and_prove(1))
    cb.onLog('shuffle: guest done')
    send({ t: 'shuffle_done', d: { deck: result.deck, proof: result.proof } })
    sendShares([0, 1, 2, 3])
    tryRevealHoleCards()
  }

  function onShuffleDone(d: any) {
    if (!shuffleState) return
    // host verifies guest's shuffle proof (Chaum-Pedersen)
    try {
      const valid = shuffleState.verify_and_apply(d.deck, d.proof)
      if (!valid) {
        abort('guest shuffle proof FAILED — deck rejected')
        return
      }
      cb.onMsg({ type: 'Status', phase: 'shuffling', message: 'guest shuffle verified' })
      cb.onLog('shuffle: guest proof VERIFIED ✓')
    } catch (e: any) {
      abort(`shuffle verification error: ${e}`)
      return
    }

    sendShares([0, 1, 2, 3])
    tryRevealHoleCards()
  }

  // ── share exchange ────────────────────────────────────────

  function sendShares(positions: number[]) {
    // C5: for hole cards (positions 0-3), only send shares for the
    // OPPONENT's cards. Never send your own hole-card shares, or the
    // opponent could reconstruct your hand.
    const mySeatPositions = isHost ? [0, 1] : [2, 3]

    const shares: Record<number, Share> = {}
    for (const pos of positions) {
      const parsed = parseShare(shuffleKeys.decrypt_share(shuffleState, pos))
      if (parsed) {
        myShares.set(pos, parsed) // always store locally
        // only SEND if it's not our own hole-card position
        if (!mySeatPositions.includes(pos)) {
          shares[pos] = parsed
        }
      }
    }
    if (Object.keys(shares).length > 0) {
      send({ t: 'reveal', d: { shares } })
    }
  }

  function onRevealShares(d: any) {
    const shares = d.shares as Record<string, Share>
    for (const [pos, share] of Object.entries(shares)) {
      if (share && typeof share.share === 'string' && typeof share.proof === 'string') {
        oppShares.set(Number(pos), share)
      }
    }
    tryRevealHoleCards()
    tryRevealCommunity()
    tryRevealOppHoleCards()
  }

  function revealPositions(positions: number[]): number[] | null {
    for (const i of positions) {
      if (!myShares.has(i) || !oppShares.has(i)) return null
    }
    const revealed: number[] = []
    for (const i of positions) {
      // host is player A (pk_a), guest is player B (pk_b): the wasm verifies
      // each share's DLEQ proof against the matching public key, so the a/b
      // assignment must match how the deck was constructed.
      const host = isHost ? myShares.get(i)! : oppShares.get(i)!
      const guest = isHost ? oppShares.get(i)! : myShares.get(i)!
      const cardIdx = shuffleState.reveal_card(i, host.share, host.proof, guest.share, guest.proof)
      if (cardIdx < 0) return null // bad share or invalid DLEQ proof
      revealed.push(cardIdx)
    }
    return revealed
  }

  function tryRevealHoleCards() {
    if (shuffleReady) return

    // C5: only reveal OUR hole cards. we hold both shares for our positions
    // (our own + the opponent's, which they sent); we never hold both shares
    // for the opponent's positions, so we can't see their hand.
    const myPositions: [number, number] = isHost ? [0, 1] : [2, 3]

    for (const p of myPositions) {
      if (!myShares.has(p) || !oppShares.has(p)) return
    }

    const myRevealed = revealPositions(myPositions)
    if (!myRevealed) {
      abort('hole-card reveal failed — invalid decryption proof')
      return
    }
    shuffleReady = true

    const myCards: [number, number] = [myRevealed[0]!, myRevealed[1]!]
    // opponent's cards stay UNKNOWN until showdown — as it should be
    const oppCards: [number, number] = [255, 255] // sentinel: unknown

    cb.onLog('shuffle: hole cards revealed (zk)')
    cb.onMsg({ type: 'Status', phase: 'dealing', message: 'deck verified — dealing...' })
    setTimeout(() => cb.onDeal(myCards, oppCards, communityCards), 1500)
  }

  // ── showdown: reveal our own hole cards to the opponent ───

  function revealShowdown() {
    if (!shuffleState || !shuffleKeys) return
    // release the shares we withheld during the hand (C5) for OUR hole
    // positions, so the opponent can bind our claimed cards to the deck.
    const shares: Record<number, Share> = {}
    for (const pos of myHolePositions) {
      const parsed = parseShare(shuffleKeys.decrypt_share(shuffleState, pos))
      if (parsed) {
        myShares.set(pos, parsed)
        shares[pos] = parsed
      }
    }
    if (Object.keys(shares).length > 0) {
      send({ t: 'reveal', d: { shares } })
    }
    // the opponent's shares may already be here (they revealed first)
    tryRevealOppHoleCards()
  }

  function tryRevealOppHoleCards() {
    if (oppHoleRevealed) return
    // need BOTH shares for the opponent's hole positions: ours (computed at
    // deal) plus theirs (sent only now, at showdown).
    for (const p of oppHolePositions) {
      if (!myShares.has(p) || !oppShares.has(p)) return
    }
    const revealed = revealPositions(oppHolePositions)
    if (!revealed) {
      // a share whose DLEQ proof does not verify against the committed deck:
      // the opponent tried to claim cards they were not dealt. Refuse.
      abort('showdown reveal failed — opponent decryption proof invalid')
      return
    }
    oppHoleRevealed = true
    cb.onLog('shuffle: opponent hole cards revealed + verified (zk)')
    cb.onShowdownReveal([revealed[0]!, revealed[1]!])
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
          oppPopHex = d.pop ?? ''
          if (available) {
            freshKeys()
            cb.onLog('shuffle: responding with key (hand ' + handId + ')')
            send({ t: 'shuffle_pk', d: { pk: myPkHex, pop: myPopHex, hand: handId } })
          }
        } else {
          // reply to our own shuffle_pk (same hand) — just store their key + pop
          oppPkHex = d.pk
          oppPopHex = d.pop ?? ''
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
    revealShowdown,
    available,
    community: () => communityCards,
    currentHandId: () => handId,
    setHandIdBaseline: (n: number) => { if (n > handId) handId = n },
  }
}
