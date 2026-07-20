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
  by?: string
  tier?: 'gold' | 'platinum'
  funded?: boolean          // platinum: escrow landed. gold is always effectively funded.
}

type TournamentSummary = {
  id: string
  name: string
  organizer: string
  paid: boolean
  buyin_zat: number
  state: 'registering' | 'running' | 'finished' | 'cancelled'
  player_count: number
  sponsors?: Sponsor[]
  total_prize_zat?: number
  scheduled_start?: number | null // unix seconds; auto-starts at this time
}

type Match = {
  id: string
  round: number
  a: string | null
  b: string | null
  winner: string | null
  room?: string | null
  // present on `pending` matches: paid = route into a STAKED escrow room; stake_zat = deposit each.
  paid?: boolean
  stake_zat?: number
}

type Tournament = {
  id: string
  name: string
  organizer: string
  paid: boolean
  buyin_zat: number
  state: 'registering' | 'running' | 'finished' | 'cancelled'
  rounds: number
  sponsors?: Sponsor[]
  total_prize_zat?: number
  scheduled_start?: number | null // unix seconds; auto-starts at this time (else manual)
  roll_bps?: number // winner's per-round roll-forward (10000=100% doubling, 7500=×1.5, 5000=flat)
  players: string[]
  matches: Match[]
  // playable matches, annotated by the server with `room`/`paid`/`stake_zat`. Only these carry a
  // room code — the raw `matches` never do. Route play from here.
  pending?: Match[]
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

// Format a Date as a <input type="datetime-local"> value (YYYY-MM-DDTHH:MM) in LOCAL time.
function toLocalInput(d: Date): string {
  const p = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`
}
// The user's timezone, spelled out so a scheduled time is never ambiguous, e.g. "Asia/Bangkok · UTC+7".
function tzLabel(): string {
  const tz = Intl.DateTimeFormat().resolvedOptions().timeZone || 'local'
  const off = -new Date().getTimezoneOffset() / 60
  return `${tz} · UTC${off >= 0 ? '+' : '−'}${Math.abs(off)}`
}
// default scheduled start: current time + 60 minutes, as a datetime-local value.
const defaultStart = () => toLocalInput(new Date(Date.now() + 60 * 60_000))

// The champion's total winnings (zat), honoring the winner's roll-forward %. Each round the winner
// takes the pot (2× the round stake); on a non-final round they re-risk `rollBps` of that pot into
// the next round and BANK the remainder. So rollBps=10000 → pure doubling (bank nothing until the
// final), 5000 → flat stakes (bank half each round). Mirrors the server's stake_for_round ladder.
function championPrize(buyinZat: number, rounds: number, rollBps: number): number {
  const roll = Math.max(1, Math.min(10000, rollBps || 10000))
  let stake = buyinZat
  let take = 0
  for (let r = 1; r <= rounds; r++) {
    const pot = 2 * stake
    if (r === rounds) { take += pot; break }   // final round: keep the whole pot
    const next = Math.floor((pot * roll) / 10000)
    take += pot - next                          // bank the un-re-risked remainder
    stake = next
  }
  return take
}

// Human countdown for a scheduled auto-start (unix seconds). '' when unscheduled.
function startsLabel(ts?: number | null): string {
  if (!ts) return ''
  const secs = ts - Math.floor(Date.now() / 1000)
  if (secs <= 0) return 'starting…'
  if (secs < 3600) return `starts in ${Math.ceil(secs / 60)}m`
  if (secs < 86400) return `starts in ${Math.floor(secs / 3600)}h ${Math.ceil((secs % 3600) / 60)}m`
  return 'starts ' + new Date(ts * 1000).toLocaleString([], { weekday: 'short', hour: '2-digit', minute: '2-digit' })
}

// Preview the per-round stake ladder (ZEC) for a paid tournament given the round-1 buy-in and the
// winner's roll-forward bps: stake_{r+1} = stake_r × 2 × rollBps/10000.
function ladderPreview(buyin: number, rollBps: number): string {
  if (!(buyin > 0)) return ''
  const out: string[] = []
  let s = buyin
  for (let i = 0; i < 4; i++) { out.push(s.toFixed(s < 0.001 ? 4 : 3)); s = (s * 2 * rollBps) / 10000 }
  return out.join(' → ') + ' ZEC'
}

// Last server-supplied error message (e.g. "need at least 2 players", "already registered").
// The server returns a specific {error} with a 4xx; we surface it verbatim instead of collapsing
// every failure into a misleading "service offline". Empty string == a genuine network/offline
// failure (no response body), which callers still render as offline.
let lastApiError = ''
async function api<T>(path: string, init?: RequestInit): Promise<T | null> {
  try {
    const resp = await fetch(path, {
      ...init,
      headers: { 'content-type': 'application/json', ...(init?.headers || {}) },
    })
    if (!resp.ok) {
      try { const b = await resp.json(); lastApiError = (b && b.error) ? String(b.error) : `error ${resp.status}` }
      catch { lastApiError = `error ${resp.status}` }
      return null
    }
    lastApiError = ''
    const text = await resp.text()
    return text ? (JSON.parse(text) as T) : ({} as T)
  } catch {
    lastApiError = '' // network failure / offline — no server message
    return null
  }
}

// ── action auth ─────────────────────────────────────────────────────────────
// Every tournament MUTATION is signed by the same persisted Ed25519 session key that co-signs
// settlements (zid provider). The server binds each handle to the key that first claims it, so a
// caller can only ever act AS ITSELF — no forging an opponent's result, spoofing the organizer, or
// kicking another registrant. This makes the previously self-declared handle strings authenticated.
let _session: Awaited<ReturnType<typeof import('./zid/provider').createSessionKey>> | null = null
async function sessionKey() {
  if (!_session) {
    const { createSessionKey } = await import('./zid/provider')
    _session = await createSessionKey()
  }
  return _session
}
/** Sign a canonical action message; returns the `{pubkey, sig}` fields to merge into the body. */
async function authFields(msg: string): Promise<{ pubkey: string; sig: string }> {
  const s = await sessionKey()
  const sig = await s.sign(new TextEncoder().encode(msg))
  return { pubkey: s.pubkey, sig }
}
const A = 'zk.poker/tourney/v1' // canonical message prefix (must match the server verbatim)

// Route into a tournament match. For FREE matches the room code is joined directly (App creates a
// chip-only room on the fly). For PAID matches we FIRST ask the server to get-or-create the STAKED
// escrow room (server-authoritative stake) — only then navigate, so a paid match can never land in
// a free-play room. Returns an error string on failure (shown by the caller).
async function enterMatch(m: Match, tournamentId: string): Promise<string | null> {
  let room = m.room!
  if (m.paid) {
    const auth = await authFields(`${A}/matchroom:${tournamentId}:${m.id}`)
    const res = await api<{ room: string }>(
      `/tournaments/${tournamentId}/match/${m.id}/room`,
      { method: 'POST', body: JSON.stringify({ who: playerHandle(), ...auth }) },
    )
    if (!res || !res.room) return 'could not open the staked match room (escrow may be offline)'
    room = res.room
  }
  // remember which tournament match this room belongs to so the result hook can report the winner.
  try {
    localStorage.setItem('poker_tourney_match', JSON.stringify({ tournamentId, matchId: m.id, room }))
  } catch { /* ignore */ }
  // reuse the app's pathname-based join: App reads location.pathname on load and drops the player
  // into the room's "sit down" lobby with their saved nickname.
  window.location.assign('/' + room.replace(/^\/+/, ''))
  return null
}

// ── sponsor banner ─────────────────────────────────────────────────────────
/** One sponsor chip — honest tiering: 🥇 gold = "pledged" (a promise); 💎 platinum = "verified"
 *  only once its escrow is funded, else "funding…". A platinum that isn't funded is shown muted. */
function SponsorChip(props: { s: Sponsor; tid: string; me: string; organizer?: string; onChanged: () => void }) {
  const s = () => props.s
  const platinum = () => s().tier === 'platinum'
  const funded = () => !platinum() || !!s().funded
  const canRemove = () => s().by && (s().by === props.me || props.organizer === props.me)
  const remove = async () => {
    const auth = await authFields(`${A}/sponsor_remove:${props.tid}:${s().by}`)
    await api(`/tournaments/${props.tid}/sponsor/remove`, {
      method: 'POST', body: JSON.stringify({ who: props.me, target: s().by, ...auth }),
    })
    props.onChanged()
  }
  return (
    <div class={`flex items-center gap-2 p-2 rounded-lg border ${
      platinum() && funded() ? 'bg-zec-yellow/10 border-zec-yellow/50'
      : platinum() ? 'bg-white/5 border-white/15 opacity-70'
      : 'bg-zec-yellow/5 border-zec-yellow/25'}`}>
      <span class="text-11px shrink-0" title={platinum() ? 'escrowed 2-of-3 prize' : 'organizer pledge'}>{platinum() ? '💎' : '🥇'}</span>
      <a href={s().url || '#'} target={s().url ? '_blank' : undefined} rel="noopener noreferrer"
        class="flex items-center gap-2 min-w-0 no-underline flex-1">
        <Show when={s().logo_url}><img src={s().logo_url!} alt={s().name || ''} class="h-6 max-w-24 object-contain shrink-0" /></Show>
        <Show when={s().name}><span class="text-12px font-semibold text-zec-text truncate">{s().name}</span></Show>
      </a>
      <Show when={(s().added_prize_zat ?? 0) > 0}>
        <span class={`text-10px px-1.5 py-0.5 rounded shrink-0 ${
          platinum() && funded() ? 'bg-zec-yellow text-black font-semibold'
          : platinum() ? 'text-white/50 border border-white/15'
          : 'bg-zec-yellow/15 text-zec-yellow border border-zec-yellow/40'}`}>
          {platinum() && !funded() ? 'funding…' : (platinum() ? 'verified ' : 'pledged ') + fmtZec(s().added_prize_zat!)}
        </span>
      </Show>
      <Show when={canRemove()}>
        <button class="text-10px text-neutral-500 hover:text-red-400 shrink-0"
          onClick={remove} title="remove this sponsor">✕</button>
      </Show>
    </div>
  )
}

function SponsorList(props: { sponsors: Sponsor[]; tid: string; totalZat?: number; me: string; organizer?: string; onChanged: () => void }) {
  // public surfaces only show gold + FUNDED platinum; a not-yet-funded platinum is shown to its
  // own owner (so they can manage it) but not advertised to everyone.
  const visible = () => props.sponsors.filter(s =>
    s.tier !== 'platinum' || s.funded || s.by === props.me || props.organizer === props.me)
  return (
    <Show when={visible().length > 0 || (props.totalZat ?? 0) > 0}>
      <div class="mb-4">
        <div class="flex items-center justify-between mb-2">
          <span class="text-9px text-white/40 uppercase tracking-widest">sponsors</span>
          <Show when={(props.totalZat ?? 0) > 0}>
            <span class="text-10px text-zec-yellow">prize pool {fmtZec(props.totalZat!)}</span>
          </Show>
        </div>
        <div class="flex flex-col gap-1.5">
          <For each={visible()}>{s => <SponsorChip s={s} tid={props.tid} me={props.me} organizer={props.organizer} onChanged={props.onChanged} />}</For>
        </div>
      </div>
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
  const [paid, setPaid] = createSignal(false)
  const [buyin, setBuyin] = createSignal('') // ZEC (decimal) buy-in for round 1
  const [startAt, setStartAt] = createSignal(defaultStart()) // datetime-local value; default now+60min, empty = manual
  const [rollBps, setRollBps] = createSignal(10000) // winner's per-round roll-forward (paid only)
  const create = async () => {
    const n = name().trim()
    if (!n) { setErr('name your tournament'); return }
    const buyinZat = Math.max(0, Math.round((parseFloat(buyin()) || 0) * 1e8))
    if (paid() && buyinZat <= 0) { setErr('paid tournaments need a buy-in'); return }
    // optional scheduled auto-start — datetime-local is local time; convert to unix seconds.
    let scheduledStart: number | undefined
    if (startAt()) {
      const ms = new Date(startAt()).getTime()
      if (!Number.isFinite(ms)) { setErr('invalid start time'); return }
      if (ms < Date.now() + 30_000) { setErr('start time must be at least a minute out'); return }
      scheduledStart = Math.floor(ms / 1000)
    }
    setBusy(true)
    setErr('')
    const organizer = playerHandle()
    const auth = await authFields(`${A}/create:${organizer}`)
    const res = await api<{ id: string }>('/tournaments', {
      method: 'POST',
      body: JSON.stringify({ name: n, organizer, paid: paid(), buyin_zat: buyinZat, scheduled_start: scheduledStart, roll_bps: paid() ? rollBps() : undefined, ...auth }),
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
      <div class="flex items-center gap-2 mt-3">
        <span class="text-10px text-white/40 uppercase tracking-wider mr-1">format</span>
        <button
          class={`text-10px px-2 py-0.5 rounded-full border ${!paid() ? 'bg-white/12 text-white/80 border-white/25' : 'bg-white/5 text-white/45 border-white/10'}`}
          onClick={() => setPaid(false)}
        >free · chip-only</button>
        <button
          class={`text-10px px-2 py-0.5 rounded-full border ${paid() ? 'bg-zec-yellow/15 text-zec-yellow border-zec-yellow/40' : 'bg-white/5 text-white/45 border-white/10'}`}
          onClick={() => setPaid(true)}
        >paid · real ZEC</button>
      </div>
      {/* scheduled auto-start — defaults to now+60min; clear it for a manual start. On mobile the
          datetime-local input opens the native Android/iOS calendar+clock picker. */}
      <div class="mt-3">
        <div class="flex items-center gap-2 flex-wrap">
          <span class="text-10px text-white/40 uppercase tracking-wider mr-1">starts</span>
          <Show when={startAt()} fallback={
            <button class="text-11px text-zec-yellow/90 underline decoration-dotted"
              onClick={() => setStartAt(defaultStart())}>set a start time</button>
          }>
            <input type="datetime-local" class="input-field text-12px"
              min={toLocalInput(new Date(Date.now() + 60_000))}
              value={startAt()} onInput={e => setStartAt(e.currentTarget.value)} />
            <button class="text-9px text-neutral-500 underline" onClick={() => setStartAt('')}>manual start</button>
          </Show>
        </div>
        <div class="text-9px text-neutral-500 mt-1">
          <Show when={startAt()} fallback="no schedule — you start it manually.">
            times in <span class="text-white/60">{tzLabel()}</span> · auto-starts with whoever's registered (≥2, else cancels)
          </Show>
        </div>
      </div>
      <Show when={paid()}>
        <div class="mt-3 grid gap-2">
          <label class="flex items-center gap-2">
            <span class="text-10px text-white/50 w-24 shrink-0">round-1 buy-in</span>
            <input class="input-field text-12px flex-1 min-w-32" placeholder="ZEC per player, e.g. 0.01"
              value={buyin()} onInput={e => setBuyin(e.currentTarget.value)} />
          </label>
          {/* roll-forward: how much of the pot the winner re-risks each round vs banks */}
          <div>
            <div class="text-10px text-white/50 mb-1">winner re-risks each round</div>
            <div class="flex gap-1.5">
              <For each={[[10000, '100%'], [7500, '75%'], [5000, '50%']] as [number, string][]}>
                {opt =>
                  <button
                    class={`text-10px px-2.5 py-1 rounded-full border ${rollBps() === opt[0] ? 'bg-zec-yellow/15 text-zec-yellow border-zec-yellow/40' : 'bg-white/5 text-white/45 border-white/10'}`}
                    onClick={() => setRollBps(opt[0])}
                  >{opt[1]}</button>
                }
              </For>
            </div>
            <div class="text-9px text-neutral-500 mt-1.5 leading-relaxed">
              {rollBps() === 10000
                ? 'classic — the stake doubles each round and the winner takes all.'
                : `winner banks ${((10000 - rollBps()) / 100).toFixed(0)}% of the pot each round, so deep runs profit even without winning.`}
              <Show when={parseFloat(buyin()) > 0}>
                {' stakes: '}<span class="font-mono text-white/60">{ladderPreview(parseFloat(buyin()), rollBps())}</span>
              </Show>
            </div>
          </div>
          <div class="text-9px text-neutral-500 leading-relaxed">
            Peer-to-peer &amp; non-custodial — no house holds the pot. Each match is its own 2-of-3
            FROST escrow between the two players; the winner is paid to their own wallet and
            re-deposits (double the stake) for the next round. Field must be a power of two
            (2/4/8/16) so stacks stay equal. A network fee applies per round.
          </div>
        </div>
      </Show>
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
  // sponsor form (organizer sets branding + a *pledged* prize — no money is held; the sponsor
  // pays the champion directly, so this is pure display + a pledge, zero custody).
  const [showSponsor, setShowSponsor] = createSignal(false)
  const [spName, setSpName] = createSignal('')
  const [spLogo, setSpLogo] = createSignal('')
  const [spUrl, setSpUrl] = createSignal('')
  const [spPrize, setSpPrize] = createSignal('') // ZEC (decimal), optional

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
  // a playable match: read the server's `pending` list (the only matches carrying room/stake).
  const myMatch = (): Match | null => {
    const cur = t()
    if (!cur || cur.state !== 'running') return null
    return (cur.pending ?? []).find(m =>
      (m.a === props.me || m.b === props.me) && m.room,
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

  // `msg` is the canonical action string to sign (must match the server); `body` is merged with the
  // resulting {pubkey, sig}. All mutating tournament calls go through here so every one is signed.
  const act = async (path: string, msg: string, body: Record<string, unknown>) => {
    setBusy(true)
    const auth = await authFields(msg)
    const res = await api(`/tournaments/${props.id}/${path}`, { method: 'POST', body: JSON.stringify({ ...body, ...auth }) })
    setBusy(false)
    // show the server's specific reason (e.g. "need at least 2 players", "only the organizer can
    // start", "already registered"); fall back to offline only when there was no response body.
    if (res === null) setErr(lastApiError || 'action failed — the service may be offline')
    else setErr('')
    await load()
  }

  const join = () => act('join', `${A}/join:${props.id}:${props.me}`, { player: props.me })
  const leave = () => act('leave', `${A}/leave:${props.id}:${props.me}`, { player: props.me })
  const start = () => act('start', `${A}/start:${props.id}`, { who: props.me })
  const cancel = () => { if (confirm('Cancel this tournament? This cannot be undone.')) act('cancel', `${A}/cancel:${props.id}`, { who: props.me }) }

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

          <SponsorList sponsors={cur().sponsors ?? []} tid={props.id} totalZat={cur().total_prize_zat} me={props.me} organizer={cur().organizer} onChanged={load} />

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
                  : cur().state === 'cancelled' ? 'text-red-400/80'
                  : 'text-white/50'
                }>{cur().state}</span>
                <Show when={cur().state === 'registering' && cur().scheduled_start}>
                  {' · '}<span class="text-zec-yellow/80">{startsLabel(cur().scheduled_start)}</span>
                </Show>
              </div>
            </div>
            <Show when={cur().paid} fallback={
              <span class="text-9px font-semibold uppercase tracking-wider px-1.5 py-0.5 rounded bg-white/8 text-white/50 border border-white/15 shrink-0">free</span>
            }>
              <span class="text-9px font-semibold uppercase tracking-wider px-1.5 py-0.5 rounded bg-zec-yellow/15 text-zec-yellow border border-zec-yellow/40 shrink-0" title="peer-to-peer buy-in, non-custodial">
                paid · {fmtZec(cur().buyin_zat)}
              </span>
            </Show>
          </div>

          {/* paid tournament: prize/stake summary — honors the winner's roll-forward % (roll_bps). */}
          <Show when={cur().paid && cur().rounds > 0}>
            {(() => {
              const roll = cur().roll_bps ?? 10000
              const mode = roll >= 10000
                ? 'Winner-take-all — the stake doubles each round.'
                : roll <= 5000
                  ? `Flat stakes — the winner banks part of every pot (rolls ${(roll / 100).toFixed(0)}% forward).`
                  : `The winner rolls ${(roll / 100).toFixed(0)}% of each pot forward and banks the rest.`
              return (
                <div class="mb-4 p-3 rounded-lg border border-zec-yellow/20 bg-zec-yellow/5 text-11px text-white/70 leading-relaxed">
                  <span class="text-zec-yellow font-semibold">{mode}</span>{' '}
                  Buy-in {fmtZec(cur().buyin_zat)}. A champion of {cur().rounds} round{cur().rounds === 1 ? '' : 's'} ends with about{' '}
                  <span class="font-mono text-zec-yellow">{fmtZec(championPrize(cur().buyin_zat, cur().rounds, roll))}</span>{' '}
                  in their own wallet (minus per-round network fees). Non-custodial: each match is its own
                  player-to-player escrow — no house ever holds the pot.
                </div>
              )
            })()}
          </Show>

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
                    <Show when={m().paid && m().stake_zat}>
                      {' · '}<span class="text-zec-yellow">stake {fmtZec(m().stake_zat!)} + network fee</span> to your match escrow (exact amount shown at the table)
                    </Show>
                  </div>
                </div>
                <button
                  class="btn btn-primary text-13px px-5 py-2 shrink-0"
                  disabled={busy()}
                  onClick={async () => {
                    setBusy(true)
                    const e = await enterMatch(m(), props.id)
                    setBusy(false)
                    if (e) setErr(e)
                  }}
                >{m().paid ? 'Deposit & play →' : 'Play →'}</button>
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
                <button class="text-11px text-red-400/80 hover:text-red-400 underline ml-auto" disabled={busy()}
                  onClick={cancel}>cancel tournament</button>
              </Show>
            </div>
          </Show>

          {/* organizer can also cancel a running tournament */}
          <Show when={cur().state === 'running' && isOrganizer()}>
            <div class="mb-4">
              <button class="text-11px text-red-400/80 hover:text-red-400 underline" disabled={busy()}
                onClick={cancel}>cancel tournament</button>
            </div>
          </Show>

          {/* PERMISSIONLESS sponsorship. The creator's entry is a GOLD pledge (unescrowed promise);
              anyone else is PLATINUM — their prize will be escrowed in a 2-of-3 vault (funding flow
              coming) and shows as "pending" until funded. */}
          <Show when={cur().state === 'registering' || cur().state === 'running'}>
            <div class="mb-4">
              <button
                class="text-11px text-zec-yellow/80 hover:text-zec-yellow underline decoration-dotted"
                onClick={() => setShowSponsor(v => !v)}
              >+ become a sponsor</button>
              <Show when={showSponsor()}>
                <div class="mt-2 p-3 rounded-lg border border-white/10 bg-zec-surface grid gap-2">
                  <div class="text-9px text-neutral-500 uppercase tracking-widest leading-relaxed">
                    {isOrganizer()
                      ? '🥇 gold pledge — a promise you pay the champion directly. No funds held.'
                      : '💎 platinum — your prize will be escrowed in a 2-of-3 vault (funding coming soon); shown as pending until funded.'}
                  </div>
                  <input class="input-field text-11px" placeholder="sponsor name"
                    value={spName()} onInput={e => setSpName(e.currentTarget.value)} />
                  <input class="input-field text-11px" placeholder="logo image URL (https://…)"
                    value={spLogo()} onInput={e => setSpLogo(e.currentTarget.value)} />
                  <input class="input-field text-11px" placeholder="website URL (https://…)"
                    value={spUrl()} onInput={e => setSpUrl(e.currentTarget.value)} />
                  <input class="input-field text-11px" placeholder="pledged prize in ZEC (optional, e.g. 0.5)"
                    value={spPrize()} onInput={e => setSpPrize(e.currentTarget.value)} />
                  <button class="btn btn-primary text-11px px-3" disabled={busy() || !spName().trim()}
                    onClick={async () => {
                      const zat = Math.max(0, Math.round((parseFloat(spPrize()) || 0) * 1e8))
                      await act('sponsor', `${A}/sponsor:${props.id}`, {
                        who: props.me, name: spName().trim(),
                        logo_url: spLogo().trim(), url: spUrl().trim(), added_prize_zat: zat,
                      })
                      setShowSponsor(false)
                    }}
                  >save sponsor</button>
                </div>
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
        {/* first shown sponsor's logo (gold or funded platinum) */}
        {(() => { const s = (props2.t.sponsors ?? []).find(x => x.tier !== 'platinum' || x.funded); return (
          <Show when={s?.logo_url}><img src={s!.logo_url!} alt="" class="h-6 w-6 rounded object-contain shrink-0" /></Show>
        ) })()}
        <div class="min-w-0">
          <div class="text-13px font-semibold text-white/87 truncate">{props2.t.name}</div>
          <div class="text-10px text-neutral-500 truncate">
            by <span class="font-mono">{props2.t.organizer}</span> · {props2.t.player_count} players
            <Show when={props2.t.state === 'registering' && props2.t.scheduled_start}>
              {' · '}<span class="text-zec-yellow/80">{startsLabel(props2.t.scheduled_start)}</span>
            </Show>
            <Show when={(props2.t.total_prize_zat ?? 0) > 0}>{' · '}<span class="text-zec-yellow">{fmtZec(props2.t.total_prize_zat!)} prize</span></Show>
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
  const parsePath = () => {
    // one-time migration: translate a legacy `#/tournaments[/<id>]` link to the new `/t` path.
    const h = window.location.hash.replace(/^#/, '')
    if (h.startsWith('/tournaments')) {
      const rest = h.slice('/tournaments'.length).replace(/^\/+/, '')
      history.replaceState(null, '', '/t' + (rest ? '/' + rest : ''))
    }
    const segs = window.location.pathname.replace(/^\/+|\/+$/g, '').split('/').filter(Boolean)
    if (segs[0] !== 't') return { open: false, id: null as string | null }
    return { open: true, id: segs[1] || null }
  }
  const [route, setRoute] = createSignal(parsePath())
  // Back/Forward (and the mobile back gesture) drive the overlay via popstate — real URLs, so a
  // tournament/bracket is a shareable `/t/<id>` link, not a hidden hash.
  const onNav = () => setRoute(parsePath())
  onMount(() => window.addEventListener('popstate', onNav))
  onCleanup(() => window.removeEventListener('popstate', onNav))

  const me = playerHandle()
  const open = (id: string) => { history.pushState(null, '', '/t/' + id); setRoute(parsePath()) }
  const back = () => { history.pushState(null, '', '/t'); setRoute(parsePath()) }
  const close = () => { history.pushState(null, '', '/'); setRoute({ open: false, id: null }) }

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
  const auth = await authFields(`${A}/result:${saved.tournamentId}:${saved.matchId}:${winner}`)
  await api(`/tournaments/${saved.tournamentId}/result`, {
    method: 'POST',
    body: JSON.stringify({ match_id: saved.matchId, winner, reporter, ...auth }),
  })
  try { localStorage.removeItem('poker_tourney_match') } catch { /* ignore */ }
}
