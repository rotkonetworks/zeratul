import { createSignal, createEffect, For, Show, onMount, onCleanup } from 'solid-js'
import { relayBase } from './config'

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
    blinds: '0.05/0.1',
    sb: 5_000_000, bb: ZEC / 10,   // 0.05 / 0.1 ZEC (~$10 / $20)
    buyin: ZEC,                     // 1 ZEC = ~$200 — tops the clean 10× ladder (was 100 ZEC)
    maxBuyin: 2.5 * ZEC,
    speed: 'normal', timeout: 45,
    color: '#5a2d3d',
    rakeBps: 100, rakeCap: ZEC / 10, // 1% capped at 0.1 ZEC (~$20)
  },
]

type LiveTable = {
  code: string
  players: number
  max_players: number
  waiting: boolean
  access: string
  bot_friendly?: boolean
  staked?: boolean      // real-money (escrow) table vs free
  buyin_zat?: number
  live: boolean
  blinds: string
  hand_number: number
  spectators: number
}

// honest REAL vs FREE pill for a table card — the signal players kept asking for.
function StakeBadge(props: { staked?: boolean; buyin_zat?: number }) {
  return props.staked
    ? <span class="text-9px font-semibold uppercase tracking-wider px-1.5 py-0.5 rounded bg-zec-yellow/15 text-zec-yellow border border-zec-yellow/40">
        real{props.buyin_zat ? ` · ${fmtZec(props.buyin_zat)}` : ''}
      </span>
    : <span class="text-9px font-semibold uppercase tracking-wider px-1.5 py-0.5 rounded bg-white/8 text-white/50 border border-white/15">free</span>
}

export default function Lobby(props: {
  onJoin: (table: Table, name: string, bot: boolean) => void
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
  // cash = real-ZEC private table (deposit + escrow); practice = play-chip table vs bot
  const [playMode, setPlayMode] = createSignal<'cash' | 'practice'>('practice')
  // show the zafu install flow only in-context (real-money intent), never as a landing wall
  const [showInstall, setShowInstall] = createSignal(false)
  // one-line explanatory notice (e.g. "you need the wallet for real money") — clears on action
  const [notice, setNotice] = createSignal('')
  // real-money availability — server reports whether the escrow service is wired.
  // when false the lobby offers practice only (don't advertise what we can't settle).
  const [escrowEnabled, setEscrowEnabled] = createSignal(false)
  onMount(async () => {
    try {
      const cfg = await (await fetch('/api/config')).json()
      setEscrowEnabled(!!cfg.escrow_enabled)
      // land on free-play unless the visitor already has zafu AND real money is live —
      // browsing + practice must never require the extension (funnel: hook first, install later).
      setPlayMode(cfg.escrow_enabled && props.hasWallet ? 'cash' : 'practice')
    } catch { /* leave practice-only */ }
  })
  const isMobile = window.innerWidth <= 640
  const [chatMessages, setChatMessages] = createSignal<{text: string, cls: string}[]>([])
  const [players, setPlayers] = createSignal<{ name: string; ready: boolean }[]>([])
  const [chatInput, setChatInput] = createSignal('')
  // "looking to play" — flags me on everyone's board so opponents know to challenge me
  const [myReady, setMyReady] = createSignal(false)
  // an incoming challenge waiting for my Accept/Decline ({ from, table_code })
  const [incomingChallenge, setIncomingChallenge] = createSignal<{ from: string; table_code: string } | null>(null)
  let lobbyWs: WebSocket | null = null
  // after a /nick, the re-Join makes the server broadcast "<nick> joined the lobby". Suppress
  // that one echo for the renamer so it doesn't read as a stranger appearing.
  let suppressJoinFor = ''
  let chatEl!: HTMLDivElement

  function addChat(text: string, cls = '') {
    setChatMessages(m => [...m.slice(-100), { text, cls }])
  }

  // the exact handle we Joined the lobby with (incl. a generated anon name) — used to
  // hide myself on the board and to keep the same identity when I sit at a table.
  const [myLobbyName, setMyLobbyName] = createSignal('')
  const myHandle = () => myLobbyName() || name().trim() || props.pubkey?.slice(0, 8) || ''

  onMount(() => {
    // connect lobby WebSocket
    // `/lobby`, not `/ws/lobby`: HAProxy routes `/ws*` to the FROST relay.
    // relayBase(): user-selectable relay origin (default same-origin).
    lobbyWs = new WebSocket(`${relayBase()}/lobby`)
    lobbyWs.onmessage = (ev) => {
      try {
        const msg = JSON.parse(ev.data)
        switch (msg.type) {
          case 'Chat': addChat(`${msg.from}: ${msg.text}`); break
          case 'Whisper': addChat(`[${msg.from} → ${msg.to}]: ${msg.text}`, 'text-purple-400'); break
          case 'System':
            if (suppressJoinFor && msg.text === `${suppressJoinFor} joined the lobby`) { suppressJoinFor = ''; break }
            addChat(msg.text, 'text-neutral-500'); break
          case 'Players': setPlayers(msg.players || []); break
          case 'Tables': setLiveTables(msg.tables); break
          case 'Challenge':
            // someone wants to play us — surface an Accept/Decline prompt
            setIncomingChallenge({ from: msg.from, table_code: msg.table_code })
            addChat(`${msg.from} challenged you to a game`, 'text-zec-yellow font-bold')
            break
          case 'ChallengeSent':
            // our challenge minted a table — go sit down and wait for them
            addChat(`waiting for ${msg.to} at the table…`, 'text-zec-yellow')
            props.onJoinCode(msg.table_code, myHandle() || 'anon')
            break
          case 'ChallengeAccepted':
            addChat(`${msg.by} accepted — they're joining your table`, 'text-green-400')
            break
          case 'ChallengeDeclined':
            addChat(`${msg.by} declined your challenge`, 'text-neutral-500')
            break
        }
      } catch {}
    }
    lobbyWs.onopen = () => {
      const n = name() || 'anon' + String(Math.random()*1e5|0).padStart(5,'0')
      setMyLobbyName(n)
      lobbyWs?.send(JSON.stringify({ type: 'Join', name: n }))
    }
  })
  onCleanup(() => lobbyWs?.close())

  // change my lobby handle. The server keys users by name and its Join handler already
  // swaps an existing entry, so a fresh Join with the new name is a clean rename.
  function setNick(raw: string) {
    const nick = raw.trim().replace(/\s+/g, '_').slice(0, 20)
    if (!nick || nick === myHandle()) return
    updateName(nick)
    setMyLobbyName(nick)
    suppressJoinFor = nick
    lobbyWs?.send(JSON.stringify({ type: 'Join', name: nick }))
    if (myReady()) lobbyWs?.send(JSON.stringify({ type: 'Ready', ready: true })) // rejoin resets ready
    addChat(`you are now ${nick}`, 'text-green-400')
  }

  function sendChat(text: string) {
    if (!lobbyWs || !text.trim()) return
    // parse commands
    if (text.startsWith('/nick ') || text.startsWith('/name ')) {
      setNick(text.slice(text.indexOf(' ') + 1))
    } else if (text.startsWith('/w ') || text.startsWith('/msg ')) {
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

  // toggle "looking to play" and tell the server so I appear (highlighted) on the board
  function toggleReady() {
    const next = !myReady()
    setMyReady(next)
    lobbyWs?.send(JSON.stringify({ type: 'Ready', ready: next }))
  }

  // challenge a specific player → server mints a free table and prompts them; we get a
  // ChallengeSent back and auto-sit. This is the core "pick a person to play" action.
  function challenge(who: string) {
    if (!lobbyWs || !who || who === myHandle()) return
    lobbyWs.send(JSON.stringify({ type: 'Challenge', to: who }))
    addChat(`challenging ${who}…`, 'text-zec-yellow')
  }

  function acceptChallenge() {
    const c = incomingChallenge()
    if (!c) return
    lobbyWs?.send(JSON.stringify({ type: 'AcceptChallenge', from: c.from, table_code: c.table_code }))
    setIncomingChallenge(null)
    props.onJoinCode(c.table_code, myHandle() || 'anon')
  }

  function declineChallenge() {
    const c = incomingChallenge()
    if (!c) return
    lobbyWs?.send(JSON.stringify({ type: 'DeclineChallenge', from: c.from }))
    setIncomingChallenge(null)
  }

  // auto-scroll chat
  createEffect(() => {
    chatMessages()
    if (chatEl) chatEl.scrollTop = chatEl.scrollHeight
  })

  // free play runs on an ephemeral key — no extension needed. Only real-money actions
  // require zafu; those route through requireWalletForCash() which surfaces the install flow.
  function requireWalletForCash(): boolean {
    if (props.hasWallet) return true
    // clear, visible feedback — the user needs to know WHY nothing happened.
    setNotice('You need the zafu wallet to play for real money — install it below to continue.')
    setPlayMode('cash')      // pivot the create tab so the install panel appears in-context
    setTab('play')
    setShowInstall(true)
    return false
  }

  function joinTable(i: number, bot: boolean = false) {
    const practice = bot
    if (!practice && !requireWalletForCash()) return
    setNotice('')
    const n = name().trim() || 'anon'
    props.onJoin(TABLES[i], n, bot)
  }

  function joinByCode() {
    // joining by code could be a cash or free table; if no wallet, let them in on an
    // ephemeral key (a cash table will prompt for deposit downstream, which needs zafu).
    const code = inviteCode().trim()
    if (!code) return
    const n = name().trim() || 'anon'
    props.onJoinCode(code, n)
  }

  function joinLive(table: LiveTable) {
    // a REAL-money table needs zafu; tell the user plainly instead of silently no-op'ing.
    if (table.staked && !requireWalletForCash()) return
    setNotice('')
    const n = name().trim() || 'anon'
    props.onJoinCode(table.code, n)
  }

  const waitingTables = () => liveTables().filter(t => t.waiting && t.access === 'public')
  const activeTables = () => liveTables().filter(t => !t.waiting && t.access === 'public')
  const liveStreams = () => liveTables().filter(t => t.live)

  return (
    <div class="w-full m-auto flex flex-col items-center justify-center p-3">
      <div class="w-full max-w-md lg:max-w-xl">
        {/* header */}
        <div class="text-center mb-5">
          <div class="flex items-center justify-center gap-2">
            <div class="text-zec-yellow text-24px lg:text-30px font-bold tracking-wider font-mono">zk.poker</div>
            <span class="text-9px font-semibold uppercase tracking-wider px-1.5 py-0.5 rounded bg-zec-yellow/15 text-zec-yellow border border-zec-yellow/40 align-middle">beta</span>
          </div>
          {/* value prop, not cryptography — sell the game a newcomer understands */}
          <div class="text-white/70 text-13px lg:text-15px mt-1.5 font-medium">
            Heads-up hold'em where you <span class="text-zec-text">see and hear</span> your opponent.
          </div>
          <div class="text-white/38 text-11px mt-1">
            Real faces · real ZEC · play free in your browser first
          </div>
        </div>

        {/* persistent, non-blocking wallet nudge for newcomers — discoverable but never a wall.
            Free play still needs no extension; this just opens the in-context install panel. */}
        <Show when={!props.hasWallet && !showInstall() && !notice()}>
          <button
            onClick={() => setShowInstall(true)}
            class="mb-4 w-full p-2.5 rounded-lg bg-zec-yellow/5 border border-zec-yellow/20 text-zec-yellow/90 text-12px flex items-center justify-center gap-2 hover:bg-zec-yellow/10 transition-colors"
          >
            <span class="i-lucide-wallet w-4 h-4 shrink-0" />
            <span>New here? <span class="font-semibold">Get the zafu wallet</span> — play for real ZEC &amp; invite friends</span>
            <span class="i-lucide-arrow-right w-3.5 h-3.5 shrink-0" />
          </button>
        </Show>

        {/* explanatory notice (e.g. real-money needs the wallet) */}
        <Show when={notice()}>
          <div class="mb-4 p-3 rounded-lg bg-zec-yellow/10 border border-zec-yellow/30 text-zec-yellow text-12px flex items-start gap-2">
            <span class="i-lucide-info w-4 h-4 mt-0.5 shrink-0" />
            <span>{notice()}</span>
          </div>
        </Show>

        {/* incoming challenge — someone in the lobby wants to play us right now */}
        <Show when={incomingChallenge()}>
          <div class="mb-4 p-3 rounded-lg bg-green-500/10 border border-green-500/40 flex items-center justify-between gap-3">
            <div class="text-13px text-white/90">
              <span class="text-green-400 font-semibold">{incomingChallenge()!.from}</span> wants to play you
              <div class="text-11px text-white/45">free heads-up · sit down and see them face to face</div>
            </div>
            <div class="flex gap-2 shrink-0">
              <button class="text-11px px-2 py-1 rounded border border-white/15 text-neutral-400 hover:text-white/80"
                onClick={declineChallenge}>decline</button>
              <button class="text-11px px-3 py-1 rounded bg-green-500 text-black font-semibold hover:bg-green-400"
                onClick={acceptChallenge}>play →</button>
            </div>
          </div>
        </Show>

        {/* Contextual install flow — shown only when the visitor reaches for real money
            (cash mode / explicit install), never as a landing wall. Free play + browsing
            need no extension. The Chrome Web Store build lags on poker features, so we point
            at the latest GitHub beta (crx/zip) for now. */}
        <Show when={!props.hasWallet && (showInstall() || (tab() === 'play' && playMode() === 'cash'))}>
          <div class="p-5 border border-white/10 rounded-xl bg-zec-surface mb-4">
            <div class="text-center mb-4">
              <div class="text-zec-text text-13px font-semibold uppercase tracking-wider mb-1.5">zafu wallet required</div>
              <div class="text-white/60 text-12px leading-relaxed">
                every action is signed with your key. the store build is
                <span class="text-zec-text"> outdated for poker</span> — grab the latest beta from github while the store update lands.
              </div>
            </div>

            <a href="https://github.com/rotkonetworks/zafu/releases/latest" target="_blank"
              class="btn btn-primary w-full flex items-center justify-center text-12px py-2.5 no-underline mb-4">
              download latest beta (.crx / .zip)
            </a>

            {/* install steps */}
            <ol class="text-white/50 text-11px leading-relaxed list-decimal pl-4 space-y-1 mb-4">
              <li>download <span class="text-white/70 font-mono">zafu-beta-*.zip</span> from the release and unzip it</li>
              <li>open <span class="text-white/70 font-mono">chrome://extensions</span> and enable <span class="text-white/70">Developer mode</span></li>
              <li>click <span class="text-white/70">Load unpacked</span> and pick the unzipped folder</li>
              <li>reload this page</li>
            </ol>

            <div class="flex items-center justify-center gap-3 text-11px">
              <a href="https://github.com/rotkonetworks/zafu" target="_blank"
                class="text-white/40 underline hover:text-zec-text">source</a>
              <span class="text-white/20">·</span>
              <a href="https://chromewebstore.google.com/detail/zafu-wallet-beta/bhlogefpcebekhjpomlodifcelldoimn" target="_blank"
                class="text-white/40 underline hover:text-zec-text">web store (outdated)</a>
            </div>
          </div>
        </Show>

        {/* identity — display only; name is set on /{code} */}
        <Show when={props.hasWallet}>
          <div class="flex items-center justify-center gap-2 mb-3">
            <span class="text-11px text-zec-yellow font-mono">{name() || props.pubkey?.slice(0, 8) || 'anon'}</span>
            <Show when={props.pubkey}>
              <span class="text-10px text-neutral-700 font-mono" title={props.pubkey}>
                {props.pubkey!.slice(0, 6)}..
              </span>
            </Show>
          </div>
        </Show>

        {/* tabs */}
        <div class="flex gap-0 mb-3 border-b border-white/10">
          {(['play', 'public', 'invite'] as const).map(t =>
            <button
              class={`flex-1 text-center py-2 text-11px uppercase tracking-wider transition-colors ${
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

        {/* ===== CREATE / JOIN TABLE ===== (visible to everyone; free play needs no wallet) */}
        <Show when={tab() === 'play'}>
          {/* mode toggle — real money vs practice-vs-bot, one clear choice.
              real money only appears when the escrow service is live. */}
          <div class="grid grid-cols-2 gap-1 p-1 mb-3 rounded-xl bg-black/30 border border-white/8">
            <button
              class={`py-2 rounded-lg text-12px font-semibold transition-all duration-150 disabled:opacity-40 disabled:cursor-not-allowed ${playMode() === 'cash' ? 'bg-zec-yellow text-black shadow-[0_2px_12px_rgba(244,183,40,0.25)]' : 'text-white/50 hover:text-white/80'}`}
              disabled={!escrowEnabled()}
              title={escrowEnabled() ? '' : 'real-money tables are offline right now'}
              onClick={() => escrowEnabled() && setPlayMode('cash')}
            >real money{!escrowEnabled() && ' · soon'}</button>
            <button
              class={`py-2 rounded-lg text-12px font-semibold transition-all duration-150 ${playMode() === 'practice' ? 'bg-white/12 text-white/87 border border-white/15' : 'text-white/50 hover:text-white/80'}`}
              onClick={() => setPlayMode('practice')}
            >free play</button>
          </div>

          <div class="text-11px text-white/38 mb-2.5 px-1">
            <Show when={playMode() === 'cash'} fallback={
              'chip-only table — create it, then invite a friend with the link. no deposit, nothing at stake.'
            }>
              creates a private ZEC table — you deposit your buy-in, then share the invite link with an opponent.
            </Show>
          </div>

          <div class="flex flex-col gap-2">
              <For each={TABLES}>
                {(table, i) => (
                  <button
                    class="w-full flex items-center justify-between p-4 bg-zec-surface border border-white/10 rounded-xl transition-all duration-150 hover:border-zec-yellow/50 hover:bg-zec-elevated active:scale-99 group text-left"
                    onClick={() => joinTable(i(), playMode() === 'practice')}
                    title={playMode() === 'cash' ? 'create a real-money private table' : 'chip-only — invite a friend'}
                  >
                    <div>
                      <div class="text-15px font-semibold text-white/87 group-hover:text-zec-text transition-colors">{table.name}</div>
                      <div class="text-12px text-white/50 mt-0.5">{table.blinds} ZEC blinds</div>
                    </div>
                    <div class="text-right">
                      <Show when={playMode() === 'cash'} fallback={
                        <div class="text-12px font-medium text-white/50 uppercase tracking-wider">free play</div>
                      }>
                        <div class="text-14px font-mono tabular-nums text-zec-text">{fmtZec(table.buyin)}</div>
                        <div class="text-11px text-white/38 mt-0.5">buy-in · {table.rakeBps/100}% fee</div>
                      </Show>
                    </div>
                  </button>
                )}
              </For>
            </div>
        </Show>

        {/* ===== PUBLIC TABLES ===== (browsable by anyone — spectate or jump in on free play) */}
        <Show when={tab() === 'public'}>
          <Show when={waitingTables().length > 0} fallback={
            <div class="text-center py-8">
              <div class="text-neutral-600 text-11px mb-2">no public tables waiting</div>
              <div class="text-neutral-700 text-11px">create one from the "create table" tab<br/>or invite a friend</div>
            </div>
          }>
            <div class="text-neutral-500 text-11px uppercase tracking-wider mb-2">waiting for opponent</div>
            <div class="flex flex-col gap-2">
              <For each={waitingTables()}>
                {table => (
                  <button
                    class="flex items-center justify-between p-3 bg-zec-surface border border-white/10 rounded-lg active:border-zec-yellow transition-colors"
                    onClick={() => joinLive(table)}
                  >
                    <div>
                      <div class="flex items-center gap-2">
                        <StakeBadge staked={table.staked} buyin_zat={table.buyin_zat} />
                        <span class="text-11px font-mono text-white/70">{table.code}</span>
                        <Show when={table.bot_friendly}>
                          <span class="text-9px text-sky-300 bg-sky-500/10 border border-sky-400/25 rounded px-1 uppercase tracking-wider" title="Bots are welcome to sit at this table.">bots ok</span>
                        </Show>
                      </div>
                      <div class="text-11px text-neutral-500 mt-0.5">{table.blinds} blinds</div>
                    </div>
                    <div class="flex items-center gap-2">
                      <span class="text-11px text-zec-text font-medium">{table.players}/{table.max_players} · sit →</span>
                    </div>
                  </button>
                )}
              </For>
            </div>
          </Show>

          <Show when={liveStreams().length > 0}>
            <div class="text-neutral-500 text-11px uppercase tracking-wider mb-2 mt-4 flex items-center gap-2">
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
                      <div class="text-10px text-neutral-500">{table.blinds} · hand #{table.hand_number}</div>
                    </div>
                    <div class="flex items-center gap-2">
                      <span class="text-10px text-neutral-500">{table.spectators} watching</span>
                      <span class="text-10px px-1.5 py-0.5 rounded bg-red-900/30 text-red-400">LIVE</span>
                    </div>
                  </button>
                )}
              </For>
            </div>
          </Show>

          <Show when={activeTables().length > 0}>
            <div class="text-neutral-500 text-11px uppercase tracking-wider mb-2 mt-4">in progress</div>
            <div class="flex flex-col gap-1">
              <For each={activeTables()}>
                {table => (
                  <div class="flex items-center justify-between p-2 bg-zec-surface/50 border border-white/10/50 rounded text-neutral-600">
                    <span class="text-10px font-mono">{table.code}</span>
                    <span class="text-10px">{table.blinds} · hand #{table.hand_number}</span>
                    <span class="w-2 h-2 rounded-full bg-green-500" />
                  </div>
                )}
              </For>
            </div>
          </Show>
        </Show>

        {/* ===== INVITE FRIEND ===== */}
        {/* no-wallet fallback: contact invites need zafu, but sharing a table link doesn't */}
        <Show when={tab() === 'invite' && !props.hasWallet}>
          <div class="p-5 text-center">
            <div class="text-white/70 text-13px mb-2">invite a friend — no wallet needed</div>
            <div class="text-white/45 text-12px leading-relaxed mb-4">
              create a free table and share the link. both of you play instantly in the browser.
              installing <span class="text-zec-yellow">zafu</span> unlocks contact invites and real-money tables.
            </div>
            <button class="btn btn-primary w-full text-12px py-2.5"
              onClick={() => { setTab('play'); setPlayMode('practice') }}>
              create a free table to share
            </button>
            <button class="mt-2 text-neutral-500 text-11px underline"
              onClick={() => setShowInstall(true)}>or install zafu for contact invites</button>
          </div>
        </Show>

        <Show when={tab() === 'invite' && props.hasWallet}>
          <div class="p-4">
            {/* invite from contacts */}
            <Show when={props.identity?.pickContacts}>
              <div class="mb-4">
                <div class="text-neutral-500 text-11px uppercase tracking-wider mb-2">invite from contacts</div>
                <button
                  class="w-full p-3 bg-zec-surface border border-white/10 rounded-lg active:border-zec-yellow transition-colors flex items-center gap-3"
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
                    <div class="text-10px text-neutral-500">opens zafu contact picker</div>
                  </div>
                </button>
              </div>
            </Show>

            <div class="text-neutral-400 text-10px mb-4">
              or share a room code with a friend. both players need the <span class="text-zec-yellow">zafu</span> extension.
            </div>

            {/* create private game */}
            <div class="mb-4">
              <div class="text-neutral-500 text-11px uppercase tracking-wider mb-2">create private table</div>
              <div class="flex gap-2">
                <For each={TABLES}>
                  {(table, i) => (
                    <button
                      class="flex-1 p-2 bg-zec-surface border border-white/10 rounded text-center active:border-zec-yellow"
                      onClick={() => joinTable(i())}
                    >
                      <div class="text-10px font-semibold">{table.name}</div>
                      <div class="text-10px text-neutral-600">{table.blinds}</div>
                    </button>
                  )}
                </For>
              </div>
              <div class="text-neutral-700 text-10px mt-1 text-center">
                creates a private table · share the code with your friend
              </div>
            </div>

            {/* join by code */}
            <div>
              <div class="text-neutral-500 text-11px uppercase tracking-wider mb-2">join by code</div>
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

        {/* ===== COMMON LOBBY — who's here, who to play ===== */}
        {/* Un-gated: free-play visitors (no wallet) are exactly who we want mingling here. */}
        <div class="mt-3 border border-white/10 rounded-lg overflow-hidden">
          {/* header + "looking to play" toggle */}
          <div class="flex items-center justify-between px-2.5 py-1.5 bg-neutral-900/50 border-b border-white/10 gap-2">
            <div class="flex items-center gap-2 min-w-0">
              <span class="text-10px text-neutral-400 uppercase tracking-wider shrink-0">lobby · {players().length} online</span>
              <button
                class="text-10px text-neutral-500 hover:text-zec-yellow font-mono truncate"
                title="click to change your name"
                onClick={() => setChatInput('/nick ')}
              >you: {myHandle()} ✎</button>
            </div>
            <button
              class={`text-10px px-2 py-0.5 rounded-full border transition-colors ${
                myReady()
                  ? 'bg-green-500/15 border-green-500/50 text-green-400'
                  : 'border-white/15 text-neutral-400 hover:text-white/80 hover:border-white/30'
              }`}
              onClick={toggleReady}
              title="Flag yourself as looking for a game so others can challenge you"
            >{myReady() ? '● ready to play' : 'ready to play?'}</button>
          </div>

          {/* players board — click a person to challenge them to a free heads-up game */}
          <div class="max-h-40 overflow-y-auto">
            <Show when={players().filter(p => p.name !== myHandle()).length > 0} fallback={
              <div class="text-neutral-600 text-10px py-4 px-3 text-center leading-relaxed">
                nobody else here yet — flip <span class="text-green-400">ready to play</span> and invite a friend,<br/>
                or spin up a table from the <span class="text-white/60">create table</span> tab.
              </div>
            }>
              <For each={players().filter(p => p.name !== myHandle())}>
                {p => (
                  <div class={`flex items-center justify-between px-2.5 py-1.5 border-b border-white/5 ${p.ready ? 'bg-green-500/5' : ''}`}>
                    <span class="flex items-center gap-1.5 min-w-0">
                      <span class={`w-1.5 h-1.5 rounded-full shrink-0 ${p.ready ? 'bg-green-500' : 'bg-neutral-700'}`} />
                      <span class="text-11px font-mono text-white/75 truncate">{p.name}</span>
                      <Show when={p.ready}>
                        <span class="text-9px text-green-400/80 uppercase tracking-wider shrink-0">ready</span>
                      </Show>
                    </span>
                    <span class="flex items-center gap-1 shrink-0">
                      <button class="text-10px px-1.5 py-0.5 rounded text-neutral-500 hover:text-zec-yellow"
                        onClick={() => setChatInput(`/w ${p.name} `)} title={`whisper ${p.name}`}>msg</button>
                      <button class="text-10px px-2 py-0.5 rounded bg-zec-yellow/15 border border-zec-yellow/40 text-zec-yellow hover:bg-zec-yellow/25"
                        onClick={() => challenge(p.name)} title={`challenge ${p.name} to a free heads-up game`}>play →</button>
                    </span>
                  </div>
                )}
              </For>
            </Show>
          </div>

          {/* chat */}
          <div ref={chatEl!} class="h-24 lg:h-36 overflow-y-auto px-2 py-1 font-mono text-11px bg-zec-surface/50 border-t border-white/10">
            <For each={chatMessages()}>
              {m => <div class={`text-neutral-500 leading-relaxed ${m.cls}`}>{m.text}</div>}
            </For>
            <Show when={chatMessages().length === 0}>
              <div class="text-neutral-700 text-10px py-4 text-center">
                say hi · click <span class="text-zec-yellow">play →</span> next to a player to start a game
              </div>
            </Show>
          </div>

          {/* input */}
          <form class="flex border-t border-white/10" onSubmit={e => {
            e.preventDefault()
            sendChat(chatInput())
          }}>
            <input
              class="flex-1 bg-transparent text-11px px-2 py-1.5 text-white outline-none placeholder-neutral-700"
              placeholder="chat · /nick name · /w player hi..."
              value={chatInput()}
              onInput={e => setChatInput(e.currentTarget.value)}
            />
            <button type="submit" class="text-10px px-2 text-neutral-600 hover:text-neutral-400">↵</button>
          </form>
        </div>

        <div class="text-center text-10px text-neutral-700 mt-3 uppercase tracking-widest">
          heads-up nlhe · nested frost escrow · pallas
        </div>
      </div>
    </div>
  )
}
