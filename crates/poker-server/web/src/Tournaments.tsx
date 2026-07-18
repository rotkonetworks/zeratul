import { createSignal, createEffect, onCleanup, onMount, For, Show } from 'solid-js'

/**
 * FREE (chip-only) heads-up TOURNAMENT UI.
 *
 * Self-contained overlay. Opens on the `#/tournaments` hash route so wiring into the
 * app is a single mount line — the component manages its own visibility and does NOT
 * touch App.tsx state. All backend calls are plain same-origin HTTP JSON (the relay
 * WebSocket layer in config.ts is unrelated to these REST endpoints).
 *
 * Backend contract (server team is building these):
 *   POST /tournaments              {name,organizer,paid,buyin_zat} -> {id}
 *   GET  /tournaments              -> [{id,name,organizer,paid,buyin_zat,state,player_count,sponsor}]
 *   GET  /tournaments/{id}         -> full tournament (see Tournament type)
 *   POST /tournaments/{id}/join    {player}
 *   POST /tournaments/{id}/leave   {player}
 *   POST /tournaments/{id}/start   {who}
 *   POST /tournaments/{id}/sponsor {who,name,logo_url,url,added_prize_zat}
 *   POST /tournaments/{id}/result  {match_id,winner,reporter}
 *
 * "You're up" flow: when polling shows the player has a playable match (they're `a`
 * or `b`, no winner yet), we surface a "Play" button that navigates into the free-play
 * room `match.room` via the app's normal pathname-based join path (go to /{room} then
 * reload — App reads location.pathname on load to enter the room lobby). We remember the
 * match so that when the game ends the client can POST /result.
 */

type Sponsor = {
  name?: string
  logo_url?: string
  url?: string
  added_prize_zat?: number
} | null

type TournamentSummary = {
  id: string
  name: string
  organizer: string
  paid: boolean
  buyin_zat: number
  state: 'registering' | 'running' | 'finished'
  player_count: number
  sponsor?: Sponsor
}

type Match = {
  id: string
  round: number
  a: string | null
  b: string | null
  winner: string | null
  room?: string | null
}

type Tournament = {
  id: string
  name: string
  organizer: string
  paid: boolean
  buyin_zat: number
  state: 'registering' | 'running' | 'finished'
  rounds: number
  sponsor?: Sponsor
  players: string[]
  matches: Match[]
  // some backends echo the summary field on the detail object too — optional.
  player_count?: number
}

// persistent handle — the SAME nickname the rest of the app uses (Lobby.tsx / App.tsx
// read localStorage 'poker_nickname'). Fall back to a stable generated anon handle so a
// player can still create/join even without a set nickname.
function playerHandle(): string {
  const saved = localStorage.getItem('poker_nickname')
  if (saved && saved.trim()) return saved.trim()
  let anon = localStorage.getItem('poker_tourney_anon')
  if (!anon) {
    anon = 'anon' + String((Math.random() * 1e5) | 0).padStart(5, '0')
    try { localStorage.setItem('poker_tourney_anon', anon) } catch { /* ignore */ }
  }
  return anon
}

const ZEC = 100_000_000
function fmtZec(zats: number): string {
  const z = zats / ZEC
  if (z >= 1) return z.toFixed(1) + ' ZEC'
  if (z >= 0.01) return z.toFixed(2) + ' ZEC'
  return z.toFixed(4) + ' ZEC'
}

async function api<T>(path: string, init?: RequestInit): Promise<T | null> {
  try {
    const resp = await fetch(path, {
      ...init,
      headers: { 'content-type': 'application/json', ...(init?.headers || {}) },
    })
    if (!resp.ok) return null
    const text = await resp.text()
    return text ? (JSON.parse(text) as T) : ({} as T)
  } catch {
    return null
  }
}

// key under which App's normal room-join reload path is triggered: we simply set the
// nickname (so the room lobby prefills it) and navigate to /{room}.
function enterRoom(room: string, matchId: string, tournamentId: string) {
  // remember which tournament match this room belongs to so that, when the free-play
  // game ends, the client can report the result. Read back by the report hook.
  try {
    localStorage.setItem('poker_tourney_match', JSON.stringify({ tournamentId, matchId, room }))
  } catch { /* ignore */ }
  // reuse the app's pathname-based join: App reads location.pathname on load and drops
  // the player into the room's "sit down" lobby with their saved nickname.
  window.location.assign('/' + room.replace(/^\/+/, ''))
}

// ── sponsor banner ─────────────────────────────────────────────────────────
function SponsorBanner(props: { sponsor: Sponsor }) {
  const s = () => props.sponsor
  return (
    <Show when={s() && (s()!.name || s()!.logo_url)}>
      <a
        href={s()!.url || '#'}
        target={s()!.url ? '_blank' : undefined}
        rel="noopener"
        class="flex items-center justify-center gap-3 p-3 mb-4 rounded-xl bg-zec-yellow/5 border border-zec-yellow/25 no-underline hover:border-zec-yellow/50 transition-colors"
        title={s()!.url || undefined}
      >
        <span class="text-9px text-white/40 uppercase tracking-widest shrink-0">sponsored by</span>
        <Show when={s()!.logo_url}>
          <img src={s()!.logo_url!} alt={s()!.name || 'sponsor'} class="h-7 max-w-32 object-contain" />
        </Show>
        <Show when={s()!.name}>
          <span class="text-13px font-semibold text-zec-text">{s()!.name}</span>
        </Show>
        <Show when={(s()!.added_prize_zat ?? 0) > 0}>
          <span class="text-10px px-1.5 py-0.5 rounded bg-zec-yellow/15 text-zec-yellow border border-zec-yellow/40">
            +{fmtZec(s()!.added_prize_zat!)} prize
          </span>
        </Show>
      </a>
    </Show>
  )
}

// ── bracket ────────────────────────────────────────────────────────────────
function Bracket(props: { t: Tournament; me: string }) {
  const rounds = () => {
    const byRound = new Map<number, Match[]>()
    for (const m of props.t.matches) {
      const arr = byRound.get(m.round) || []
      arr.push(m)
      byRound.set(m.round, arr)
    }
    return [...byRound.entries()].sort((a, b) => a[0] - b[0]).map(([r, ms]) => ({ round: r, matches: ms }))
  }
  const roundLabel = (round: number, total: number) => {
    const fromEnd = total - round
    if (fromEnd === 1) return 'Final'
    if (fromEnd === 2) return 'Semifinals'
    if (fromEnd === 3) return 'Quarterfinals'
    return `Round ${round + 1}`
  }
  const seat = (name: string | null, m: Match) => {
    const tbd = !name
    const won = !!m.winner && m.winner === name
    const lost = !!m.winner && m.winner !== name && !tbd
    const isMe = !!name && name === props.me
    return (
      <div
        class={`flex items-center justify-between px-2 py-1 text-11px ${
          won ? 'text-zec-yellow font-semibold' : lost ? 'text-neutral-600 line-through' : 'text-white/80'
        }`}
      >
        <span class="flex items-center gap-1 min-w-0">
          <Show when={isMe}>
            <span class="w-1.5 h-1.5 rounded-full bg-green-500 shrink-0" title="you" />
          </Show>
          <span class={`truncate font-mono ${tbd ? 'text-neutral-700 italic' : ''}`}>{name || 'TBD'}</span>
        </span>
        <Show when={won}>
          <span class="i-lucide-crown w-3 h-3 text-zec-yellow shrink-0" />
        </Show>
      </div>
    )
  }
  return (
    <div class="overflow-x-auto">
      <div class="flex gap-4 min-w-max pb-2">
        <For each={rounds()}>
          {col => (
            <div class="flex flex-col justify-around gap-3 min-w-40">
              <div class="text-9px text-neutral-500 uppercase tracking-widest text-center">
                {roundLabel(col.round, props.t.rounds)}
              </div>
              <For each={col.matches}>
                {m => (
                  <div class={`rounded-lg border overflow-hidden ${
                    m.winner ? 'border-white/10' : 'border-zec-yellow/25'
                  } bg-zec-surface`}>
                    {seat(m.a, m)}
                    <div class="border-t border-white/8" />
                    {seat(m.b, m)}
                  </div>
                )}
              </For>
            </div>
          )}
        </For>
      </div>
    </div>
  )
}

// ── create form ────────────────────────────────────────────────────────────
function CreateForm(props: { onCreated: (id: string) => void }) {
  const [name, setName] = createSignal('')
  const [busy, setBusy] = createSignal(false)
  const [err, setErr] = createSignal('')
  const create = async () => {
    const n = name().trim()
    if (!n) { setErr('name your tournament'); return }
    setBusy(true)
    setErr('')
    const res = await api<{ id: string }>('/tournaments', {
      method: 'POST',
      body: JSON.stringify({ name: n, organizer: playerHandle(), paid: false, buyin_zat: 0 }),
    })
    setBusy(false)
    if (res && res.id) {
      setName('')
      props.onCreated(res.id)
    } else {
      setErr('could not create — the tournament service may be offline')
    }
  }
  return (
    <div class="panel p-4 mb-4">
      <div class="text-zec-text text-11px font-semibold uppercase tracking-wider mb-3">create a tournament</div>
      <div class="flex gap-2 flex-wrap">
        <input
          class="input-field flex-1 min-w-40 text-12px"
          placeholder="tournament name"
          maxLength={40}
          value={name()}
          onInput={e => setName(e.currentTarget.value)}
          onKeyDown={e => { if (e.key === 'Enter') create() }}
        />
        <button class="btn btn-primary text-12px px-5" disabled={busy()} onClick={create}>
          {busy() ? 'creating…' : 'create'}
        </button>
      </div>
      <div class="flex items-center gap-3 mt-3">
        <span class="text-10px text-white/40 uppercase tracking-wider">format</span>
        <span class="text-10px px-2 py-0.5 rounded-full bg-white/8 text-white/60 border border-white/15">free · chip-only</span>
        <span
          class="text-10px px-2 py-0.5 rounded-full bg-white/5 text-neutral-600 border border-white/10 cursor-not-allowed"
          title="paid buy-in tournaments are coming soon"
        >paid · soon</span>
      </div>
      <Show when={err()}>
        <div class="mt-2 text-11px text-red-400">{err()}</div>
      </Show>
    </div>
  )
}

// ── tournament detail view ─────────────────────────────────────────────────
function Detail(props: { id: string; me: string; onBack: () => void }) {
  const [t, setT] = createSignal<Tournament | null>(null)
  const [err, setErr] = createSignal('')
  const [busy, setBusy] = createSignal(false)

  const load = async () => {
    const res = await api<Tournament>(`/tournaments/${props.id}`)
    if (res && res.id) { setT(res); setErr('') }
    else if (!t()) setErr('tournament not found (the service may be offline)')
  }
  onMount(load)
  // poll every ~3s for live bracket / state updates
  const iv = setInterval(load, 3000)
  onCleanup(() => clearInterval(iv))

  const joined = () => !!t()?.players.includes(props.me)
  const isOrganizer = () => t()?.organizer === props.me
  // a playable match: this player is a or b, both seats filled, no winner yet, has a room
  const myMatch = (): Match | null => {
    const cur = t()
    if (!cur || cur.state !== 'running') return null
    return cur.matches.find(m =>
      (m.a === props.me || m.b === props.me) && m.a && m.b && !m.winner && m.room,
    ) || null
  }
  const champion = (): string | null => {
    const cur = t()
    if (!cur || cur.state !== 'finished') return null
    // final = highest round, single match with a winner
    const finalRound = Math.max(...cur.matches.map(m => m.round), 0)
    const final = cur.matches.find(m => m.round === finalRound && m.winner)
    return final?.winner || null
  }

  const act = async (path: string, body: Record<string, unknown>) => {
    setBusy(true)
    const res = await api(`/tournaments/${props.id}/${path}`, { method: 'POST', body: JSON.stringify(body) })
    setBusy(false)
    if (res === null) setErr('action failed — the service may be offline')
    else setErr('')
    await load()
  }

  const join = () => act('join', { player: props.me })
  const leave = () => act('leave', { player: props.me })
  const start = () => act('start', { who: props.me })

  return (
    <Show when={t()} fallback={
      <div class="text-center py-10">
        <div class="text-neutral-500 text-12px mb-3">{err() || 'loading…'}</div>
        <button class="btn text-11px px-4" onClick={props.onBack}>← back</button>
      </div>
    }>
      {(cur) => (
        <div>
          <button class="text-11px text-neutral-500 hover:text-zec-yellow mb-3" onClick={props.onBack}>← all tournaments</button>

          <SponsorBanner sponsor={cur().sponsor ?? null} />

          {/* header */}
          <div class="flex items-start justify-between gap-3 mb-4">
            <div>
              <div class="text-18px font-bold text-white/90">{cur().name}</div>
              <div class="text-11px text-neutral-500 mt-0.5">
                by <span class="font-mono text-white/60">{cur().organizer}</span>
                {' · '}<span class="text-white/50">{cur().player_count ?? cur().players.length} players</span>
                {' · '}
                <span class={
                  cur().state === 'running' ? 'text-green-400'
                  : cur().state === 'finished' ? 'text-zec-yellow'
                  : 'text-white/50'
                }>{cur().state}</span>
              </div>
            </div>
            <span class="text-9px font-semibold uppercase tracking-wider px-1.5 py-0.5 rounded bg-white/8 text-white/50 border border-white/15 shrink-0">free</span>
          </div>

          {/* champion screen */}
          <Show when={champion()}>
            <div class="text-center p-6 mb-4 rounded-xl bg-zec-yellow/5 border border-zec-yellow/30">
              <div class="i-lucide-trophy w-8 h-8 text-zec-yellow mx-auto mb-2" />
              <div class="text-10px text-white/50 uppercase tracking-widest mb-1">champion</div>
              <div class="text-22px font-bold text-zec-yellow font-mono">{champion()}</div>
              <Show when={champion() === props.me}>
                <div class="text-12px text-green-400 mt-1">that's you 🏆</div>
              </Show>
            </div>
          </Show>

          {/* "you're up" — playable match ready */}
          <Show when={myMatch()}>
            {(m) => (
              <div class="flex items-center justify-between gap-3 p-4 mb-4 rounded-xl bg-green-500/10 border border-green-500/40 animate-pulse">
                <div>
                  <div class="text-13px text-white/90 font-semibold">Your match is ready</div>
                  <div class="text-11px text-white/50">
                    vs <span class="font-mono">{m().a === props.me ? m().b : m().a}</span>
                  </div>
                </div>
                <button
                  class="btn btn-primary text-13px px-5 py-2 shrink-0"
                  onClick={() => enterRoom(m().room!, m().id, props.id)}
                >Play →</button>
              </div>
            )}
          </Show>

          {/* organizer start (registering only) */}
          <Show when={cur().state === 'registering'}>
            <div class="flex items-center gap-2 mb-4 flex-wrap">
              <Show when={!joined()} fallback={
                <button class="btn text-12px px-4" disabled={busy()} onClick={leave}>leave</button>
              }>
                <button class="btn btn-primary text-12px px-4" disabled={busy()} onClick={join}>join tournament</button>
              </Show>
              <Show when={isOrganizer()}>
                <button
                  class="btn btn-secondary text-12px px-4"
                  disabled={busy() || (cur().player_count ?? cur().players.length) < 2}
                  title={(cur().player_count ?? cur().players.length) < 2 ? 'need at least 2 players' : 'start the tournament'}
                  onClick={start}
                >start tournament</button>
              </Show>
            </div>
          </Show>

          {/* bracket (running / finished) or player list (registering) */}
          <Show when={cur().matches.length > 0} fallback={
            <div>
              <div class="text-10px text-neutral-500 uppercase tracking-wider mb-2">registered players</div>
              <Show when={cur().players.length > 0} fallback={
                <div class="text-neutral-600 text-11px py-4 text-center">no players yet — be the first to join</div>
              }>
                <div class="flex flex-wrap gap-1.5">
                  <For each={cur().players}>
                    {p => (
                      <span class={`text-11px font-mono px-2 py-1 rounded border ${
                        p === props.me ? 'border-green-500/40 text-green-400 bg-green-500/5' : 'border-white/10 text-white/70 bg-zec-surface'
                      }`}>{p}</span>
                    )}
                  </For>
                </div>
              </Show>
            </div>
          }>
            <div class="text-10px text-neutral-500 uppercase tracking-wider mb-2">bracket</div>
            <Bracket t={cur()} me={props.me} />
          </Show>

          <Show when={err()}>
            <div class="mt-3 text-11px text-red-400">{err()}</div>
          </Show>
        </div>
      )}
    </Show>
  )
}

// ── list view ──────────────────────────────────────────────────────────────
function ListView(props: { me: string; onOpen: (id: string) => void }) {
  const [items, setItems] = createSignal<TournamentSummary[]>([])
  const [loaded, setLoaded] = createSignal(false)
  const [offline, setOffline] = createSignal(false)

  const load = async () => {
    const res = await api<TournamentSummary[]>('/tournaments')
    if (res && Array.isArray(res)) { setItems(res); setOffline(false) }
    else if (!loaded()) setOffline(true)
    setLoaded(true)
  }
  onMount(load)
  const iv = setInterval(load, 3000)
  onCleanup(() => clearInterval(iv))

  const open = () => items().filter(t => t.state === 'registering')
  const running = () => items().filter(t => t.state === 'running')
  const finished = () => items().filter(t => t.state === 'finished')

  const Card = (props2: { t: TournamentSummary }) => (
    <button
      class="w-full flex items-center justify-between gap-3 p-3 bg-zec-surface border border-white/10 rounded-lg hover:border-zec-yellow/50 transition-colors text-left"
      onClick={() => props.onOpen(props2.t.id)}
    >
      <div class="flex items-center gap-2 min-w-0">
        <Show when={props2.t.sponsor?.logo_url}>
          <img src={props2.t.sponsor!.logo_url!} alt="" class="h-6 w-6 rounded object-contain shrink-0" />
        </Show>
        <div class="min-w-0">
          <div class="text-13px font-semibold text-white/87 truncate">{props2.t.name}</div>
          <div class="text-10px text-neutral-500 truncate">
            by <span class="font-mono">{props2.t.organizer}</span> · {props2.t.player_count} players
            <Show when={props2.t.sponsor?.name}>{' · '}<span class="text-zec-text">{props2.t.sponsor!.name}</span></Show>
          </div>
        </div>
      </div>
      <div class="flex items-center gap-2 shrink-0">
        <span class="text-9px font-semibold uppercase tracking-wider px-1.5 py-0.5 rounded bg-white/8 text-white/50 border border-white/15">free</span>
        <span class={`text-11px font-medium ${
          props2.t.state === 'running' ? 'text-green-400'
          : props2.t.state === 'finished' ? 'text-zec-yellow'
          : 'text-white/60'
        }`}>{props2.t.state === 'registering' ? 'open →' : props2.t.state}</span>
      </div>
    </button>
  )

  return (
    <div>
      <CreateForm onCreated={props.onOpen} />

      <Show when={offline()}>
        <div class="text-center py-6 text-neutral-600 text-11px">
          tournament service is offline — check back soon
        </div>
      </Show>

      <Show when={open().length > 0}>
        <div class="text-10px text-neutral-500 uppercase tracking-wider mb-2">open · registering</div>
        <div class="flex flex-col gap-2 mb-4">
          <For each={open()}>{t => <Card t={t} />}</For>
        </div>
      </Show>

      <Show when={running().length > 0}>
        <div class="text-10px text-neutral-500 uppercase tracking-wider mb-2 flex items-center gap-2">
          <span class="w-2 h-2 rounded-full bg-green-500 animate-pulse" /> running
        </div>
        <div class="flex flex-col gap-2 mb-4">
          <For each={running()}>{t => <Card t={t} />}</For>
        </div>
      </Show>

      <Show when={finished().length > 0}>
        <div class="text-10px text-neutral-500 uppercase tracking-wider mb-2">finished</div>
        <div class="flex flex-col gap-2 mb-4">
          <For each={finished()}>{t => <Card t={t} />}</For>
        </div>
      </Show>

      <Show when={loaded() && !offline() && items().length === 0}>
        <div class="text-center py-6 text-neutral-600 text-11px">
          no tournaments yet — create the first one above
        </div>
      </Show>
    </div>
  )
}

/**
 * Root overlay. Renders only when the URL hash is `#/tournaments` (optionally
 * `#/tournaments/<id>`). This keeps the wiring into App.tsx to a single mount line
 * and lets the feature be reached from any view via a hash link/button.
 */
export default function Tournaments() {
  const parseHash = () => {
    const h = window.location.hash.replace(/^#/, '')
    if (!h.startsWith('/tournaments')) return { open: false, id: null as string | null }
    const rest = h.slice('/tournaments'.length).replace(/^\/+/, '')
    return { open: true, id: rest || null }
  }
  const [route, setRoute] = createSignal(parseHash())
  const onHash = () => setRoute(parseHash())
  onMount(() => window.addEventListener('hashchange', onHash))
  onCleanup(() => window.removeEventListener('hashchange', onHash))

  const me = playerHandle()
  const open = (id: string) => { window.location.hash = '#/tournaments/' + id }
  const back = () => { window.location.hash = '#/tournaments' }
  const close = () => {
    // clear the hash without leaving a dangling '#'
    history.replaceState(null, '', window.location.pathname + window.location.search)
    setRoute({ open: false, id: null })
  }

  createEffect(() => {
    // lock body scroll while the overlay is open (mirrors app modal behavior)
    if (route().open) document.body.style.overflow = 'hidden'
    else document.body.style.overflow = ''
  })
  onCleanup(() => { document.body.style.overflow = '' })

  return (
    <Show when={route().open}>
      <div class="fixed inset-0 z-50 bg-zec-dark/95 backdrop-blur-sm overflow-y-auto">
        <div class="w-full max-w-2xl mx-auto p-4 pt-6">
          <div class="flex items-center justify-between mb-4">
            <div class="flex items-center gap-2">
              <span class="i-lucide-trophy w-5 h-5 text-zec-yellow" />
              <span class="text-16px font-bold text-zec-text">Tournaments</span>
              <span class="text-9px font-semibold uppercase tracking-wider px-1.5 py-0.5 rounded bg-zec-yellow/15 text-zec-yellow border border-zec-yellow/40">free</span>
            </div>
            <button class="text-neutral-500 hover:text-white text-18px leading-none px-2" onClick={close} title="close">×</button>
          </div>

          <Show when={route().id} fallback={<ListView me={me} onOpen={open} />}>
            <Detail id={route().id!} me={me} onBack={back} />
          </Show>

          <div class="text-center text-10px text-neutral-700 mt-6 uppercase tracking-widest">
            heads-up single elimination · playing as <span class="text-neutral-500 font-mono">{me}</span>
          </div>
        </div>
      </div>
    </Show>
  )
}

/**
 * Result-reporting hook. Call this from the app when a free-play game ends (e.g. from
 * the GameOver handler) passing the winning player's handle. If the room the player just
 * finished belongs to a tournament match (recorded by enterRoom), this POSTs /result so
 * the backend can advance the bracket on both-players agreement. No-op otherwise.
 *
 * Usage in App.tsx GameOver handler (optional, exact wiring is yours):
 *   import { reportTournamentResult } from './Tournaments'
 *   reportTournamentResult(winnerHandle)   // winnerHandle = the player who won the match
 */
export async function reportTournamentResult(winner: string): Promise<void> {
  let saved: { tournamentId: string; matchId: string; room: string } | null = null
  try {
    saved = JSON.parse(localStorage.getItem('poker_tourney_match') || 'null')
  } catch { /* ignore */ }
  if (!saved) return
  // only report if we're actually in that room
  const room = window.location.pathname.replace(/^\/+|\/+$/g, '')
  if (room && saved.room && room !== saved.room) return
  const reporter = playerHandle()
  await api(`/tournaments/${saved.tournamentId}/result`, {
    method: 'POST',
    body: JSON.stringify({ match_id: saved.matchId, winner, reporter }),
  })
  try { localStorage.removeItem('poker_tourney_match') } catch { /* ignore */ }
}
