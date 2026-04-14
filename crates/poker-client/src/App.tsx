import { createSignal, For, Show, createEffect, onCleanup, onMount } from 'solid-js'
import { createSocket } from './ws'
import { Card } from './Card'
import Lobby, { type Table } from './Lobby'
import { detectZafu } from './zid/provider'
import { getPositionShort } from './positions'
import type { ServerMsg, CardJson, ValidAction } from './types'

export default function App() {
  const [view, setView] = createSignal<'casino' | 'lobby' | 'waiting' | 'game'>(
    location.pathname.length > 1 ? 'lobby' : 'casino'
  )
  const [selectedTable, setSelectedTable] = createSignal<Table | null>(null)
  const [hasWallet, setHasWallet] = createSignal(false)
  const [walletPubkey, setWalletPubkey] = createSignal<string | undefined>(undefined)

  // detect zafu wallet via zid SDK
  onCleanup((() => {
    const check = async () => {
      const found = await detectZafu()
      setHasWallet(!!found)
      const id = identity()
      if (id?.sessionPubKey) setWalletPubkey(id.sessionPubKey)
    }
    check()
    const iv = setTimeout(check, 1500)
    return () => clearTimeout(iv)
  })())
  const [name, setName] = createSignal('')
  const [mySeat, setMySeat] = createSignal(-1)
  const [maxSeats, setMaxSeats] = createSignal(2)
  const [oppName, setOppName] = createSignal('\u2014')
  const [playerNames, setPlayerNames] = createSignal<Record<number, string>>({})
  const [stacks, setStacks] = createSignal([0, 0])
  const [bets, setBets] = createSignal([0, 0])
  const [myCards, setMyCards] = createSignal<[CardJson, CardJson] | null>(null)
  const [oppCards, setOppCards] = createSignal<[CardJson, CardJson] | null>(null)
  const [oppRevealed, setOppRevealed] = createSignal(false)
  const [board, setBoard] = createSignal<CardJson[]>([])
  const [pot, setPot] = createSignal(0)
  const [handNum, setHandNum] = createSignal(0)
  const [button, setButton] = createSignal(0)
  const [actions, setActions] = createSignal<ValidAction[]>([])
  const [acting, setActing] = createSignal(-1)
  const [logs, setLogs] = createSignal<{ text: string; cls: string }[]>([])
  const [raiseVal, setRaiseVal] = createSignal(0)
  const [roomCode, setRoomCode] = createSignal('')
  const [inviteUrl, setInviteUrl] = createSignal('')
  const [juryProgress, setJuryProgress] = createSignal('')
  const [escrow, setEscrow] = createSignal('')
  const [pendingRules, setPendingRules] = createSignal<{ buyin: number; smallBlind: number; bigBlind: number; turnTimeout: number; fromSelf: boolean } | null>(null)
  const [oppDisconnected, setOppDisconnected] = createSignal(false)
  const [reconnectCountdown, setReconnectCountdown] = createSignal(0)
  const [actionTimer, setActionTimer] = createSignal(0)
  const [autoAction, setAutoAction] = createSignal<'none' | 'check/fold' | 'check' | 'fold' | 'call any'>('check/fold')
  const [deckVerified, setDeckVerified] = createSignal(false)
  const [mediaWarningAcked, setMediaWarningAcked] = createSignal(false)
  const [gameStatus, setGameStatus] = createSignal('')
  const [lastResult, setLastResult] = createSignal<{ won: boolean; amount: number } | null>(null)

  // --- Desktop detection ---
  const [isDesktop, setIsDesktop] = createSignal(window.innerWidth > 640)
  onMount(() => {
    const onResize = () => setIsDesktop(window.innerWidth > 640)
    window.addEventListener('resize', onResize)
    onCleanup(() => window.removeEventListener('resize', onResize))
  })

  // --- Desktop chat widget ---
  const [chatOpen, setChatOpen] = createSignal(false)
  const [chatUnread, setChatUnread] = createSignal(0)

  // --- Desktop game log fade ---
  const [logFadeTimer, setLogFadeTimer] = createSignal<ReturnType<typeof setTimeout> | null>(null)
  const [logVisible, setLogVisible] = createSignal(true)

  function resetLogFade() {
    setLogVisible(true)
    const prev = logFadeTimer()
    if (prev) clearTimeout(prev)
    setLogFadeTimer(setTimeout(() => setLogVisible(false), 6000))
  }

  // --- Broadcast (spectator stream) ---
  // player controls what spectators see. server fans out via /{code}/spectate WS.
  // only public info by default. own hole cards opt-in.
  const [broadcasting, setBroadcasting] = createSignal(false)
  const [showMyCards, setShowMyCards] = createSignal(false)
  const [spectatorCount, setSpectatorCount] = createSignal(0)

  function broadcastEvent(event: string, data: Record<string, unknown> = {}) {
    if (!broadcasting()) return
    // filter: only public info
    const payload = JSON.stringify({ event, ...data, ts: Date.now() })
    send({ type: 'Broadcast', data: payload })
  }

  const opp = () => mySeat() === 0 ? 1 : 0
  const myStack = () => stacks()[mySeat()] ?? 0
  const oppStack = () => stacks()[opp()] ?? 0
  const myBet = () => bets()[mySeat()] ?? 0
  const oppBet = () => bets()[opp()] ?? 0
  const isMyTurn = () => acting() === mySeat()

  function log(text: string, cls = '') {
    setLogs(l => [...l.slice(-60), { text, cls }])
    resetLogFade()
    // increment unread when chat is closed on desktop
    if (isDesktop() && !chatOpen() && cls.includes('text-')) {
      setChatUnread(c => c + 1)
    }
  }

  function onMsg(msg: ServerMsg) {
    switch (msg.type) {
      case 'Seated':
        setMySeat(msg.seat)
        setView('waiting')
        break
      case 'Waiting':
        setView('waiting')
        break
      case 'OpponentJoined':
        setOppName(msg.name)
        setPlayerNames(p => ({ ...p, [msg.seat]: msg.name }))
        break
      case 'RulesProposed':
        setPendingRules({ buyin: msg.buyin, smallBlind: msg.smallBlind, bigBlind: msg.bigBlind, turnTimeout: (msg as any).turnTimeout ?? 30, fromSelf: msg.fromSelf })
        if (msg.fromSelf) {
          log(`proposed: ${msg.smallBlind}/${msg.bigBlind} blinds, ${msg.buyin} buyin`)
        } else {
          log(`opponent proposes: ${msg.smallBlind}/${msg.bigBlind} blinds, ${msg.buyin} buyin`, 'c-zec-yellow')
        }
        break
      case 'RulesAccepted':
        setPendingRules(null)
        log('rules accepted', 'c-green')
        break
      case 'OpponentLeft':
        setOppName('\u2014')
        setOppDisconnected(false)
        setReconnectCountdown(0)
        setActions([])
        setView('waiting')
        log('opponent left')
        break
      case 'OpponentDisconnected': {
        setOppDisconnected(true)
        setReconnectCountdown(msg.reconnect_secs)
        log(`opponent disconnected (${msg.reconnect_secs}s to reconnect)`, 'c-red')
        // countdown timer
        const iv = setInterval(() => {
          setReconnectCountdown(c => {
            if (c <= 1) { clearInterval(iv); return 0 }
            return c - 1
          })
        }, 1000)
        break
      }
      case 'OpponentReconnected':
        setOppDisconnected(false)
        setReconnectCountdown(0)
        log('opponent reconnected', 'c-green')
        break
      case 'TimerTick':
        setActionTimer(msg.seconds_left)
        break
      case 'ActionTimeout':
        log(`${msg.seat === mySeat() ? 'you' : 'opp'} timed out (auto-fold)`, 'c-red')
        setActionTimer(0)
        break
      case 'HandStarted':
        setView('game')
        setStacks(msg.stacks)
        setBets([0, 0])
        setButton(msg.button)
        setHandNum(msg.hand_number)
        setBoard([])
        setPot(0)
        setActions([])
        setGameStatus('') // clear shuffle overlay
        setOppRevealed(false)
        setOppCards(null)
        if (msg.your_cards) {
          setMyCards(msg.your_cards)
        }
        log(`hand #${msg.hand_number}`, 'c-green')
        broadcastEvent('hand_started', {
          hand: msg.hand_number, button: msg.button, stacks: msg.stacks,
          ...(showMyCards() && msg.your_cards ? { hero_cards: msg.your_cards } : {}),
        })
        break
      case 'BlindsPosted':
        setBets(b => {
          const n = [...b]
          n[msg.small_blind[0]] = msg.small_blind[1]
          n[msg.big_blind[0]] = msg.big_blind[1]
          return n
        })
        break
      case 'ActionRequired':
        if (msg.seat < 0) { setActions([]); setActing(-1); break } // clear stale
        setActing(msg.seat)
        if (msg.seat === mySeat()) {
          // don't auto-action when all-in (stack = 0) or only 1 valid action
          const myCurrentStack = stacks()[mySeat()] ?? 0
          if (myCurrentStack === 0) {
            setActions(msg.valid_actions)
            break
          }
          // check auto-action first
          const aa = autoAction()
          const hasCheck = msg.valid_actions.some(a => a.kind === 'check')
          const hasCall = msg.valid_actions.find(a => a.kind === 'call')
          let autoFired = false
          if (aa === 'check/fold') {
            setAutoAction('none')
            // T3: use setTimeout(0) to let UI update before action
            setTimeout(() => { if (hasCheck) act('check'); else act('fold') }, 0)
            autoFired = true
          } else if (aa === 'check' && hasCheck) {
            setAutoAction('none')
            setTimeout(() => act('check'), 0)
            autoFired = true
          } else if (aa === 'fold') {
            setAutoAction('none')
            setTimeout(() => act('fold'), 0)
            autoFired = true
          } else if (aa === 'call any' && hasCall) {
            setAutoAction('none')
            setTimeout(() => act('call', hasCall.min_amount), 0)
            autoFired = true
          }
          if (autoFired) break
          // auto-action didn't match this prompt — keep it active for next turn
          setActions(msg.valid_actions)
          const r = msg.valid_actions.find(a => a.kind === 'raise' || a.kind === 'bet')
          if (r) setRaiseVal(r.min_amount)
        } else {
          setActions([])
        }
        break
      case 'PlayerActed': {
        setActing(-1)
        setActions([])
        const s = [...stacks()]
        s[msg.seat] = msg.new_stack
        setStacks(s)
        const b = [...bets()]
        if (msg.action === 'bet' || msg.action === 'raise') b[msg.seat] = msg.amount
        else if (msg.action === 'call') b[msg.seat] = Math.max(...b)
        setBets(b)
        const pos = msg.seat === button() ? 'BTN' : 'BB'
        const who = msg.seat === mySeat() ? `you(${pos})` : `opp(${pos})`
        const amt = msg.amount > 0 && (msg.action === 'bet' || msg.action === 'raise') ? ` ${msg.amount}` : ''
        log(`${who}: ${msg.action}${amt}`)
        broadcastEvent('action', { seat: msg.seat, action: msg.action, amount: msg.amount })
        break
      }
      case 'CommunityCards':
        setBoard(msg.cards)
        setBets([0, 0])
        log(`${msg.phase}: ${msg.cards.map(c => c.rank + c.suit).join(' ')}`, 'c-green')
        broadcastEvent('community', { phase: msg.phase, cards: msg.cards })
        break
      case 'PotUpdate':
        setPot(msg.pots.reduce((s, p) => s + p.amount, 0))
        break
      case 'Showdown':
        for (const [seat, cards] of msg.hands) {
          if (seat === opp()) { setOppCards(cards); setOppRevealed(true) }
        }
        log('showdown', 'c-green')
        broadcastEvent('showdown', { hands: msg.hands })
        break
      case 'PotAwarded': {
        const won = msg.seat === mySeat()
        log(`${won ? 'you' : oppName()} wins ${msg.amount}${msg.amount === 0 ? ' (split)' : ''}`, 'c-zec-yellow font-500')
        setLastResult({ won, amount: msg.amount })
        setTimeout(() => setLastResult(null), 2500)
        broadcastEvent('pot_awarded', { seat: msg.seat, amount: msg.amount })
        break
      }
      case 'HandComplete':
        setStacks(msg.stacks)
        setBets([0, 0])
        setActions([])
        setActing(-1)
        break
      case 'Chat':
        if (msg.seat !== mySeat()) {
          log(`${msg.name}: ${msg.text}`, 'text-neutral-300')
        }
        break
      case 'JuryVote':
        setJuryProgress(`jury ${msg.node}/${msg.total}`)
        log(`jury node ${msg.node}/${msg.total} voted [${msg.payload_hash}]`, 'c-green')
        break
      case 'JurySettlement':
        setJuryProgress('')
        log(`settlement ${msg.verified ? 'verified' : 'FAILED'} (${msg.threshold}/${msg.contributions} OSST)`, msg.verified ? 'c-green font-500' : 'c-red')
        break
      case 'RoomInfo':
        if (msg.code && msg.code !== roomCode()) {
          setRoomCode(msg.code)
          log(`table: ${msg.code}`, 'c-green')
        }
        if (msg.escrow && msg.escrow.length > 5) {
          setEscrow(msg.escrow)
        }
        break
      case 'GameOver': {
        const myPayout = msg.payouts.find((p: any) => p[0] === mySeat())
        log(`game over: ${msg.reason}`, 'c-red font-500')
        if (myPayout) log(`your payout: ${myPayout[1]}`, 'c-zec-yellow font-500')
        setActions([])
        setActing(-1)
        break
      }
      case 'DepositStatus':
        log(`deposits: A=${msg.player_a_deposit} B=${msg.player_b_deposit} ${msg.ready ? '✓ ready' : 'waiting...'}`,
          msg.ready ? 'c-green' : 'c-zec-yellow')
        break
      case 'InviteLink':
        setInviteUrl(window.location.origin + msg.url)
        break
      case 'Status':
        setGameStatus(msg.message)
        if (msg.message.includes('verified')) setDeckVerified(true)
        if (msg.phase === 'dealing') setDeckVerified(false) // reset for new hand
        break
      case 'Chat':
        log(`${msg.from}: ${msg.text}`, 'text-cyan-400')
        break
      case 'Error':
        log(`err: ${msg.message}`)
        break
    }
  }

  const { connected, connect, send, identity, encrypted, media } = createSocket(onMsg)

  function sit() {
    const n = name().trim() || 'anon' + String(Math.random() * 100000 | 0).padStart(5, '0')
    // read custom rules from inputs (host only)
    const sbEl = document.getElementById('sb-input') as HTMLInputElement | null
    const bbEl = document.getElementById('bb-input') as HTMLInputElement | null
    const buyinEl = document.getElementById('buyin-input') as HTMLInputElement | null
    const timeoutEl = document.getElementById('timeout-input') as HTMLInputElement | null
    const customRules = sbEl ? {
      smallBlind: parseInt(sbEl.value) || 5,
      bigBlind: parseInt(bbEl?.value ?? '10') || 10,
      buyin: parseInt(buyinEl?.value ?? '1000') || 1000,
      turnTimeout: parseInt(timeoutEl?.value ?? '30') || 30,
    } : undefined
    connect(n, customRules)
  }

  // no auto-connect — always show lobby so user can enter name

  function act(action: string, amount?: number) {
    send({ type: 'Action', action, ...(amount !== undefined && { amount }) })
    setActions([])
  }

  // keybinding modes
  const [keyMode, setKeyMode] = createSignal<'classic' | 'vim'>('classic')

  // keyboard shortcuts
  onCleanup((() => {
    const onKey = (e: KeyboardEvent) => {
      if (view() !== 'game') return
      if ((e.target as HTMLElement)?.tagName === 'INPUT') return

      const a = actions()
      const myTurn = isMyTurn() && a.length > 0
      const hasBet = a.some(v => v.kind === 'bet' || v.kind === 'raise')
      const betAction = a.find(v => v.kind === 'raise') || a.find(v => v.kind === 'bet')

      if (!myTurn) return

      const km = keyMode()

      if (km === 'classic') {
        // PokerStars-style: F1-F4, Space, Enter, Q, 1-4
        switch (e.key) {
          case 'F1': e.preventDefault(); if (a.some(v => v.kind === 'fold')) act('fold'); break
          case 'F2': e.preventDefault()
            if (a.some(v => v.kind === 'check')) act('check')
            else if (a.some(v => v.kind === 'call')) act('call'); break
          case 'F3': e.preventDefault()
            if (betAction) act(betAction.kind, raiseVal() || betAction.min_amount); break
          case 'F4': e.preventDefault()
            if (hasBet && betAction) act(betAction.kind, pot()); break
          case ' ': e.preventDefault()
            if (a.some(v => v.kind === 'check')) act('check')
            else if (a.some(v => v.kind === 'call')) act('call'); break
          case 'Enter': e.preventDefault()
            if (betAction && raiseVal() > 0) act(betAction.kind, raiseVal()); break
          case 'q': case 'Q':
            if (a.some(v => v.kind === 'allin')) act('allin'); break
        }
      } else {
        // vim-style: hjkl movement metaphor
        // f=fold  d=check(do nothing)  s=call(see)  r=raise  w=bet(wager)
        // a=allin  e=enter(confirm raise)  x=check/fold preset
        // gg=min raise  G=pot  H=½pot  L=2x  M=¾pot
        switch (e.key) {
          case 'f': if (a.some(v => v.kind === 'fold')) act('fold'); break
          case 'd': if (a.some(v => v.kind === 'check')) act('check'); break
          case 's': if (a.some(v => v.kind === 'call')) act('call')
                    else if (a.some(v => v.kind === 'check')) act('check'); break
          case 'w': if (betAction) act(betAction.kind, raiseVal() || betAction.min_amount); break
          case 'r': if (betAction) act(betAction.kind, raiseVal() || betAction.min_amount); break
          case 'a': if (a.some(v => v.kind === 'allin')) act('allin'); break
          case 'e': case 'Enter':
            if (betAction && raiseVal() > 0) act(betAction.kind, raiseVal()); break
          case 'x': // check/fold toggle
            setAutoAction(autoAction() === 'check/fold' ? 'none' : 'check/fold'); break
          case 'G': if (hasBet) setRaiseVal(pot()); break
          case 'H': if (hasBet && betAction) setRaiseVal(Math.max(Math.floor(pot() / 2), betAction.min_amount)); break
          case 'M': if (hasBet && betAction) setRaiseVal(Math.max(Math.floor(pot() * 3 / 4), betAction.min_amount)); break
          case 'L': if (hasBet && betAction) setRaiseVal(Math.max(pot() * 2, betAction.min_amount)); break
          case 'j': if (hasBet) setRaiseVal(v => Math.max(v - (betAction?.min_amount || 10), betAction?.min_amount || 0)); break
          case 'k': if (hasBet) setRaiseVal(v => v + (betAction?.min_amount || 10)); break
        }
      }

      // number sizing works in both modes
      if (hasBet && betAction) {
        const p = pot() || 1
        const min = betAction.min_amount || 0
        switch (e.key) {
          case '1': setRaiseVal(Math.max(Math.floor(p / 4), min)); break   // 1/4 pot
          case '2': setRaiseVal(Math.max(Math.floor(p / 2), min)); break   // 1/2 pot
          case '3': setRaiseVal(Math.max(Math.floor(p * 3 / 4), min)); break // 3/4 pot
          case '4': setRaiseVal(Math.max(p, min)); break                    // pot
          case '5': setRaiseVal(Math.max(p * 2, min)); break               // 2x pot
        }
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  })())

  // action timer countdown — driven by game.ts authoritative timer (T1 fix)

  // auto-scroll log
  let logEl!: HTMLDivElement
  createEffect(() => {
    logs()
    if (logEl) logEl.scrollTop = logEl.scrollHeight
  })

  // --- Shared sub-components for reuse between mobile and desktop ---

  /** Titlebar used in non-game views */
  function Titlebar() {
    return (
      <div class="titlebar">
        <span class="text-zec-yellow text-14px">{'\u2666'}</span>
        <span class="flex-1 text-center text-zec-yellow">poker.zk.bot</span>
        <Show when={encrypted()}>
          <span class="text-8px text-green-400 mr-1">enc</span>
        </Show>
        <Show when={identity()}>
          <span class={`text-8px mr-1 ${identity()!.mode === 'zafu' ? 'text-zec-yellow' : 'text-neutral-500'}`}
            title={identity()!.sessionPubKey}>
            {identity()!.mode === 'zafu' ? 'zafu' : 'anon'}
          </span>
        </Show>
        <span class={`w-2 h-2 rounded-full ${connected() ? 'bg-green-500' : 'bg-neutral-600'}`} />
      </div>
    )
  }

  /** Casino lobby view */
  function CasinoView() {
    return (
      <Lobby
        hasWallet={hasWallet()}
        pubkey={walletPubkey()}
        onJoin={(table, playerName) => {
          setSelectedTable(table)
          setName(playerName)
          const params = new URLSearchParams({
            sb: String(table.sb), bb: String(table.bb),
            buyin: String(table.buyin), timeout: String(table.timeout),
            rake_bps: String(table.rakeBps), rake_cap: String(table.rakeCap),
            access: 'private',
          })
          fetch(`/new?${params}`, { redirect: 'follow' }).then(resp => {
            const url = resp.url || resp.headers.get('location') || ''
            const code = url.split('/').pop() || ''
            if (code) {
              history.pushState(null, '', '/' + code)
              setView('lobby')
              setTimeout(() => sit(), 200)
            }
          })
        }}
        onJoinCode={(code, playerName) => {
          setName(playerName)
          history.pushState(null, '', '/' + code)
          setView('lobby')
          setTimeout(() => sit(), 200)
        }}
      />
    )
  }

  /** Direct room link lobby view */
  function LobbyView() {
    return (
      <div class="p-8 text-center">
        <div class="text-zec-yellow text-10px font-semibold uppercase tracking-3px mb-5">
          no-limit hold'em
        </div>
        <Show when={location.pathname.length <= 1} fallback={
          <div class="text-neutral-500 text-11px tracking-wider mb-4">
            joining table &middot; host sets rules
          </div>
        }>
          <div class="flex items-center justify-center gap-4 mb-3">
            <label class="flex flex-col items-center gap-1">
              <span class="text-neutral-500 text-9px uppercase tracking-wider">small blind</span>
              <input class="input-field w-16 text-center" value="5" id="sb-input" />
            </label>
            <label class="flex flex-col items-center gap-1">
              <span class="text-neutral-500 text-9px uppercase tracking-wider">big blind</span>
              <input class="input-field w-16 text-center" value="10" id="bb-input" />
            </label>
            <label class="flex flex-col items-center gap-1">
              <span class="text-neutral-500 text-9px uppercase tracking-wider">buy-in</span>
              <input class="input-field w-20 text-center" value="1000" id="buyin-input" />
            </label>
            <label class="flex flex-col items-center gap-1">
              <span class="text-neutral-500 text-9px uppercase tracking-wider">turn (sec)</span>
              <input class="input-field w-16 text-center" value="30" id="timeout-input" />
            </label>
          </div>
        </Show>
        <div class="text-neutral-600 text-9px tracking-wider mb-6">
          zk-shuffle &middot; co-signed action log &middot; encrypted
        </div>
        <div class="flex flex-col items-center gap-4">
          <div class="flex items-center justify-center gap-2">
            <input
              class="input-field w-48 text-center"
              placeholder="name"
              maxLength={16}
              spellcheck={false}
              value={name()}
              onInput={e => setName(e.currentTarget.value)}
              onKeyDown={e => { if (e.key === 'Enter') sit() }}
              autofocus
            />
            <button class="btn btn-primary" onClick={sit}>
              {location.pathname.length > 1 ? 'sit down' : 'create table'}
            </button>
          </div>
        </div>
      </div>
    )
  }

  /** Waiting for players view */
  function WaitingView() {
    return (
      <div class="p-10 text-center">
        <div class="text-zec-yellow text-11px uppercase tracking-2px mb-4">
          waiting for players
        </div>

        <Show when={identity()?.pickContacts}>
          <button
            class="mb-4 px-4 py-2 bg-zec-surface border border-neutral-800 rounded-lg hover:border-zec-yellow transition-colors inline-flex items-center gap-2"
            onClick={async () => {
              const contacts = await identity()?.pickContacts?.({ purpose: 'Invite to your table', max: 5 })
              if (contacts?.length) {
                for (const c of contacts) {
                  await identity()?.invite?.(c.handle, {
                    type: 'poker-table-invite',
                    data: { url: inviteUrl() },
                    ttl: 300,
                  })
                }
                log(`invited ${contacts.map(c => c.displayName).join(', ')}`, 'c-zec-yellow')
              }
            }}
          >
            <span class="text-zec-yellow text-14px">+</span>
            <span class="text-10px text-neutral-400">invite from contacts</span>
          </button>
        </Show>

        <Show when={inviteUrl()}>
          <div class="mb-4">
            <div class="text-neutral-500 text-9px uppercase tracking-wider mb-2">or share link</div>
            <div
              class="input-field text-11px text-center cursor-pointer select-all"
              onClick={() => { navigator.clipboard?.writeText(inviteUrl()); log('copied invite link', 'c-green') }}
              title="click to copy"
            >
              {inviteUrl()}
            </div>
            <div class="text-neutral-600 text-8px mt-1">click to copy</div>
          </div>
        </Show>
        <Show when={escrow() && escrow().length > 10}>
          <div class="mb-4 p-3 border border-neutral-800 rounded-lg bg-zec-surface">
            <div class="text-neutral-500 text-9px uppercase tracking-wider mb-2">escrow address (2-of-3 multisig)</div>
            <div
              class="font-mono text-9px text-zec-yellow break-all cursor-pointer select-all mb-2"
              onClick={() => { navigator.clipboard?.writeText(escrow()); log('copied escrow address', 'c-green') }}
              title="click to copy"
            >
              {escrow()}
            </div>
            <div class="text-neutral-600 text-8px mb-3">send buy-in to this address - 0-conf accepted</div>
            <div class="flex gap-2 justify-center">
              <button
                class="btn text-9px px-4 py-1"
                onClick={() => {
                  send({ type: 'ReportDeposit', txid: 'demo_' + Date.now(), amount: 1000 })
                  log('deposit reported (demo)', 'c-zec-yellow')
                }}
              >report deposit</button>
            </div>
          </div>
        </Show>

        <Show when={pendingRules() && !pendingRules()?.fromSelf}>
          <div class="mt-4 p-4 border border-neutral-700 rounded">
            <div class="text-neutral-400 text-10px uppercase tracking-wider mb-2">opponent proposes</div>
            <div class="text-white text-12px font-mono mb-3">
              {pendingRules()!.smallBlind}/{pendingRules()!.bigBlind} blinds - {pendingRules()!.buyin} buyin
            </div>
            <button class="btn btn-primary text-11px px-6" onClick={() => send({ type: 'AcceptRules' })}>
              accept
            </button>
          </div>
        </Show>
        <Show when={!pendingRules()}>
          <div class="flex items-end justify-center gap-1 h-6">
            <For each={[0,.07,.14,.21,.28,.35]}>
              {d => <div class="w-1 rounded-sm bg-zec-yellow animate-pulse" style={`animation-delay:${d}s; height: 60%`} />}
            </For>
          </div>
        </Show>
      </div>
    )
  }

  /** Opponent info box - reused in both layouts */
  function OpponentBox(props: { size?: 'desktop' }) {
    const lg = props.size === 'desktop'
    return (
      <div class={`text-center ${lg ? 'w-52' : 'w-44'}`}>
        <div class={`inline-block ${lg ? 'px-4 py-2 rounded-lg' : 'px-3 py-1'} bg-zec-surface border ${acting() === opp() ? 'border-zec-yellow shadow-[0_0_8px_rgba(244,183,40,0.3)]' : oppDisconnected() ? 'border-red-800' : 'border-neutral-800'}`}>
          <div class={`${lg ? 'text-11px' : 'text-9px'} font-semibold uppercase tracking-wider ${acting() === opp() ? 'text-zec-yellow' : oppDisconnected() ? 'text-red-400' : 'text-neutral-500'}`}>
            {oppName()} <span class="text-neutral-600">{getPositionShort(opp(), button(), maxSeats())}</span> {oppDisconnected() ? '(dc)' : ''}
          </div>
          <div class={`font-mono ${lg ? 'text-16px' : 'text-13px'} text-zec-yellow`}>{oppStack()}</div>
          <Show when={acting() === opp() && actionTimer() > 0}>
            <div class={`font-mono ${lg ? 'text-13px' : 'text-11px'} font-bold ${actionTimer() <= 5 ? 'text-red-500 animate-pulse' : actionTimer() <= 10 ? 'text-orange-400' : actionTimer() <= 20 ? 'text-zec-yellow' : 'text-neutral-400'}`}>{actionTimer()}s</div>
          </Show>
        </div>
        <div class={`flex gap-1 justify-center ${lg ? 'mt-2' : 'mt-1.5'}`}>
          <Show when={oppRevealed() && oppCards()} fallback={
            <Show when={myCards()}>
              <Card size={lg ? 'lg' : undefined} /><Card size={lg ? 'lg' : undefined} />
            </Show>
          }>
            <Card card={oppCards()![0]} size={lg ? 'lg' : undefined} />
            <Card card={oppCards()![1]} size={lg ? 'lg' : undefined} />
          </Show>
        </div>
        <div class={`font-mono ${lg ? 'text-13px' : 'text-11px'} text-neutral-400 mt-0.5 h-4`}>{oppBet() > 0 ? oppBet() : ''}</div>
      </div>
    )
  }

  /** Hero info box - reused in both layouts */
  function HeroBox(props: { size?: 'desktop' }) {
    const lg = props.size === 'desktop'
    return (
      <div class={`text-center ${lg ? 'w-52' : 'w-44'}`}>
        <div class={`font-mono ${lg ? 'text-13px' : 'text-11px'} text-neutral-400 mb-0.5 h-4`}>{myBet() > 0 ? myBet() : ''}</div>
        <div class={`flex gap-1 justify-center ${lg ? 'mb-2' : 'mb-1.5'}`}>
          <Show when={myCards()}>
            <Card card={myCards()![0]} size={lg ? 'lg' : undefined} />
            <Card card={myCards()![1]} size={lg ? 'lg' : undefined} />
          </Show>
        </div>
        <div class={`inline-block ${lg ? 'px-4 py-2 rounded-lg' : 'px-3 py-1'} bg-zec-surface border ${acting() === mySeat() ? 'border-zec-yellow shadow-[0_0_8px_rgba(244,183,40,0.3)]' : 'border-neutral-800'}`}>
          <Show when={acting() === mySeat() && actionTimer() > 0}>
            <div class={`font-mono ${lg ? 'text-13px' : 'text-11px'} font-bold ${actionTimer() <= 5 ? 'text-red-500 animate-pulse' : actionTimer() <= 10 ? 'text-orange-400' : actionTimer() <= 20 ? 'text-zec-yellow' : 'text-neutral-400'}`}>{actionTimer()}s</div>
          </Show>
          <div class={`font-mono ${lg ? 'text-16px' : 'text-13px'} text-zec-yellow`}>{myStack()}</div>
          <div class={`${lg ? 'text-11px' : 'text-9px'} font-semibold uppercase tracking-wider ${acting() === mySeat() ? 'text-zec-yellow' : 'text-neutral-500'}`}>
            {name() || 'you'} <span class="text-neutral-600">{button() === mySeat() ? 'BTN/SB' : 'BB'}</span>
          </div>
        </div>
      </div>
    )
  }

  /** Action buttons - reused in both layouts */
  function ActionButtons(props: { size?: 'desktop' }) {
    const lg = props.size === 'desktop'
    return (
      <>
        <Show when={isMyTurn() && actions().length > 0} fallback={
          <Show when={acting() >= 0 && !isMyTurn()}>
            <span class={`text-neutral-600 ${lg ? 'text-13px' : 'text-10px'} uppercase tracking-wider`}>opponent to act</span>
          </Show>
        }>
          {/* sizing buttons */}
          {(() => {
            const betAction = actions().find(v => v.kind === 'raise') || actions().find(v => v.kind === 'bet')
            if (!betAction) return null
            const p = pot()
            const min = betAction.min_amount || 0
            const max = betAction.max_amount || 0
            const clamp = (v: number) => Math.min(Math.max(Math.round(v), min), max)
            const sizes = [
              { label: '\u00BC', val: clamp(p / 4) },
              { label: '\u00BD', val: clamp(p / 2) },
              { label: '\u00BE', val: clamp(p * 3 / 4) },
              { label: 'pot', val: clamp(p) },
              { label: '2x', val: clamp(p * 2) },
            ].filter(s => s.val >= min && s.val <= max)
            const unique = sizes.filter((s, i) => i === 0 || s.val !== sizes[i-1].val)
            return <div class={`flex gap-0.5 justify-center ${lg ? 'mb-2' : 'mb-1'}`}>
              {unique.map(s =>
                <button class={`btn btn-xs ${lg ? 'px-3 py-1 text-11px' : 'px-2 py-0.5 text-9px'} ${raiseVal() === s.val ? 'btn-active' : 'btn-ghost'}`}
                  onClick={() => setRaiseVal(s.val)}>{s.label}</button>
              )}
            </div>
          })()}
          {/* main action buttons */}
          <For each={actions()}>
            {a => {
              const btnCls = lg ? 'min-h-12 px-6 py-2 text-14px' : 'min-h-9 sm:min-h-auto px-3 sm:px-2'
              if (a.kind === 'fold')
                return <button class={`btn btn-danger ${btnCls}`} onClick={() => act('fold')}>fold</button>
              if (a.kind === 'check')
                return <button class={`btn ${btnCls}`} onClick={() => act('check')}>check</button>
              if (a.kind === 'call')
                return <button class={`btn btn-primary ${btnCls}`} onClick={() => act('call')}>call {a.min_amount}</button>
              if (a.kind === 'bet')
                return <div class="flex items-center gap-1">
                  <input class={`input-field ${lg ? 'w-20 text-13px' : 'w-14 sm:w-16 text-11px'} text-center`} type="number"
                    min={a.min_amount} max={a.max_amount}
                    value={raiseVal()} onInput={e => setRaiseVal(+e.currentTarget.value)} />
                  <button class={`btn ${btnCls}`} onClick={() => act('bet', raiseVal())}>bet {raiseVal()}</button>
                </div>
              if (a.kind === 'raise')
                return <div class="flex items-center gap-1">
                  <Show when={!actions().some(x => x.kind === 'bet')}>
                    <input class={`input-field ${lg ? 'w-20 text-13px' : 'w-14 sm:w-16 text-11px'} text-center`} type="number"
                      min={a.min_amount} max={a.max_amount}
                      value={raiseVal()} onInput={e => setRaiseVal(+e.currentTarget.value)} />
                  </Show>
                  <button class={`btn ${btnCls}`} onClick={() => act('raise', raiseVal())}>raise {raiseVal()}</button>
                </div>
              if (a.kind === 'allin')
                return <button class={`btn btn-allin ${btnCls}`} onClick={() => act('allin')}>all in</button>
              return null
            }}
          </For>
        </Show>
      </>
    )
  }

  /** Board cards (deck + community) */
  function BoardCards(props: { size?: 'desktop' }) {
    const lg = props.size === 'desktop'
    return (
      <>
        <Show when={board().length === 0}>
          <div class={`relative ${lg ? 'w-14 h-20' : 'w-12 h-17'} mr-2`} title={deckVerified() ? 'deck verified (Chaum-Pedersen)' : gameStatus() || 'deck'}>
            <div class="absolute inset-0 rounded-sm border border-neutral-700 bg-zec-surface"
              style="transform: rotate(-2deg); background-image: repeating-linear-gradient(45deg,transparent,transparent 3px,rgba(244,183,40,0.05) 3px,rgba(244,183,40,0.05) 4px)" />
            <div class="absolute inset-0 rounded-sm border border-neutral-700 bg-zec-surface"
              style="transform: rotate(1deg); background-image: repeating-linear-gradient(45deg,transparent,transparent 3px,rgba(244,183,40,0.05) 3px,rgba(244,183,40,0.05) 4px)" />
            <div class={`absolute inset-0 rounded-sm border bg-zec-surface flex items-center justify-center ${lg ? 'text-11px' : 'text-9px'} font-bold ${
              gameStatus().includes('shuffl') || gameStatus().includes('key') ? 'border-zec-yellow animate-pulse text-zec-yellow' :
              deckVerified() ? 'border-green-500 text-green-400' :
              'border-neutral-700 text-neutral-600'
            }`} style="background-image: repeating-linear-gradient(45deg,transparent,transparent 3px,rgba(244,183,40,0.05) 3px,rgba(244,183,40,0.05) 4px)">
              {gameStatus().includes('shuffl') || gameStatus().includes('key') || gameStatus().includes('prov') ? '...' :
               deckVerified() ? 'OK' : '52'}
            </div>
          </div>
        </Show>
        <For each={board()}>
          {c => <Card card={c} size="lg" />}
        </For>
      </>
    )
  }

  /** Overlays for disconnect and shuffle status */
  function TableOverlays() {
    return (
      <>
        <Show when={oppDisconnected()}>
          <div class="absolute inset-0 bg-black/60 z-10 flex items-center justify-center rounded-25">
            <div class="text-center">
              <div class="text-red-400 text-11px uppercase tracking-wider mb-1">opponent disconnected</div>
              <div class="font-mono text-18px text-white">{reconnectCountdown()}s</div>
              <div class="text-neutral-500 text-9px">waiting for reconnect</div>
            </div>
          </div>
        </Show>
        <Show when={gameStatus() && !gameStatus().includes('verified') && acting() < 0}>
          <div class="absolute inset-0 bg-black/40 z-10 flex items-center justify-center rounded-25">
            <div class="text-center">
              <div class="text-zec-yellow text-11px uppercase tracking-wider mb-2 animate-pulse">{gameStatus()}</div>
              <div class="flex items-end justify-center gap-1 h-4">
                {[0,.08,.16,.24,.32].map(d =>
                  <div class="w-1 rounded-sm bg-zec-yellow animate-pulse" style={`animation-delay:${d}s; height: 60%`} />
                )}
              </div>
            </div>
          </div>
        </Show>
      </>
    )
  }

  /** Auto-action presets row */
  function AutoActionPresets(props: { size?: 'desktop' }) {
    const lg = props.size === 'desktop'
    return (
      <div class={`flex ${lg ? 'gap-2' : 'gap-1.5'} justify-center py-2`}>
        {(['check/fold', 'check', 'fold', 'call any'] as const).map(mode =>
          <button
            class={`${lg ? 'text-11px px-4 py-2' : 'text-9px px-3 py-1.5'} rounded-md border transition-all ${
              autoAction() === mode
                ? 'border-zec-yellow text-zec-yellow bg-zec-yellow/15 shadow-[0_0_6px_rgba(244,183,40,0.2)]'
                : 'border-neutral-700 text-neutral-500 hover:text-neutral-300 hover:border-neutral-500'
            }`}
            onClick={() => setAutoAction(autoAction() === mode ? 'none' : mode)}
          >
            {mode}
          </button>
        )}
      </div>
    )
  }

  /** Hotkey legend + mode toggle */
  function HotkeyLegend() {
    return (
      <>
        <div class="flex items-center justify-center gap-2 py-0.5">
          <button
            class={`text-7px px-1.5 py-0.5 rounded border ${keyMode() === 'classic' ? 'border-zec-yellow text-zec-yellow' : 'border-neutral-800 text-neutral-700'}`}
            onClick={() => setKeyMode('classic')}
            title="PokerStars-style hotkeys"
          >classic</button>
          <button
            class={`text-7px px-1.5 py-0.5 rounded border ${keyMode() === 'vim' ? 'border-zec-yellow text-zec-yellow' : 'border-neutral-800 text-neutral-700'}`}
            onClick={() => setKeyMode('vim')}
            title="vim-style hotkeys"
          >vim</button>
        </div>
        <div class="text-center text-6px text-neutral-700 pb-0.5">
          <Show when={keyMode() === 'classic'}>
            F1 fold - F2 check/call - F3 raise - F4 pot - Space call - Q all-in - 1-5 sizing
          </Show>
          <Show when={keyMode() === 'vim'}>
            f fold - d check - s call - r/w raise - a all-in - j/k size +/- - H half-p - M 3/4p - G pot - L 2x
          </Show>
        </div>
      </>
    )
  }

  /** Media controls */
  function MediaControls() {
    return (
      <div class="flex items-center justify-between px-1 py-1">
        <div class="flex gap-1">
          <button
            class={`text-9px px-2 py-0.5 rounded border ${media()?.micEnabled() ? 'border-green-500 text-green-400' : 'border-neutral-700 text-neutral-600 hover:text-neutral-400'}`}
            onClick={() => {
              if (!media()?.micEnabled() && !mediaWarningAcked()) {
                if (!confirm('Voice/video is peer-to-peer and reveals your IP address to the other player. Game messages stay encrypted through the relay. Continue?')) return
                setMediaWarningAcked(true)
              }
              media()?.toggleMic()
            }}
          >{media()?.micEnabled() ? 'mic on' : 'mic'}</button>
          <button
            class={`text-9px px-2 py-0.5 rounded border ${media()?.camEnabled() ? 'border-green-500 text-green-400' : 'border-neutral-700 text-neutral-600 hover:text-neutral-400'}`}
            onClick={() => {
              if (!media()?.camEnabled() && !mediaWarningAcked()) {
                if (!confirm('Voice/video is peer-to-peer and reveals your IP address to the other player. Game messages stay encrypted through the relay. Continue?')) return
                setMediaWarningAcked(true)
              }
              media()?.toggleCam()
            }}
          >{media()?.camEnabled() ? 'cam on' : 'cam'}</button>
        </div>
        <div class="flex gap-1">
          <Show when={media()?.remoteStream()}>
            <video
              class="w-16 h-12 rounded border border-neutral-700 object-cover"
              autoplay playsinline
              ref={(el: HTMLVideoElement) => { el.srcObject = media()!.remoteStream() }}
            />
          </Show>
          <Show when={media()?.localStream() && media()?.camEnabled()}>
            <video
              class="w-12 h-9 rounded border border-neutral-800 object-cover opacity-60"
              autoplay playsinline muted
              ref={(el: HTMLVideoElement) => { el.srcObject = media()!.localStream() }}
            />
          </Show>
        </div>
      </div>
    )
  }

  /** Chat + log sidebar for mobile */
  function MobileChatSidebar() {
    return (
      <div class="lg:w-72 lg:flex-shrink-0">
        <div ref={logEl!} class="bg-zec-surface border border-neutral-800 p-2 max-h-28 lg:max-h-80 overflow-y-auto font-mono text-10px mb-1 leading-relaxed">
          <For each={logs()}>
            {l => <div class={`text-neutral-600 ${l.cls}`}>{l.text}</div>}
          </For>
        </div>
        <div class="flex gap-1 mb-1 flex-wrap">
          {['nh', 'gg', 'wp', 'lol', 'wow', '...'].map(r =>
            <button
              class="text-8px px-2 py-0.5 rounded border border-neutral-800 text-neutral-600 hover:text-neutral-400 hover:border-neutral-600 active:text-zec-yellow active:border-zec-yellow transition-colors"
              onClick={() => { send({ type: 'Chat', text: r }); log(`you: ${r}`, 'text-white') }}
            >{r}</button>
          )}
        </div>
        <form class="flex gap-1 mb-2" onSubmit={(e) => {
          e.preventDefault()
          const input = e.currentTarget.querySelector('input') as HTMLInputElement
          const msg = input.value.trim()
          if (!msg) return
          input.value = ''
          if (msg.startsWith('/nick ')) {
            const newName = msg.slice(6).trim()
            if (newName) {
              setName(newName)
              localStorage.setItem('poker_nickname', newName)
              log(`nickname set to ${newName}`, 'text-neutral-400')
            }
            return
          }
          send({ type: 'Chat', text: msg })
          log(`you: ${msg}`, 'text-white')
        }}>
          <input
            class="input-field flex-1 text-10px py-0.5 px-2"
            placeholder="chat..."
            maxLength={200}
          />
          <button type="submit" class="text-8px px-2 py-0.5 rounded border border-neutral-700 text-neutral-600 hover:text-neutral-400">send</button>
        </form>
      </div>
    )
  }

  // ========================================================================
  // DESKTOP GAME VIEW - full-screen felt, floating overlays
  // ========================================================================

  function DesktopGameView() {
    let desktopLogEl!: HTMLDivElement
    createEffect(() => {
      logs()
      if (desktopLogEl) desktopLogEl.scrollTop = desktopLogEl.scrollHeight
    })

    let desktopChatLogEl!: HTMLDivElement
    createEffect(() => {
      logs()
      if (desktopChatLogEl) desktopChatLogEl.scrollTop = desktopChatLogEl.scrollHeight
    })

    return (
      <div class="fixed inset-0 font-sans text-white select-none" style="background: radial-gradient(ellipse at center, #0f2f14 0%, #0a1f0e 40%, #060e06 100%)">

        {/* ---- Top bar: minimal status ---- */}
        <div class="absolute top-0 left-0 right-0 z-20 flex items-center justify-between px-4 py-2">
          <div class="flex items-center gap-3 text-10px text-neutral-500 uppercase tracking-wider">
            <span class="text-zec-yellow font-bold text-12px">{'\u2666'} poker.zk.bot</span>
            <span>hand #{handNum()}</span>
            <Show when={deckVerified()}>
              <span class="text-green-400" title="deck verified via Chaum-Pedersen proof">verified</span>
            </Show>
            <Show when={juryProgress()}>
              <span class="text-zec-yellow animate-pulse">{juryProgress()}</span>
            </Show>
          </div>
          <div class="flex items-center gap-2">
            <Show when={encrypted()}>
              <span class="text-8px text-green-400">enc</span>
            </Show>
            <span class={`w-2 h-2 rounded-full ${connected() ? 'bg-green-500' : 'bg-neutral-600'}`} />
            <button
              class={`px-2 py-1 rounded text-8px border ${broadcasting() ? 'border-red-500 text-red-400 bg-red-900/20' : 'border-neutral-700 text-neutral-600 hover:text-neutral-400'}`}
              onClick={() => setBroadcasting(b => !b)}
            >{broadcasting() ? 'LIVE' : 'broadcast'}</button>
            <Show when={broadcasting()}>
              <button
                class={`px-2 py-1 rounded text-8px border ${showMyCards() ? 'border-zec-yellow text-zec-yellow' : 'border-neutral-700 text-neutral-600'}`}
                onClick={() => setShowMyCards(s => !s)}
              >{showMyCards() ? 'cards: shown' : 'cards: hidden'}</button>
            </Show>
            <button
              class="px-2 py-1 rounded text-8px border border-red-900 text-red-400 hover:bg-red-900/20"
              onClick={() => { if (confirm('Leave table and cash out?')) send({ type: 'Leave' }) }}
            >leave</button>
          </div>
        </div>

        {/* ---- Felt table (elliptical, centered) ---- */}
        <div class="absolute inset-0 flex items-center justify-center">
          <div class="relative" style="width: 70vw; max-width: 900px; height: 50vh; max-height: 500px;">
            {/* Table surface */}
            <div class="absolute inset-0 rounded-[50%] border-4 border-[#1a4a2a]"
              style="background: radial-gradient(ellipse at center, #0f3018 0%, #0b2412 60%, #081a0c 100%); box-shadow: inset 0 4px 30px rgba(0,0,0,0.5), 0 0 60px rgba(10,40,20,0.4)">
            </div>
            {/* Table rail (outer ring) */}
            <div class="absolute rounded-[50%] border-2 border-[#2a1a0a]/60 pointer-events-none"
              style="inset: -8px; box-shadow: inset 0 2px 8px rgba(0,0,0,0.3);" />

            <TableOverlays />

            {/* Opponent (top of table) */}
            <div class="absolute left-50% -translate-x-50%" style="top: -60px">
              <OpponentBox size="desktop" />
            </div>

            {/* Dealer chip */}
            <Show when={button() === mySeat()}>
              <div class="absolute rounded-full w-6 h-6 bg-zec-yellow text-black text-10px font-bold leading-6 text-center border-2 border-zec-gold z-5"
                style="bottom: 20%; left: calc(50% + 80px)">D</div>
            </Show>
            <Show when={button() === opp()}>
              <div class="absolute rounded-full w-6 h-6 bg-zec-yellow text-black text-10px font-bold leading-6 text-center border-2 border-zec-gold z-5"
                style="top: 20%; left: calc(50% + 80px)">D</div>
            </Show>

            {/* Board center */}
            <div class="absolute left-50% top-50% -translate-x-50% -translate-y-50%">
              <div class="flex gap-2 justify-center items-center">
                <BoardCards size="desktop" />
              </div>
              {/* Pot display */}
              <div class="text-center font-mono text-18px font-600 mt-3 min-h-6">
                <Show when={lastResult()} fallback={
                  <span class="text-zec-yellow drop-shadow-[0_0_8px_rgba(244,183,40,0.3)]">{pot() > 0 ? pot() : ''}</span>
                }>
                  <span class={`animate-pulse ${lastResult()!.won ? 'text-green-400' : 'text-red-400'}`}>
                    {lastResult()!.won ? '+' : ''}{lastResult()!.amount}
                  </span>
                </Show>
              </div>
            </div>

            {/* Hero (bottom of table) */}
            <div class="absolute left-50% -translate-x-50%" style="bottom: -60px">
              <HeroBox size="desktop" />
            </div>
          </div>
        </div>

        {/* ---- Action buttons (bottom center, large) ---- */}
        <div class="absolute bottom-0 left-50% -translate-x-50% z-20 pb-4 w-full max-w-2xl">
          <div class="flex gap-2 justify-center items-center flex-wrap mb-2">
            <ActionButtons size="desktop" />
          </div>
          <AutoActionPresets size="desktop" />
          <HotkeyLegend />
          <MediaControls />
        </div>

        {/* ---- Game log overlay (top-left, semi-transparent, auto-fades) ---- */}
        <div
          class="absolute top-12 left-4 z-30 pointer-events-none transition-opacity duration-1000"
          style={`opacity: ${logVisible() ? '0.85' : '0.15'}`}
          onMouseEnter={() => setLogVisible(true)}
          onMouseLeave={() => resetLogFade()}
        >
          <div
            ref={desktopLogEl!}
            class="pointer-events-auto w-64 max-h-40 overflow-y-auto rounded-lg px-3 py-2 font-mono text-10px leading-relaxed"
            style="background: rgba(10,10,10,0.7); backdrop-filter: blur(4px);"
          >
            <For each={logs().slice(-5)}>
              {l => <div class={`text-neutral-500 ${l.cls}`}>{l.text}</div>}
            </For>
            <Show when={logs().length === 0}>
              <div class="text-neutral-700 text-9px">game log</div>
            </Show>
          </div>
        </div>

        {/* ---- Chat widget (bottom-right, messenger-style) ---- */}
        <div class="absolute bottom-4 right-4 z-30">
          <Show when={chatOpen()}>
            <div class="mb-2 rounded-lg overflow-hidden shadow-xl" style="width: 300px; height: 400px; background: rgba(14,14,14,0.9); backdrop-filter: blur(8px);">
              {/* Chat header */}
              <div class="flex items-center justify-between px-3 py-2 border-b border-neutral-800">
                <span class="text-10px text-neutral-400 uppercase tracking-wider">chat</span>
                <button
                  class="text-neutral-500 hover:text-neutral-300 text-12px px-1"
                  onClick={() => setChatOpen(false)}
                >x</button>
              </div>
              {/* Chat messages */}
              <div
                ref={desktopChatLogEl!}
                class="overflow-y-auto px-3 py-2 font-mono text-10px leading-relaxed"
                style="height: calc(100% - 90px);"
              >
                <For each={logs()}>
                  {l => <div class={`text-neutral-500 ${l.cls} mb-0.5`}>{l.text}</div>}
                </For>
                <Show when={logs().length === 0}>
                  <div class="text-neutral-700 text-9px py-4 text-center">
                    type to chat - /nick name
                  </div>
                </Show>
              </div>
              {/* Quick reactions */}
              <div class="flex gap-1 px-3 py-1 border-t border-neutral-800/50">
                {['nh', 'gg', 'wp', 'lol', '...'].map(r =>
                  <button
                    class="text-8px px-2 py-0.5 rounded border border-neutral-800 text-neutral-600 hover:text-neutral-400 active:text-zec-yellow transition-colors"
                    onClick={() => { send({ type: 'Chat', text: r }); log(`you: ${r}`, 'text-white') }}
                  >{r}</button>
                )}
              </div>
              {/* Chat input */}
              <form class="flex border-t border-neutral-800" onSubmit={(e) => {
                e.preventDefault()
                const input = e.currentTarget.querySelector('input') as HTMLInputElement
                const msg = input.value.trim()
                if (!msg) return
                input.value = ''
                if (msg.startsWith('/nick ')) {
                  const newName = msg.slice(6).trim()
                  if (newName) {
                    setName(newName)
                    localStorage.setItem('poker_nickname', newName)
                    log(`nickname set to ${newName}`, 'text-neutral-400')
                  }
                  return
                }
                send({ type: 'Chat', text: msg })
                log(`you: ${msg}`, 'text-white')
              }}>
                <input
                  class="flex-1 bg-transparent text-10px px-3 py-2 text-white outline-none placeholder-neutral-700"
                  placeholder="chat..."
                  maxLength={200}
                />
                <button type="submit" class="text-9px px-3 text-neutral-600 hover:text-neutral-400">send</button>
              </form>
            </div>
          </Show>

          {/* Chat toggle button */}
          <button
            class="flex items-center gap-2 px-4 py-2 rounded-full border border-neutral-700 hover:border-zec-yellow/50 transition-colors shadow-lg"
            style="background: rgba(14,14,14,0.85); backdrop-filter: blur(4px);"
            onClick={() => { setChatOpen(c => !c); setChatUnread(0) }}
          >
            <span class="text-11px text-neutral-400">Chat</span>
            <Show when={chatUnread() > 0}>
              <span class="min-w-5 h-5 rounded-full bg-zec-yellow text-black text-9px font-bold flex items-center justify-center px-1">{chatUnread()}</span>
            </Show>
          </button>
        </div>

        {/* ---- Video overlays (top-right) ---- */}
        <div class="absolute top-12 right-4 z-20 flex flex-col gap-2">
          <Show when={media()?.remoteStream()}>
            <video
              class="w-24 h-18 rounded-lg border border-neutral-700 object-cover shadow-lg"
              autoplay playsinline
              ref={(el: HTMLVideoElement) => { el.srcObject = media()!.remoteStream() }}
            />
          </Show>
          <Show when={media()?.localStream() && media()?.camEnabled()}>
            <video
              class="w-18 h-14 rounded-lg border border-neutral-800 object-cover opacity-60"
              autoplay playsinline muted
              ref={(el: HTMLVideoElement) => { el.srcObject = media()!.localStream() }}
            />
          </Show>
        </div>
      </div>
    )
  }

  // ========================================================================
  // MOBILE GAME VIEW - original layout, unchanged
  // ========================================================================

  function MobileGameView() {
    return (
      <div class="px-2 lg:flex lg:gap-4">
        <div class="lg:flex-1">
          {/* status bar */}
          <div class="flex justify-between px-2 py-1.5 text-9px text-neutral-500 uppercase tracking-wider">
            <span>
              hand #{handNum()}
              <Show when={deckVerified()}>
                <span class="text-green-400 ml-1" title="deck verified via Chaum-Pedersen proof">OK</span>
              </Show>
            </span>
            <Show when={juryProgress()}>
              <span class="text-zec-yellow animate-pulse">{juryProgress()}</span>
            </Show>
            <span class="flex items-center gap-2">
              <span>you: {getPositionShort(mySeat(), button(), maxSeats())}</span>
              <button
                class={`px-1.5 py-0.5 rounded text-7px border ${broadcasting() ? 'border-red-500 text-red-400 bg-red-900/20' : 'border-neutral-700 text-neutral-600 hover:text-neutral-400'}`}
                onClick={() => setBroadcasting(b => !b)}
              >{broadcasting() ? 'LIVE' : 'broadcast'}</button>
              <Show when={broadcasting()}>
                <button
                  class={`px-1.5 py-0.5 rounded text-7px border ${showMyCards() ? 'border-zec-yellow text-zec-yellow' : 'border-neutral-700 text-neutral-600'}`}
                  onClick={() => setShowMyCards(s => !s)}
                >{showMyCards() ? 'cards: shown' : 'cards: hidden'}</button>
              </Show>
              <button
                class="px-1.5 py-0.5 rounded text-7px border border-red-900 text-red-400 hover:bg-red-900/20"
                onClick={() => { if (confirm('Leave table and cash out?')) send({ type: 'Leave' }) }}
              >leave</button>
            </span>
          </div>

          {/* felt */}
          <div class="bg-zec-felt border-2 border-zec-feltb rounded-15 sm:rounded-25 px-2 sm:px-5 py-4 sm:py-6 relative" style="min-height: 220px; box-shadow: inset 0 2px 20px rgba(0,0,0,0.4)">
            <TableOverlays />

            {/* opponent (top) */}
            <div class="absolute top--4 left-50% -translate-x-50%">
              <OpponentBox />
            </div>

            {/* dealer chip */}
            <Show when={button() === mySeat()}>
              <div class="absolute bottom-12 rounded-full w-5.5 h-5.5 bg-zec-yellow text-black text-9px font-bold leading-5.5 text-center border-2 border-zec-gold z-5"
                style="left: calc(50% + 55px)">D</div>
            </Show>
            <Show when={button() === opp()}>
              <div class="absolute top-12 rounded-full w-5.5 h-5.5 bg-zec-yellow text-black text-9px font-bold leading-5.5 text-center border-2 border-zec-gold z-5"
                style="left: calc(50% + 55px)">D</div>
            </Show>

            {/* deck + board */}
            <div class="flex gap-1.5 justify-center items-center my-13">
              <BoardCards />
            </div>

            {/* pot + result */}
            <div class="text-center font-mono text-14px font-500 min-h-5 relative">
              <Show when={lastResult()} fallback={
                <span class="text-zec-yellow">{pot() > 0 ? pot() : ''}</span>
              }>
                <span class={`animate-pulse ${lastResult()!.won ? 'text-green-400' : 'text-red-400'}`}>
                  {lastResult()!.won ? '+' : ''}{lastResult()!.amount}
                </span>
              </Show>
            </div>

            {/* you (bottom) */}
            <div class="absolute bottom--4 left-50% -translate-x-50%">
              <HeroBox />
            </div>
          </div>

          {/* actions */}
          <div class="flex gap-1 sm:gap-1.5 justify-center items-center py-2 sm:py-3 min-h-11 flex-wrap">
            <ActionButtons />
          </div>

          <AutoActionPresets />
          <HotkeyLegend />
          <MediaControls />
        </div>

        {/* log + chat sidebar */}
        <MobileChatSidebar />
      </div>
    )
  }

  // ========================================================================
  // MAIN RENDER
  // ========================================================================

  return (
    <>
      {/* Desktop full-screen game view - renders outside the panel container */}
      <Show when={view() === 'game' && isDesktop()}>
        <DesktopGameView />
      </Show>

      {/* All other views + mobile game - rendered inside the original panel */}
      <Show when={!(view() === 'game' && isDesktop())}>
        <div class="min-h-screen min-h-[100dvh] flex items-center justify-center p-1 sm:p-4 bg-zec-dark font-sans text-white">
          <div class="w-full max-w-160 lg:max-w-4xl xl:max-w-5xl">
            <div class="panel">
              <Titlebar />

              <Show when={view() === 'casino'}>
                <CasinoView />
              </Show>

              <Show when={view() === 'lobby'}>
                <LobbyView />
              </Show>

              <Show when={view() === 'waiting'}>
                <WaitingView />
              </Show>

              {/* Mobile game view */}
              <Show when={view() === 'game' && !isDesktop()}>
                <MobileGameView />
              </Show>

              <div class="text-center py-1.5 text-8px text-neutral-600 uppercase tracking-widest">poker.zk.bot</div>
            </div>
          </div>
        </div>
      </Show>
    </>
  )
}
