/**
 * Multi-table manager.
 *
 * Manages N concurrent poker games. Each table is an independent
 * game instance with its own transport, engine, and state.
 *
 * The renderer (SolidJS 2D or future Bevy/WebGPU 3D) consumes
 * events from the manager. The manager doesn't know about rendering.
 *
 * Architecture:
 *   TableManager → Table[] → { transport, game, engine, shuffle }
 *   Renderer subscribes to table events via callbacks
 *
 * This separation means we can:
 *   1. Run multiple tables in one browser tab
 *   2. Swap renderers without changing game logic
 *   3. Have mobile (1 table) and desktop (N tables) layouts
 */

import { createSignal } from 'solid-js'
import type { ServerMsg } from './types'

export interface TableInfo {
  id: string
  roomCode: string
  numSeats: number
  blinds: string      // "5/10"
  buyin: number
  playerCount: number
  status: 'lobby' | 'waiting' | 'playing' | 'finished'
  mySeat: number
  isActive: boolean   // currently focused/visible
}

export interface TableManagerApi {
  /** all tables */
  tables: () => TableInfo[]
  /** currently active table id */
  activeTableId: () => string | null
  /** switch focus to a table */
  setActive: (id: string) => void
  /** create a new table (returns table id) */
  createTable: (opts: { numSeats: number; buyin: number; sb: number; bb: number }) => string
  /** join an existing table */
  joinTable: (roomCode: string) => string
  /** leave a table */
  leaveTable: (id: string) => void
  /** get message handler for a table */
  getHandler: (id: string) => ((msg: ServerMsg) => void) | null
  /** max tables allowed */
  maxTables: number
}

let nextId = 1

export function createTableManager(maxTables: number = 4): TableManagerApi {
  const [tables, setTables] = createSignal<TableInfo[]>([])
  const [activeId, setActiveId] = createSignal<string | null>(null)
  const handlers = new Map<string, (msg: ServerMsg) => void>()

  function createTable(opts: { numSeats: number; buyin: number; sb: number; bb: number }): string {
    const id = `table-${nextId++}`
    const info: TableInfo = {
      id,
      roomCode: '',
      numSeats: opts.numSeats,
      blinds: `${opts.sb}/${opts.bb}`,
      buyin: opts.buyin,
      playerCount: 1,
      status: 'lobby',
      mySeat: 0,
      isActive: true,
    }

    setTables(prev => {
      const updated = prev.map(t => ({ ...t, isActive: false }))
      return [...updated, info]
    })
    setActiveId(id)
    return id
  }

  function joinTable(roomCode: string): string {
    const id = `table-${nextId++}`
    const info: TableInfo = {
      id,
      roomCode,
      numSeats: 0, // learned from host
      blinds: '?/?',
      buyin: 0,
      playerCount: 0,
      status: 'waiting',
      mySeat: -1,
      isActive: true,
    }

    setTables(prev => {
      const updated = prev.map(t => ({ ...t, isActive: false }))
      return [...updated, info]
    })
    setActiveId(id)
    return id
  }

  function leaveTable(id: string) {
    setTables(prev => prev.filter(t => t.id !== id))
    handlers.delete(id)
    if (activeId() === id) {
      const remaining = tables()
      setActiveId(remaining.length > 0 ? remaining[0]!.id : null)
    }
  }

  function setActive(id: string) {
    setTables(prev => prev.map(t => ({ ...t, isActive: t.id === id })))
    setActiveId(id)
  }

  function updateTable(id: string, update: Partial<TableInfo>) {
    setTables(prev => prev.map(t => t.id === id ? { ...t, ...update } : t))
  }

  function getHandler(id: string): ((msg: ServerMsg) => void) | null {
    if (handlers.has(id)) return handlers.get(id)!

    // create handler that updates table info from game events
    const handler = (msg: ServerMsg) => {
      switch (msg.type) {
        case 'RoomInfo':
          updateTable(id, { roomCode: (msg as any).code })
          break
        case 'HandStarted':
          updateTable(id, { status: 'playing' })
          break
        case 'OpponentJoined':
          updateTable(id, {
            playerCount: (tables().find(t => t.id === id)?.playerCount ?? 0) + 1,
          })
          break
        case 'Error':
          if ((msg as any).message?.includes('match')) {
            updateTable(id, { status: 'finished' })
          }
          break
      }
    }
    handlers.set(id, handler)
    return handler
  }

  return {
    tables,
    activeTableId: activeId,
    setActive,
    createTable,
    joinTable,
    leaveTable,
    getHandler,
    maxTables,
  }
}
