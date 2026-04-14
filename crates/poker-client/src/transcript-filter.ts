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

export interface Transcript {
  /** append an action to the log */
  record: (entry: Omit<TranscriptEntry, 'localTs'>) => void
  /** get the full log */
  entries: () => readonly TranscriptEntry[]
  /** hash of the transcript (for dispute) */
  hash: () => Promise<string>
  /** reset for new hand */
  reset: () => void
  /** check if opponent exceeded timeout between last action and now */
  checkTimeout: (timeoutMs: number) => { exceeded: boolean; elapsed: number; lastRelayTs: number }
}

export function createTranscript(): Transcript {
  let log: TranscriptEntry[] = []

  function record(entry: Omit<TranscriptEntry, 'localTs'>) {
    log.push({ ...entry, localTs: Date.now() })
  }

  function entries(): readonly TranscriptEntry[] {
    return log
  }

  async function hash(): Promise<string> {
    const data = JSON.stringify(log)
    const buf = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(data))
    return Array.from(new Uint8Array(buf)).map(b => b.toString(16).padStart(2, '0')).join('')
  }

  function reset() {
    log = []
  }

  /** check time since last action using relay timestamps */
  function checkTimeout(timeoutMs: number): { exceeded: boolean; elapsed: number; lastRelayTs: number } {
    if (log.length === 0) return { exceeded: false, elapsed: 0, lastRelayTs: 0 }
    const last = log[log.length - 1]!
    const now = Date.now()
    // use relay timestamp if available, fall back to local
    const lastTs = last.relayTs || last.localTs
    const elapsed = now - lastTs
    return { exceeded: elapsed > timeoutMs, elapsed, lastRelayTs: lastTs }
  }

  return { record, entries, hash, reset, checkTimeout }
}
