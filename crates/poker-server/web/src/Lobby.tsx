import { createSignal, createEffect, For, Show, onMount, onCleanup } from 'solid-js'

export type Table = {
  id: number
  name: string
  blinds: string
  sb: number
  bb: number
  buyin: number
  speed: string
  timeout: number
  color: string
}

export const TABLES: Table[] = [
  { id: 1, name: 'Chill',    blinds: '1/2',    sb: 1,  bb: 2,  buyin: 200,  speed: 'slow',   timeout: 60, color: '#2d5a3d' },
  { id: 2, name: 'Standard', blinds: '5/10',   sb: 5,  bb: 10, buyin: 1000, speed: 'normal', timeout: 30, color: '#3d5a2d' },
  { id: 3, name: 'Turbo',    blinds: '10/20',  sb: 10, bb: 20, buyin: 2000, speed: 'fast',   timeout: 15, color: '#5a3d2d' },
  { id: 4, name: 'Hyper',    blinds: '25/50',  sb: 25, bb: 50, buyin: 5000, speed: 'hyper',  timeout: 8,  color: '#5a2d3d' },
]

type LiveTable = {
  code: string
  players: number
  max_players: number
  waiting: boolean
  has_bot: boolean
  blinds: string
  hand_number: number
}

export default function Lobby(props: {
  onJoin: (table: Table, name: string) => void
  onJoinCode: (code: string, name: string) => void
  hasWallet: boolean
  pubkey?: string  // hex pubkey from zafu
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
  const [mode, setMode] = createSignal<'casino' | 'list'>(isMobile ? 'list' : 'casino')

  // casino walk state
  const [px, setPx] = createSignal(180)
  const [py, setPy] = createSignal(260)
  const [target, setTarget] = createSignal<{x:number,y:number}|null>(null)
  const [facing, setFacing] = createSignal<'d'|'u'|'l'|'r'>('d')

  const tpos = [
    { x: 90,  y: 80 },
    { x: 270, y: 80 },
    { x: 90,  y: 190 },
    { x: 270, y: 190 },
  ]

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

  // walk animation
  let walkIv: number
  onMount(() => {
    walkIv = setInterval(() => {
      const t = target()
      if (!t) return
      const dx = t.x - px(), dy = t.y - py()
      const d = Math.sqrt(dx*dx + dy*dy)
      if (d < 3) { setTarget(null); return }
      const s = 2.5
      setPx(x => x + dx/d*s)
      setPy(y => y + dy/d*s)
      if (Math.abs(dx) > Math.abs(dy)) setFacing(dx > 0 ? 'r' : 'l')
      else setFacing(dy > 0 ? 'd' : 'u')
    }, 25)
  })
  onCleanup(() => clearInterval(walkIv))

  // keyboard
  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if (mode() !== 'casino' || tab() !== 'play') return
      if ((e.target as HTMLElement)?.tagName === 'INPUT') return
      const step = 8
      if (e.key === 'ArrowUp' || e.key === 'w') { setPy(y => Math.max(15, y - step)); setFacing('u') }
      if (e.key === 'ArrowDown' || e.key === 's') { setPy(y => Math.min(285, y + step)); setFacing('d') }
      if (e.key === 'ArrowLeft' || e.key === 'a') { setPx(x => Math.max(15, x - step)); setFacing('l') }
      if (e.key === 'ArrowRight' || e.key === 'd') { setPx(x => Math.min(345, x + step)); setFacing('r') }
      if (e.key === 'Enter' || e.key === ' ') {
        const nt = nearTable()
        if (nt !== null) { e.preventDefault(); joinTable(nt) }
      }
    }
    window.addEventListener('keydown', onKey)
    onCleanup(() => window.removeEventListener('keydown', onKey))
  })

  const nearTable = () => {
    for (let i = 0; i < tpos.length; i++) {
      const dx = px() - tpos[i].x, dy = py() - tpos[i].y
      if (Math.sqrt(dx*dx + dy*dy) < 45) return i
    }
    return null
  }

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

  const waitingTables = () => liveTables().filter(t => t.waiting)
  const activeTables = () => liveTables().filter(t => !t.waiting)

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

        {/* name input */}
        <Show when={props.hasWallet}>
          <div class="flex items-center justify-center gap-2 mb-3">
            <input
              class="input-field w-36 text-center text-11px"
              placeholder={props.pubkey?.slice(0, 8) || 'name'}
              maxLength={16}
              spellcheck={false}
              value={name()}
              onInput={e => updateName(e.currentTarget.value)}
              autofocus
            />
            <Show when={props.pubkey}>
              <span class="text-7px text-neutral-700 font-mono" title={props.pubkey}>
                {props.pubkey!.slice(0, 6)}..
              </span>
            </Show>
            <button
              class="text-8px px-2 py-1 rounded border border-neutral-700 text-neutral-500 hover:text-neutral-300"
              onClick={() => setMode(m => m === 'casino' ? 'list' : 'casino')}
            >{mode() === 'casino' ? '☰' : '🎰'}</button>
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
              {t === 'play' ? 'create table' : t === 'public' ? `public (${waitingTables().length})` : 'invite friend'}
            </button>
          )}
        </div>

        {/* ===== PLAY VS BOT ===== */}
        <Show when={tab() === 'play' && props.hasWallet}>
          <Show when={mode() === 'casino'}>
            <div
              class="relative border border-neutral-800 rounded overflow-hidden cursor-pointer select-none mx-auto"
              style="width:360px; height:300px; background:#111113;"
              onClick={e => {
                const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
                setTarget({x: e.clientX - r.left, y: e.clientY - r.top})
              }}
            >
              <div class="absolute inset-0" style="background:repeating-conic-gradient(#18181c 0% 25%,#141417 0% 50%) 0 0/16px 16px" />
              <div class="absolute left-1/2 -translate-x-1/2 top-0 bottom-0 w-16 opacity-30" style="background:linear-gradient(90deg,transparent,rgba(244,183,40,0.06),transparent)" />
              <div class="absolute top-0 left-0 right-0 h-2" style="background:linear-gradient(180deg,#2a2a30,#1a1a1f)" />
              <div class="absolute bottom-0 left-0 right-0 h-2 bg-neutral-800" />
              <div class="absolute top-3 left-1/2 -translate-x-1/2 text-7px text-neutral-600 uppercase tracking-[0.3em] font-bold">poker room</div>

              <For each={TABLES}>
                {(table, i) => {
                  const p = tpos[i()]
                  const near = () => nearTable() === i()
                  return (
                    <div class="absolute" style={`left:${p.x-24}px;top:${p.y-18}px`}>
                      <div
                        class={`w-12 h-9 rounded-lg border-2 flex flex-col items-center justify-center transition-all duration-150 ${near() ? 'border-zec-yellow scale-110' : 'border-neutral-700'}`}
                        style={`background:${table.color};${near() ? 'box-shadow:0 0 16px rgba(244,183,40,0.3)' : ''}`}
                      >
                        <div class="text-7px font-bold text-white/90">{table.blinds}</div>
                        <div class="text-5px text-white/50">{table.speed}</div>
                      </div>
                      {[[-3,-7],[15,-7],[-3,12],[15,12]].map(([cx,cy]) =>
                        <div class="absolute w-2.5 h-2.5 rounded-sm bg-neutral-800 border border-neutral-700" style={`left:${cx+16}px;top:${cy+10}px`} />
                      )}
                      <div class={`text-center text-6px mt-0.5 uppercase tracking-wider ${near() ? 'text-zec-yellow' : 'text-neutral-700'}`}>{table.name}</div>
                    </div>
                  )
                }}
              </For>

              {/* player avatar */}
              <div class="absolute z-10 transition-none" style={`left:${px()-8}px;top:${py()-14}px`}>
                <div class="w-4 h-5 rounded-t-full bg-zec-yellow border border-zec-gold relative">
                  <div class="absolute w-1 h-1 bg-black rounded-full"
                    style={`left:${facing()==='l'?2:facing()==='r'?6:3}px;top:${facing()==='u'?2:5}px`} />
                  <div class="absolute w-1 h-1 bg-black rounded-full"
                    style={`left:${facing()==='l'?4:facing()==='r'?8:7}px;top:${facing()==='u'?2:5}px`} />
                </div>
                <div class="flex justify-center gap-px">
                  <div class="w-1.5 h-1.5 bg-neutral-600 rounded-b" />
                  <div class="w-1.5 h-1.5 bg-neutral-600 rounded-b" />
                </div>
                <div class="absolute -top-3 left-1/2 -translate-x-1/2 text-6px text-zec-yellow whitespace-nowrap font-bold">{name() || '?'}</div>
              </div>

              <Show when={nearTable() !== null}>
                <div class="absolute bottom-4 left-1/2 -translate-x-1/2 z-20">
                  <button
                    class="bg-zec-yellow text-black text-9px font-bold px-4 py-1 rounded animate-pulse"
                    onClick={e => { e.stopPropagation(); joinTable(nearTable()!) }}
                  >SIT · {TABLES[nearTable()!].name} ({TABLES[nearTable()!].blinds})</button>
                </div>
              </Show>

              <div class="absolute top-20 right-6 w-1.5 h-24 bg-neutral-800 rounded-sm" />
              <div class="absolute top-28 right-3 text-5px text-neutral-700" style="writing-mode:vertical-rl">BAR</div>
            </div>
            <div class="text-center text-neutral-700 text-7px mt-1">click or WASD · approach table · ENTER to sit</div>
          </Show>

          <Show when={mode() === 'list'}>
            <div class="flex flex-col gap-2">
              <For each={TABLES}>
                {(table, i) => (
                  <button
                    class="flex items-center justify-between p-3 bg-zec-surface border border-neutral-800 rounded-lg active:border-zec-yellow transition-colors"
                    onClick={() => joinTable(i())}
                  >
                    <div>
                      <div class="text-12px font-semibold">{table.name}</div>
                      <div class="text-9px text-neutral-500">{table.blinds} · {table.buyin} buy-in</div>
                    </div>
                    <div class="text-right">
                      <div class={`text-9px px-2 py-0.5 rounded inline-block ${
                        table.speed === 'slow' ? 'bg-green-900/30 text-green-400' :
                        table.speed === 'normal' ? 'bg-blue-900/30 text-blue-400' :
                        table.speed === 'fast' ? 'bg-orange-900/30 text-orange-400' :
                        'bg-red-900/30 text-red-400'
                      }`}>{table.timeout}s</div>
                      <div class="text-7px text-neutral-600 mt-0.5">open seat</div>
                    </div>
                  </button>
                )}
              </For>
            </div>
          </Show>
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
            <div class="text-neutral-400 text-10px mb-4">
              share a room code with a friend. both players need the <span class="text-zec-yellow">zafu</span> extension.
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
