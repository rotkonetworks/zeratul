/**
 * Table state for N-player poker.
 *
 * Tracks who's at the table, their seats, stacks, and status.
 * The relay handles message broadcasting — this module tracks
 * the game state across N participants.
 *
 * Replaces the hardcoded mySeat/oppSeat model.
 */

import type { WireMsg } from './transport'

export interface Player {
  /** seat index (0 to numSeats-1) */
  seat: number
  /** display name */
  name: string
  /** session public key (for identity) */
  sessionPub?: string
  /** zafu or anon */
  mode: 'zafu' | 'anon'
  /** is this the local player? */
  isMe: boolean
  /** connected to relay? */
  connected: boolean
}

export interface TableState {
  /** max seats at this table */
  numSeats: number
  /** players by seat index (null = empty seat) */
  seats: (Player | null)[]
  /** local player's seat */
  mySeat: number
  /** who is the host (seat 0 by default) */
  hostSeat: number
}

export interface TableApi {
  /** get current table state */
  state: () => TableState
  /** handle a wire message (seated, left, etc). returns true if handled. */
  handle: (msg: WireMsg) => boolean
  /** sit down at the table. returns assigned seat. */
  sit: (name: string, sessionPub?: string, mode?: 'zafu' | 'anon') => number
  /** stand up from the table */
  leave: (seat: number) => void
  /** get player at seat */
  playerAt: (seat: number) => Player | null
  /** get all active players */
  activePlayers: () => Player[]
  /** is the table full? */
  isFull: () => boolean
  /** number of seated players */
  playerCount: () => number
}

export function createTable(
  numSeats: number,
  send: (msg: WireMsg) => void,
  isHost: boolean,
): TableApi {
  const seats: (Player | null)[] = Array(numSeats).fill(null)
  let mySeat = -1
  const hostSeat = 0

  function nextEmptySeat(): number {
    for (let i = 0; i < numSeats; i++) {
      if (seats[i] === null) return i
    }
    return -1
  }

  function sit(name: string, sessionPub?: string, mode: 'zafu' | 'anon' = 'anon'): number {
    const seat = nextEmptySeat()
    if (seat < 0) return -1 // table full

    const player: Player = { seat, name, sessionPub, mode, isMe: true, connected: true }
    seats[seat] = player
    mySeat = seat

    // announce to others
    send({ t: 'table_sit', d: { seat, name, sessionPub, mode } })
    return seat
  }

  function leave(seat: number) {
    seats[seat] = null
    send({ t: 'table_leave', d: { seat } })
  }

  function handle(msg: WireMsg): boolean {
    switch (msg.t) {
      case 'table_sit': {
        const d = msg.d as any
        if (seats[d.seat] !== null) return true // seat taken
        seats[d.seat] = {
          seat: d.seat,
          name: d.name,
          sessionPub: d.sessionPub,
          mode: d.mode || 'anon',
          isMe: false,
          connected: true,
        }
        return true
      }
      case 'table_leave': {
        const d = msg.d as any
        seats[d.seat] = null
        return true
      }
      case 'table_state': {
        // host broadcasts full table state to new joiners
        const d = msg.d as any
        for (const p of d.players) {
          seats[p.seat] = { ...p, isMe: p.seat === mySeat, connected: true }
        }
        return true
      }
      default:
        return false
    }
  }

  return {
    state: () => ({ numSeats, seats: [...seats], mySeat, hostSeat }),
    handle,
    sit,
    leave,
    playerAt: (seat) => seats[seat] ?? null,
    activePlayers: () => seats.filter((p): p is Player => p !== null),
    isFull: () => seats.every(s => s !== null),
    playerCount: () => seats.filter(s => s !== null).length,
  }
}
