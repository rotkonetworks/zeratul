/**
 * Mobile-first lobby + multi-table menu.
 *
 * Mobile: one table at a time, swipe between tabs
 * Desktop: grid of 1-4 tables side by side
 *
 * This component handles:
 *   - Creating new tables (with rules config)
 *   - Joining tables (via room code or invite link)
 *   - Switching between active tables (tabs)
 *   - Table overview (stacks, blinds, status)
 */

import { createSignal, For, Show } from 'solid-js'
import type { TableManagerApi, TableInfo } from './table-manager'

interface LobbyProps {
  manager: TableManagerApi
  onCreateTable: (opts: { numSeats: number; buyin: number; sb: number; bb: number }) => void
  onJoinTable: (roomCode: string) => void
  onSelectTable: (id: string) => void
}

export function Lobby(props: LobbyProps) {
  const [name, setName] = createSignal('')
  const [joinCode, setJoinCode] = createSignal('')
  const [numSeats, setNumSeats] = createSignal(2)
  const [buyin, setBuyin] = createSignal(1000)
  const [sb, setSb] = createSignal(5)
  const [bb, setBb] = createSignal(10)
  const [showCreate, setShowCreate] = createSignal(false)

  const canCreate = () => props.manager.tables().length < props.manager.maxTables

  return (
    <div class="p-4 max-w-lg mx-auto">
      {/* header */}
      <div class="text-center mb-6">
        <div class="text-zec-yellow text-14px font-bold tracking-wider">poker.zk.bot</div>
        <div class="text-neutral-600 text-9px mt-1">encrypted P2P poker</div>
      </div>

      {/* active tables */}
      <Show when={props.manager.tables().length > 0}>
        <div class="mb-4">
          <div class="text-neutral-500 text-9px uppercase tracking-wider mb-2">your tables</div>
          <div class="flex flex-col gap-1">
            <For each={props.manager.tables()}>
              {(table) => (
                <button
                  class={`flex items-center justify-between px-3 py-2 rounded border text-left w-full ${
                    table.isActive
                      ? 'border-zec-yellow bg-zec-yellow/10 text-white'
                      : 'border-neutral-800 bg-zec-surface text-neutral-400 hover:border-neutral-600'
                  }`}
                  onClick={() => props.onSelectTable(table.id)}
                >
                  <div>
                    <span class="text-11px font-mono">{table.roomCode || 'new table'}</span>
                    <span class="text-9px text-neutral-600 ml-2">{table.blinds} · {table.numSeats}max</span>
                  </div>
                  <div class="flex items-center gap-2">
                    <span class="text-9px text-neutral-500">{table.playerCount} players</span>
                    <span class={`w-2 h-2 rounded-full ${
                      table.status === 'playing' ? 'bg-green-500' :
                      table.status === 'waiting' ? 'bg-zec-yellow animate-pulse' :
                      table.status === 'finished' ? 'bg-red-500' :
                      'bg-neutral-600'
                    }`} />
                  </div>
                </button>
              )}
            </For>
          </div>
        </div>
      </Show>

      {/* join by code */}
      <div class="mb-4">
        <div class="flex gap-1">
          <input
            class="input-field flex-1 text-11px"
            placeholder="room code or invite link"
            value={joinCode()}
            onInput={e => setJoinCode(e.currentTarget.value)}
            onKeyDown={e => { if (e.key === 'Enter' && joinCode().trim()) props.onJoinTable(joinCode().trim()) }}
          />
          <button
            class="btn btn-primary text-11px px-4"
            disabled={!joinCode().trim()}
            onClick={() => props.onJoinTable(joinCode().trim())}
          >join</button>
        </div>
      </div>

      {/* create table */}
      <Show when={canCreate()}>
        <Show when={showCreate()} fallback={
          <button
            class="w-full py-3 rounded border border-dashed border-neutral-700 text-neutral-500 text-11px hover:border-zec-yellow hover:text-zec-yellow"
            onClick={() => setShowCreate(true)}
          >
            + create table
          </button>
        }>
          <div class="border border-neutral-800 rounded p-3 bg-zec-surface">
            <div class="text-neutral-500 text-9px uppercase tracking-wider mb-3">create table</div>

            {/* seats selector */}
            <div class="flex gap-1 mb-3">
              <For each={[2, 3, 6, 9]}>
                {n => (
                  <button
                    class={`flex-1 py-1 rounded text-10px ${
                      numSeats() === n
                        ? 'bg-zec-yellow text-black font-bold'
                        : 'bg-neutral-800 text-neutral-500'
                    }`}
                    onClick={() => setNumSeats(n)}
                  >{n === 2 ? 'heads-up' : n === 3 ? '3-max' : n === 6 ? '6-max' : 'full ring'}</button>
                )}
              </For>
            </div>

            {/* blinds + buyin */}
            <div class="flex gap-2 mb-3">
              <label class="flex-1">
                <span class="text-neutral-600 text-8px">SB</span>
                <input class="input-field w-full text-center text-11px" type="number"
                  value={sb()} onInput={e => setSb(+e.currentTarget.value)} />
              </label>
              <label class="flex-1">
                <span class="text-neutral-600 text-8px">BB</span>
                <input class="input-field w-full text-center text-11px" type="number"
                  value={bb()} onInput={e => setBb(+e.currentTarget.value)} />
              </label>
              <label class="flex-1">
                <span class="text-neutral-600 text-8px">BUY-IN</span>
                <input class="input-field w-full text-center text-11px" type="number"
                  value={buyin()} onInput={e => setBuyin(+e.currentTarget.value)} />
              </label>
            </div>

            <div class="flex gap-2">
              <button class="btn btn-primary flex-1 text-11px"
                onClick={() => {
                  props.onCreateTable({ numSeats: numSeats(), buyin: buyin(), sb: sb(), bb: bb() })
                  setShowCreate(false)
                }}
              >create</button>
              <button class="btn flex-1 text-11px" onClick={() => setShowCreate(false)}>cancel</button>
            </div>
          </div>
        </Show>
      </Show>

      <Show when={!canCreate()}>
        <div class="text-neutral-600 text-9px text-center mt-2">
          max {props.manager.maxTables} tables
        </div>
      </Show>
    </div>
  )
}

/**
 * Table tabs bar for multi-table.
 * Shows at the top on desktop, bottom on mobile.
 */
export function TableTabs(props: {
  tables: TableInfo[]
  activeId: string | null
  onSelect: (id: string) => void
  onClose: (id: string) => void
}) {
  return (
    <div class="flex gap-0.5 px-1 py-0.5 bg-zec-dark overflow-x-auto scrollbar-hide">
      <For each={props.tables}>
        {(table) => (
          <div
            class={`flex items-center gap-1 px-2 py-1 rounded-t text-9px cursor-pointer whitespace-nowrap ${
              table.id === props.activeId
                ? 'bg-zec-surface text-white border-t border-x border-zec-yellow/30'
                : 'bg-transparent text-neutral-600 hover:text-neutral-400'
            }`}
            onClick={() => props.onSelect(table.id)}
          >
            <span class={`w-1.5 h-1.5 rounded-full ${
              table.status === 'playing' ? 'bg-green-500' :
              table.status === 'waiting' ? 'bg-zec-yellow' :
              'bg-neutral-600'
            }`} />
            <span>{table.roomCode || 'new'}</span>
            <span class="text-neutral-700">{table.blinds}</span>
            <button
              class="text-neutral-700 hover:text-red-400 ml-1"
              onClick={(e) => { e.stopPropagation(); props.onClose(table.id) }}
            >×</button>
          </div>
        )}
      </For>
    </div>
  )
}
