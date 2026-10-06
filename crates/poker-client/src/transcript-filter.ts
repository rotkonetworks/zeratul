/**
 * Transcript filter — records signed action log for dispute resolution.
 *
 * Every game action is appended with THREE timestamps:
 *   - localTs:  Date.now() on the recorder's machine (untrusted)
 *   - relayTs:  relay-assigned timestamp (neutral third-party clock)
 *   - sig:      ed25519 signature from the actor's session key
 *
 * On dispute, the jury verifies:
 *   1. Each action's signature matches the session key from `seated`
 *   2. Relay timestamps are monotonically increasing
 *   3. Time between consecutive actions doesn't exceed the agreed timeout
 *   4. The action sequence replays correctly in the PVM engine
 *
 * "recordHandletime" — Eriksen §4.3
 */

export interface TranscriptEntry {
  seq: number
  seat: number
  action: string
  amount: number
  sig: string
  sessionPub: string
  /** local timestamp (recorder's clock, untrusted) */
  localTs: number
  /** relay-assigned timestamp (neutral clock for disputes) */
  relayTs: number
}

/** the revealed deal for one hand (seat-ordered hole cards + community), as
 *  card indices 0..51 - the same encoding the engine and escrow use. */
export interface Deal {
  hole: [[number, number], [number, number]]
  community: number[]
}

/** one played hand of a match: its deal + the signed action log for that hand.
 *  Matches the escrow's HandLog (poker-escrow/src/transcript.rs). */
export interface HandLog {
  deal: Deal
  entries: TranscriptEntry[]
}

export interface Transcript {
  /** append an action to a specific hand's log (the 1-based hand number). Keyed
   *  by hand so a signature recorded LATE (signing is async) still lands in its
   *  own hand, not whatever hand happens to be current when it resolves. */
  record: (hand: number, entry: Omit<TranscriptEntry, 'localTs'>) => void
  /** attach a late-arriving signature to an already-recorded entry, matched by
   *  (hand, seq). Signatures are computed and delivered asynchronously, after
   *  the unsigned action, so an entry is often recorded with an empty sig first
   *  and filled in here. No-op if no entry matches (e.g. a rejected action was
   *  never recorded). */
  attachSig: (hand: number, seq: number, sig: string) => void
  /** every action recorded so far, across all hands, in record order. */
  entries: () => readonly TranscriptEntry[]
  /** mark a hand complete and attach its revealed deal. Does NOT clear entries -
   *  a late-arriving signature for this hand can still be recorded into it. */
  finishHand: (hand: number, deal: Deal) => void
  /** the completed hands of the match, in play order, each with entries sorted
   *  by seq - the shape the escrow's settle-by-replay expects. */
  hands: () => readonly HandLog[]
  /** hash of the whole recorded log (for dispute) */
  hash: () => Promise<string>
  /** reset everything for a brand-new MATCH */
  reset: () => void
  /** check if opponent exceeded timeout between the last recorded action and now */
  checkTimeout: (timeoutMs: number) => { exceeded: boolean; elapsed: number; lastRelayTs: number }
}

export function createTranscript(): Transcript {
  // entries bucketed by hand number, so async-recorded signatures land in the
  // right hand regardless of when they resolve.
  let byHand = new Map<number, TranscriptEntry[]>()
  let deals = new Map<number, Deal>()
  let order: number[] = []            // hands in completion order
  let last: TranscriptEntry | null = null // most recent record, for checkTimeout

  function record(hand: number, entry: Omit<TranscriptEntry, 'localTs'>) {
    const full = { ...entry, localTs: Date.now() }
    let bucket = byHand.get(hand)
    if (!bucket) { bucket = []; byHand.set(hand, bucket) }
    bucket.push(full)
    last = full
  }

  function attachSig(hand: number, seq: number, sig: string) {
    const bucket = byHand.get(hand)
    if (!bucket) return
    const entry = bucket.find(e => e.seq === seq)
    if (entry) entry.sig = sig
  }

  function entries(): readonly TranscriptEntry[] {
    return Array.from(byHand.values()).flat()
  }

  function finishHand(hand: number, deal: Deal) {
    deals.set(hand, deal)
    if (!order.includes(hand)) order.push(hand)
  }

  function hands(): readonly HandLog[] {
    return order.map(h => ({
      deal: deals.get(h)!,
      entries: (byHand.get(h) ?? []).slice().sort((a, b) => a.seq - b.seq),
    }))
  }

  async function hash(): Promise<string> {
    const data = JSON.stringify(entries())
    const buf = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(data))
    return Array.from(new Uint8Array(buf)).map(b => b.toString(16).padStart(2, '0')).join('')
  }

  function reset() {
    byHand = new Map()
    deals = new Map()
    order = []
    last = null
  }

  /** check time since the last recorded action using relay timestamps */
  function checkTimeout(timeoutMs: number): { exceeded: boolean; elapsed: number; lastRelayTs: number } {
    if (!last) return { exceeded: false, elapsed: 0, lastRelayTs: 0 }
    const now = Date.now()
    const lastTs = last.relayTs || last.localTs
    const elapsed = now - lastTs
    return { exceeded: elapsed > timeoutMs, elapsed, lastRelayTs: lastTs }
  }

  return { record, attachSig, entries, finishHand, hands, hash, reset, checkTimeout }
}
