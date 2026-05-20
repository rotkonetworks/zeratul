import { createSignal, createEffect, For, Show, onMount, onCleanup } from 'solid-js'

export type Table = {
  id: number
  name: string
  blinds: string
  sb: number        // in zatoshis (1 ZEC = 100_000_000 zats)
  bb: number
  buyin: number
  maxBuyin: number
  speed: string
  timeout: number
  color: string
  rakeBps: number   // escrow fee in basis points (100 = 1%)
  rakeCap: number   // max fee per pot in zatoshis
}

// 1 ZEC = 100_000_000 zatoshis
const ZEC = 100_000_000
const mZEC = ZEC / 1000  // 0.001 ZEC = 100_000 zats

/** format zatoshis as ZEC for display */
function fmtZec(zats: number): string {
  const zec = zats / ZEC
  if (zec >= 1) return zec.toFixed(1) + ' ZEC'
  if (zec >= 0.01) return zec.toFixed(2) + ' ZEC'
  return zec.toFixed(4) + ' ZEC'
}

// ZEC ≈ $200.  Tiers from $2 → $20,000 buy-in.
export const TABLES: Table[] = [
  {
    id: 0, name: 'Nano',
    blinds: '50/100 zats',
    sb: 50, bb: 100,               // 0.0000005 / 0.000001 ZEC (~$0.0002)
    buyin: 10_000,                  // 0.0001 ZEC (~$0.02) — play money tier
    maxBuyin: 25_000,
    speed: 'normal', timeout: 30,
    color: '#1a3a2d',
    rakeBps: 0, rakeCap: 0,        // free tier, no rake
  },
  {
    id: 1, name: 'Micro',
    blinds: '0.00005/0.0001',
    sb: 5_000, bb: 10_000,         // ~$0.01 / $0.02
    buyin: mZEC,                    // 0.001 ZEC = $0.20 (100bb)
    maxBuyin: 2.5 * mZEC,
    speed: 'normal', timeout: 30,
    color: '#2d5a3d',
    rakeBps: 250, rakeCap: 50_000, // 2.5% capped at 0.0005 ZEC
  },
  {
    id: 2, name: 'Low',
    blinds: '0.0005/0.001',
    sb: 50_000, bb: 100_000,       // ~$0.10 / $0.20
    buyin: 10 * mZEC,              // 0.01 ZEC = $2 (100bb)
    maxBuyin: 25 * mZEC,
    speed: 'normal', timeout: 30,
    color: '#3d5a2d',
    rakeBps: 200, rakeCap: 500_000, // 2% capped at 0.005 ZEC
  },
  {
    id: 3, name: 'Mid',
    blinds: '0.005/0.01',
    sb: 500_000, bb: 1_000_000,    // ~$1 / $2
    buyin: ZEC / 10,               // 0.1 ZEC = $20 (100bb)
    maxBuyin: ZEC / 4,
    speed: 'normal', timeout: 30,
    color: '#5a3d2d',
    rakeBps: 150, rakeCap: 5_000_000, // 1.5% capped at 0.05 ZEC
  },
  {
    id: 4, name: 'High',
    blinds: '0.5/1.0',
    sb: 50_000_000, bb: ZEC,       // ~$100 / $200
    buyin: 100 * ZEC,              // 100 ZEC = $20,000 (100bb)
    maxBuyin: 250 * ZEC,
    speed: 'normal', timeout: 45,
    color: '#5a2d3d',
    rakeBps: 100, rakeCap: ZEC,    // 1% capped at 1 ZEC ($200)
  },
]

type LiveTable = {
  code: string
  players: number
  max_players: number
  waiting: boolean
  access: string
  live: boolean
  blinds: string
  hand_number: number
  spectators: number
}

export default function Lobby(props: {
  onJoin: (table: Table, name: string) => void
  onJoinCode: (code: string, name: string) => void
  onChat?: (msg: string) => void
  hasWallet: boolean
  pubkey?: string  // hex pubkey from zafu
  identity?: { pickContacts?: (opts?: any) => Promise<any[]>; invite?: (handle: string, payload: any) => Promise<any> }
}) {
  // default nickname: first 8 chars of pubkey, or saved custom name
  const defaultName = () => {
    const saved = localStorage.getItem('poker_nickname')
    if (saved) return saved
    if (props.pubkey) return props.pubkey.slice(0, 8)
    return ''
  }
  const [name, setName] = createSignal(defaultName())

  // persist nickname changes
  function updateName(n: string) {
    setName(n)
    if (n && n !== props.pubkey?.slice(0, 8)) {
      localStorage.setItem('poker_nickname', n)
    }
  }
  const [inviteCode, setInviteCode] = createSignal('')
  const [liveTables, setLiveTables] = createSignal<LiveTable[]>([])
  const [tab, setTab] = createSignal<'play' | 'public' | 'invite'>('play')
  const isMobile = window.innerWidth <= 640
  const [chatMessages, setChatMessages] = createSignal<{text: string, cls: string}[]>([])
  const [players, setPlayers] = createSignal<string[]>([])
  const [chatInput, setChatInput] = createSignal('')
  let lobbyWs: WebSocket | null = null
  let chatEl!: HTMLDivElement

  function addChat(text: string, cls = '') {
    setChatMessages(m => [...m.slice(-100), { text, cls }])
  }

  onMount(() => {
    // connect lobby WebSocket
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
    lobbyWs = new WebSocket(`${proto}//${location.host}/ws/lobby`)
    lobbyWs.onmessage = (ev) => {
      try {
        const msg = JSON.parse(ev.data)
        switch (msg.type) {
          case 'Chat': addChat(`${msg.from}: ${msg.text}`); break
          case 'Whisper': addChat(`[${msg.from} → ${msg.to}]: ${msg.text}`, 'text-purple-400'); break
          case 'System': addChat(msg.text, 'text-neutral-500'); break
          case 'Players': setPlayers(msg.names); break
          case 'Tables': setLiveTables(msg.tables); break
          case 'Challenge':
            addChat(`${msg.from} challenges you! table: ${msg.table_code}`, 'text-zec-yellow font-bold')
            break
        }
      } catch {}
    }
    lobbyWs.onopen = () => {
      const n = name() || 'anon' + String(Math.random()*1e5|0).padStart(5,'0')
      lobbyWs?.send(JSON.stringify({ type: 'Join', name: n }))
    }
  })
  onCleanup(() => lobbyWs?.close())

  function sendChat(text: string) {
    if (!lobbyWs || !text.trim()) return
    // parse commands
    if (text.startsWith('/w ') || text.startsWith('/msg ')) {
      const parts = text.slice(text.indexOf(' ') + 1).split(' ')
      const to = parts[0]
      const msg = parts.slice(1).join(' ')
      if (text.startsWith('/w ') && to && msg) {
        lobbyWs.send(JSON.stringify({ type: 'Whisper', to, text: msg }))
      }
    } else if (text.startsWith('/challenge ')) {
      const to = text.slice(11).trim()
      if (to) lobbyWs.send(JSON.stringify({ type: 'Challenge', to }))
    } else {
      lobbyWs.send(JSON.stringify({ type: 'Chat', text }))
    }
    setChatInput('')
  }

  // auto-scroll chat
  createEffect(() => {
    chatMessages()
    if (chatEl) chatEl.scrollTop = chatEl.scrollHeight
  })

  function joinTable(i: number) {
    if (!props.hasWallet) return // zafu required
    const n = name().trim() || 'anon'
    props.onJoin(TABLES[i], n)
  }

  function joinByCode() {
    if (!props.hasWallet) return
    const code = inviteCode().trim()
    if (!code) return
    const n = name().trim() || 'anon'
    props.onJoinCode(code, n)
  }

  function joinLive(table: LiveTable) {
    if (!props.hasWallet) return
    const n = name().trim() || 'anon'
    props.onJoinCode(table.code, n)
  }

  const waitingTables = () => liveTables().filter(t => t.waiting && t.access === 'public')
  const activeTables = () => liveTables().filter(t => !t.waiting && t.access === 'public')
  const liveStreams = () => liveTables().filter(t => t.live)

  return (
    <div class="min-h-screen min-h-[100dvh] flex flex-col items-center justify-center p-2 bg-zec-dark font-sans text-white">
      <div class="w-full max-w-md">
        {/* header */}
        <div class="text-center mb-3">
          <div class="text-zec-yellow text-14px font-bold tracking-wider font-mono">poker.zk.bot</div>
          <div class="text-neutral-600 text-8px uppercase tracking-widest mt-0.5">
            zk-shuffle · co-signed · encrypted
          </div>
        </div>

        {/* wallet gate */}
        <Show when={!props.hasWallet}>
          <div class="text-center p-6 border border-red-900/50 rounded-lg bg-red-900/10 mb-4">
            <div class="text-red-400 text-11px uppercase tracking-wider mb-2">wallet required</div>
            <div class="text-neutral-400 text-10px mb-3">
              install the <span class="text-zec-yellow">zafu</span> browser extension to play.
              every action is signed with your key.
            </div>
            <a href="https://github.com/niconicobar/zafu" target="_blank"
              class="text-9px text-zec-yellow underline">get zafu extension</a>
          </div>
        </Show>

        {/* identity — display only; name is set on /{code} */}
        <Show when={props.hasWallet}>
          <div class="flex items-center justify-center gap-2 mb-3">
            <span class="text-11px text-zec-yellow font-mono">{name() || props.pubkey?.slice(0, 8) || 'anon'}</span>
            <Show when={props.pubkey}>
              <span class="text-7px text-neutral-700 font-mono" title={props.pubkey}>
                {props.pubkey!.slice(0, 6)}..
              </span>
            </Show>
          </div>
        </Show>

        {/* tabs */}
        <div class="flex gap-0 mb-3 border-b border-neutral-800">
          {(['play', 'public', 'invite'] as const).map(t =>
            <button
              class={`flex-1 text-center py-2 text-9px uppercase tracking-wider transition-colors ${
                tab() === t
                  ? 'text-zec-yellow border-b-2 border-zec-yellow'
                  : 'text-neutral-600 hover:text-neutral-400'
              }`}
              onClick={() => setTab(t)}
            >
              {t === 'play' ? 'create table' : t === 'public' ? `tables (${waitingTables().length})${liveStreams().length ? ` · ${liveStreams().length} live` : ''}` : 'invite friend'}
            </button>
          )}
        </div>

        {/* ===== CREATE / JOIN TABLE ===== */}
        <Show when={tab() === 'play' && props.hasWallet}>
          <div class="flex flex-col gap-2">
              <For each={TABLES}>
                {(table, i) => (
                  <button
                    class="flex items-center justify-between p-3 bg-zec-surface border border-neutral-800 rounded-lg active:border-zec-yellow transition-colors"
                    onClick={() => joinTable(i())}
                  >
                    <div>
                      <div class="text-12px font-semibold">{table.name}</div>
                      <div class="text-9px text-neutral-500">{table.blinds} ZEC blinds</div>
                    </div>
                    <div class="text-right">
                      <div class="text-10px font-mono text-zec-yellow">{fmtZec(table.buyin)}</div>
                      <div class="text-7px text-neutral-600">buy-in · {table.rakeBps/100}% fee</div>
                    </div>
                  </button>
                )}
              </For>
            </div>
        </Show>

        {/* ===== PUBLIC TABLES ===== */}
        <Show when={tab() === 'public' && props.hasWallet}>
          <Show when={waitingTables().length > 0} fallback={
            <div class="text-center py-8">
              <div class="text-neutral-600 text-11px mb-2">no public tables waiting</div>
              <div class="text-neutral-700 text-9px">create one from the "create table" tab<br/>or invite a friend</div>
            </div>
          }>
            <div class="text-neutral-500 text-9px uppercase tracking-wider mb-2">waiting for opponent</div>
            <div class="flex flex-col gap-2">
              <For each={waitingTables()}>
                {table => (
                  <button
                    class="flex items-center justify-between p-3 bg-zec-surface border border-neutral-800 rounded-lg active:border-zec-yellow transition-colors"
                    onClick={() => joinLive(table)}
                  >
                    <div>
                      <div class="text-11px font-mono text-white">{table.code}</div>
                      <div class="text-9px text-neutral-500">{table.blinds} blinds</div>
                    </div>
                    <div class="flex items-center gap-2">
                      <span class="text-9px text-neutral-500">{table.players}/{table.max_players}</span>
                      <span class="w-2 h-2 rounded-full bg-zec-yellow animate-pulse" />
                    </div>
                  </button>
                )}
              </For>
            </div>
          </Show>

          <Show when={liveStreams().length > 0}>
            <div class="text-neutral-500 text-9px uppercase tracking-wider mb-2 mt-4 flex items-center gap-2">
              <span class="w-2 h-2 rounded-full bg-red-500 animate-pulse" />
              live games
            </div>
            <div class="flex flex-col gap-2">
              <For each={liveStreams()}>
                {table => (
                  <button
                    class="flex items-center justify-between p-3 bg-zec-surface border border-red-900/30 rounded-lg hover:border-red-700/50 transition-colors"
                    onClick={() => window.open(`/${table.code}/spectate`, '_blank')}
                  >
                    <div>
                      <div class="text-11px font-mono text-white">{table.code}</div>
                      <div class="text-8px text-neutral-500">{table.blinds} · hand #{table.hand_number}</div>
                    </div>
                    <div class="flex items-center gap-2">
                      <span class="text-8px text-neutral-500">{table.spectators} watching</span>
                      <span class="text-8px px-1.5 py-0.5 rounded bg-red-900/30 text-red-400">LIVE</span>
                    </div>
                  </button>
                )}
              </For>
            </div>
          </Show>

          <Show when={activeTables().length > 0}>
            <div class="text-neutral-500 text-9px uppercase tracking-wider mb-2 mt-4">in progress</div>
            <div class="flex flex-col gap-1">
              <For each={activeTables()}>
                {table => (
                  <div class="flex items-center justify-between p-2 bg-zec-surface/50 border border-neutral-800/50 rounded text-neutral-600">
                    <span class="text-10px font-mono">{table.code}</span>
                    <span class="text-8px">{table.blinds} · hand #{table.hand_number}</span>
                    <span class="w-2 h-2 rounded-full bg-green-500" />
                  </div>
                )}
              </For>
            </div>
          </Show>
        </Show>

        {/* ===== INVITE FRIEND ===== */}
        <Show when={tab() === 'invite' && props.hasWallet}>
          <div class="p-4">
            {/* invite from contacts */}
            <Show when={props.identity?.pickContacts}>
              <div class="mb-4">
                <div class="text-neutral-500 text-9px uppercase tracking-wider mb-2">invite from contacts</div>
                <button
                  class="w-full p-3 bg-zec-surface border border-neutral-800 rounded-lg active:border-zec-yellow transition-colors flex items-center gap-3"
                  onClick={async () => {
                    const contacts = await props.identity?.pickContacts?.({ purpose: 'Invite to poker table', max: 3 })
                    if (contacts?.length) {
                      const tableIdx = 0
                      const table = TABLES[tableIdx]
                      // send invites first — they contain the table info
                      const names = contacts.map(c => c.displayName).join(', ')
                      for (const c of contacts) {
                        await props.identity?.invite?.(c.handle, {
                          type: 'poker-table-invite',
                          data: { blinds: table.blinds, name: table.name, sb: table.sb, bb: table.bb, buyin: table.buyin },
                          ttl: 300,
                        })
                      }
                      // then join the table ourselves (creates it with mutuals access)
                      joinTable(tableIdx)
                      props.onChat?.(`invited ${names}`)
                    }
                  }}
                >
                  <div class="w-8 h-8 rounded-full bg-zec-yellow/10 flex items-center justify-center text-zec-yellow text-14px">+</div>
                  <div>
                    <div class="text-11px text-white">pick from address book</div>
                    <div class="text-8px text-neutral-500">opens zafu contact picker</div>
                  </div>
                </button>
              </div>
            </Show>

            <div class="text-neutral-400 text-10px mb-4">
              or share a room code with a friend. both players need the <span class="text-zec-yellow">zafu</span> extension.
            </div>

            {/* create private game */}
            <div class="mb-4">
              <div class="text-neutral-500 text-9px uppercase tracking-wider mb-2">create private table</div>
              <div class="flex gap-2">
                <For each={TABLES}>
                  {(table, i) => (
                    <button
                      class="flex-1 p-2 bg-zec-surface border border-neutral-800 rounded text-center active:border-zec-yellow"
                      onClick={() => joinTable(i())}
                    >
                      <div class="text-10px font-semibold">{table.name}</div>
                      <div class="text-7px text-neutral-600">{table.blinds}</div>
                    </button>
                  )}
                </For>
              </div>
              <div class="text-neutral-700 text-8px mt-1 text-center">
                creates a private table · share the code with your friend
              </div>
            </div>

            {/* join by code */}
            <div>
              <div class="text-neutral-500 text-9px uppercase tracking-wider mb-2">join by code</div>
              <div class="flex gap-2">
                <input
                  class="input-field flex-1 text-11px"
                  placeholder="e.g. 42-ace-bluff"
                  value={inviteCode()}
                  onInput={e => setInviteCode(e.currentTarget.value)}
                  onKeyDown={e => { if (e.key === 'Enter') joinByCode() }}
                />
                <button
                  class="btn btn-primary text-10px px-4"
                  disabled={!inviteCode().trim()}
                  onClick={joinByCode}
                >join</button>
              </div>
            </div>
          </div>
        </Show>

        {/* ===== LOBBY CHAT + PLAYERS ===== */}
        <Show when={props.hasWallet}>
          <div class="mt-3 border border-neutral-800 rounded-lg overflow-hidden">
            {/* player count */}
            <div class="flex items-center justify-between px-2 py-1 bg-neutral-900/50 border-b border-neutral-800">
              <span class="text-8px text-neutral-500 uppercase tracking-wider">lobby chat</span>
              <span class="text-8px text-neutral-600">{players().length} online</span>
            </div>

            {/* messages */}
            <div ref={chatEl!} class="h-28 overflow-y-auto px-2 py-1 font-mono text-9px bg-zec-surface/50">
              <For each={chatMessages()}>
                {m => <div class={`text-neutral-500 leading-relaxed ${m.cls}`}>{m.text}</div>}
              </For>
              <Show when={chatMessages().length === 0}>
                <div class="text-neutral-700 text-8px py-4 text-center">
                  type to chat · /w name msg · /challenge name
                </div>
              </Show>
            </div>

            {/* input */}
            <form class="flex border-t border-neutral-800" onSubmit={e => {
              e.preventDefault()
              sendChat(chatInput())
            }}>
              <input
                class="flex-1 bg-transparent text-9px px-2 py-1.5 text-white outline-none placeholder-neutral-700"
                placeholder="/w player hi · /challenge player · or just chat..."
                value={chatInput()}
                onInput={e => setChatInput(e.currentTarget.value)}
              />
              <button type="submit" class="text-8px px-2 text-neutral-600 hover:text-neutral-400">↵</button>
            </form>
          </div>

          {/* online players */}
          <Show when={players().length > 0}>
            <div class="flex flex-wrap gap-1 mt-2 px-1">
              <For each={players()}>
                {p => (
                  <span class="text-7px px-1.5 py-0.5 rounded bg-neutral-900 border border-neutral-800 text-neutral-500 cursor-pointer hover:text-zec-yellow hover:border-zec-yellow/30"
                    onClick={() => setChatInput(`/w ${p} `)}
                    title={`whisper ${p}`}
                  >{p}</span>
                )}
              </For>
            </div>
          </Show>
        </Show>

        <div class="text-center text-7px text-neutral-700 mt-3 uppercase tracking-widest">
          heads-up nlhe · nested frost escrow · pallas
        </div>
      </div>
    </div>
  )
}
