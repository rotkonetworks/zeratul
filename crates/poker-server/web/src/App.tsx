import { createSignal, For, Show, createEffect, onCleanup } from 'solid-js'
import { createSocket } from './ws'
import { Card } from './Card'
import Lobby, { type Table } from './Lobby'
import type { ServerMsg, CardJson, ValidAction } from './types'

export default function App() {
  const [view, setView] = createSignal<'casino' | 'lobby' | 'waiting' | 'game'>(
    location.pathname.length > 1 ? 'lobby' : 'casino'
  )
  const [selectedTable, setSelectedTable] = createSignal<Table | null>(null)
  const [name, setName] = createSignal('')
  const [mySeat, setMySeat] = createSignal(-1)
  const [oppName, setOppName] = createSignal('\u2014')
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
  const [gameStatus, setGameStatus] = createSignal('')

  const opp = () => mySeat() === 0 ? 1 : 0
  const myStack = () => stacks()[mySeat()] ?? 0
  const oppStack = () => stacks()[opp()] ?? 0
  const myBet = () => bets()[mySeat()] ?? 0
  const oppBet = () => bets()[opp()] ?? 0
  const isMyTurn = () => acting() === mySeat()

  function log(text: string, cls = '') {
    setLogs(l => [...l.slice(-60), { text, cls }])
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
        setActionTimer(msg.secondsLeft)
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
        break
      }
      case 'CommunityCards':
        setBoard(msg.cards)
        setBets([0, 0])
        log(`${msg.phase}: ${msg.cards.map(c => c.rank + c.suit).join(' ')}`, 'c-green')
        break
      case 'PotUpdate':
        setPot(msg.pots.reduce((s, p) => s + p.amount, 0))
        break
      case 'Showdown':
        for (const [seat, cards] of msg.hands) {
          if (seat === opp()) { setOppCards(cards); setOppRevealed(true) }
        }
        log('showdown', 'c-green')
        break
      case 'PotAwarded':
        log(`${msg.seat === mySeat() ? 'you' : 'opp'} wins ${msg.amount}${msg.amount === 0 ? ' (split)' : ''}`, 'c-zec-yellow font-500')
        break
      case 'HandComplete':
        setStacks(msg.stacks)
        setBets([0, 0])
        setActions([])
        setActing(-1)
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
          case '1': setRaiseVal(Math.max(Math.floor(p / 2), min)); break
          case '2': setRaiseVal(Math.max(Math.floor(p * 3 / 4), min)); break
          case '3': setRaiseVal(Math.max(p, min)); break
          case '4': setRaiseVal(Math.max(p * 2, min)); break
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

  return (
    <div class="min-h-screen min-h-[100dvh] flex items-center justify-center p-1 sm:p-4 bg-zec-dark font-sans text-white">
      <div class="w-full max-w-160 sm:max-w-160">
        <div class="panel">
          {/* titlebar */}
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

          {/* casino lobby */}
          <Show when={view() === 'casino'}>
            <Lobby
              hasWallet={true /* TODO: detect zafu */}
              onJoin={(table, playerName) => {
                setSelectedTable(table)
                setName(playerName)
                fetch('/new', { redirect: 'follow' }).then(resp => {
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
          </Show>

          {/* old lobby (for direct room links) */}
          <Show when={view() === 'lobby'}>
            <div class="p-8 text-center">
              <div class="text-zec-yellow text-10px font-semibold uppercase tracking-3px mb-5">
                heads-up no-limit hold'em
              </div>
              {/* game parameters — only for host (creating table) */}
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
          </Show>

          {/* waiting */}
          <Show when={view() === 'waiting'}>
            <div class="p-10 text-center">
              <div class="text-zec-yellow text-11px uppercase tracking-2px mb-4">
                waiting for opponent
              </div>
              <Show when={inviteUrl()}>
                <div class="mb-4">
                  <div class="text-neutral-500 text-9px uppercase tracking-wider mb-2">share invite link</div>
                  <div
                    class="input-field text-11px text-center cursor-pointer select-all"
                    onClick={() => navigator.clipboard?.writeText(inviteUrl())}
                    title="click to copy"
                  >
                    {inviteUrl()}
                  </div>
                  <div class="text-neutral-600 text-8px mt-1">click to copy</div>
                </div>
              </Show>
              <Show when={pendingRules() && !pendingRules()?.fromSelf}>
                <div class="mt-4 p-4 border border-neutral-700 rounded">
                  <div class="text-neutral-400 text-10px uppercase tracking-wider mb-2">opponent proposes</div>
                  <div class="text-white text-12px font-mono mb-3">
                    {pendingRules()!.smallBlind}/{pendingRules()!.bigBlind} blinds · {pendingRules()!.buyin} buyin
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
          </Show>

          {/* game */}
          <Show when={view() === 'game'}>
            <div class="px-2">
              {/* status bar */}
              <div class="flex justify-between px-2 py-1.5 text-9px text-neutral-500 uppercase tracking-wider">
                <span>
                  hand #{handNum()}
                  <Show when={deckVerified()}>
                    <span class="text-green-400 ml-1" title="deck verified via Chaum-Pedersen proof">✓</span>
                  </Show>
                </span>
                <Show when={juryProgress()}>
                  <span class="text-zec-yellow animate-pulse">{juryProgress()}</span>
                </Show>
                <span>you: {button() === mySeat() ? 'BTN/SB' : 'BB'}</span>
              </div>

              {/* felt */}
              <div class="bg-zec-felt border-2 border-zec-feltb rounded-15 sm:rounded-25 px-2 sm:px-5 py-4 sm:py-6 relative" style="min-height: 220px; box-shadow: inset 0 2px 20px rgba(0,0,0,0.4)">

                {/* disconnect overlay */}
                <Show when={oppDisconnected()}>
                  <div class="absolute inset-0 bg-black/60 z-10 flex items-center justify-center rounded-25">
                    <div class="text-center">
                      <div class="text-red-400 text-11px uppercase tracking-wider mb-1">opponent disconnected</div>
                      <div class="font-mono text-18px text-white">{reconnectCountdown()}s</div>
                      <div class="text-neutral-500 text-9px">waiting for reconnect</div>
                    </div>
                  </div>
                </Show>

                {/* shuffle/status overlay */}
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

                {/* opponent (top) */}
                <div class="absolute top--4 left-50% -translate-x-50% text-center w-44">
                  <div class={`inline-block px-3 py-1 bg-zec-surface border ${acting() === opp() ? 'border-zec-yellow shadow-[0_0_8px_rgba(244,183,40,0.3)]' : oppDisconnected() ? 'border-red-800' : 'border-neutral-800'}`}>
                    <div class={`text-9px font-semibold uppercase tracking-wider ${acting() === opp() ? 'text-zec-yellow' : oppDisconnected() ? 'text-red-400' : 'text-neutral-500'}`}>
                      {oppName()} <span class="text-neutral-600">{button() === opp() ? 'BTN/SB' : 'BB'}</span> {oppDisconnected() ? '(dc)' : ''}
                    </div>
                    <div class="font-mono text-13px text-zec-yellow">{oppStack()}</div>
                    <Show when={acting() === opp() && actionTimer() > 0}>
                      <div class={`font-mono text-9px ${actionTimer() <= 10 ? 'text-red-400' : 'text-neutral-500'}`}>{actionTimer()}s</div>
                    </Show>
                  </div>
                  <div class="flex gap-1 justify-center mt-1.5">
                    <Show when={oppRevealed() && oppCards()} fallback={
                      <Show when={myCards()}>
                        <Card /><Card />
                      </Show>
                    }>
                      <Card card={oppCards()![0]} />
                      <Card card={oppCards()![1]} />
                    </Show>
                  </div>
                  <div class="font-mono text-11px text-neutral-400 mt-0.5 h-4">{oppBet() > 0 ? oppBet() : ''}</div>
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
                  {/* deck on table — shows shuffle status */}
                  <Show when={board().length === 0}>
                    <div class="relative w-12 h-17 mr-2" title={deckVerified() ? 'deck verified (Chaum-Pedersen)' : gameStatus() || 'deck'}>
                      {/* stacked card backs */}
                      <div class="absolute inset-0 rounded-sm border border-neutral-700 bg-zec-surface"
                        style="transform: rotate(-2deg); background-image: repeating-linear-gradient(45deg,transparent,transparent 3px,rgba(244,183,40,0.05) 3px,rgba(244,183,40,0.05) 4px)" />
                      <div class="absolute inset-0 rounded-sm border border-neutral-700 bg-zec-surface"
                        style="transform: rotate(1deg); background-image: repeating-linear-gradient(45deg,transparent,transparent 3px,rgba(244,183,40,0.05) 3px,rgba(244,183,40,0.05) 4px)" />
                      <div class={`absolute inset-0 rounded-sm border bg-zec-surface flex items-center justify-center text-9px font-bold ${
                        gameStatus().includes('shuffl') || gameStatus().includes('key') ? 'border-zec-yellow animate-pulse text-zec-yellow' :
                        deckVerified() ? 'border-green-500 text-green-400' :
                        'border-neutral-700 text-neutral-600'
                      }`} style="background-image: repeating-linear-gradient(45deg,transparent,transparent 3px,rgba(244,183,40,0.05) 3px,rgba(244,183,40,0.05) 4px)">
                        {gameStatus().includes('shuffl') || gameStatus().includes('key') || gameStatus().includes('prov') ? '...' :
                         deckVerified() ? '✓' : '52'}
                      </div>
                    </div>
                  </Show>
                  <For each={board()}>
                    {c => <Card card={c} size="lg" />}
                  </For>
                </div>

                {/* pot */}
                <div class="text-center font-mono text-14px font-500 text-zec-yellow min-h-5">
                  {pot() > 0 ? pot() : ''}
                </div>

                {/* you (bottom) */}
                <div class="absolute bottom--4 left-50% -translate-x-50% text-center w-44">
                  <div class="font-mono text-11px text-neutral-400 mb-0.5 h-4">{myBet() > 0 ? myBet() : ''}</div>
                  <div class="flex gap-1 justify-center mb-1.5">
                    <Show when={myCards()}>
                      <Card card={myCards()![0]} />
                      <Card card={myCards()![1]} />
                    </Show>
                  </div>
                  <div class={`inline-block px-3 py-1 bg-zec-surface border ${acting() === mySeat() ? 'border-zec-yellow shadow-[0_0_8px_rgba(244,183,40,0.3)]' : 'border-neutral-800'}`}>
                    <Show when={acting() === mySeat() && actionTimer() > 0}>
                      <div class={`font-mono text-9px ${actionTimer() <= 10 ? 'text-red-400 animate-pulse' : 'text-neutral-500'}`}>{actionTimer()}s</div>
                    </Show>
                    <div class="font-mono text-13px text-zec-yellow">{myStack()}</div>
                    <div class={`text-9px font-semibold uppercase tracking-wider ${acting() === mySeat() ? 'text-zec-yellow' : 'text-neutral-500'}`}>
                      {name() || 'you'} <span class="text-neutral-600">{button() === mySeat() ? 'BTN/SB' : 'BB'}</span>
                    </div>
                  </div>
                </div>
              </div>

              {/* actions */}
              <div class="flex gap-1 sm:gap-1.5 justify-center items-center py-2 sm:py-3 min-h-11 flex-wrap">
                <Show when={isMyTurn() && actions().length > 0} fallback={
                  <Show when={acting() >= 0 && !isMyTurn()}>
                    <span class="text-neutral-600 text-10px uppercase tracking-wider">opponent to act</span>
                  </Show>
                }>
                  <For each={actions()}>
                    {a => {
                      if (a.kind === 'fold')
                        return <button class="btn btn-danger min-h-9 sm:min-h-auto px-3 sm:px-2" onClick={() => act('fold')}>fold</button>
                      if (a.kind === 'check')
                        return <button class="btn min-h-9 sm:min-h-auto px-3 sm:px-2" onClick={() => act('check')}>check</button>
                      if (a.kind === 'call')
                        return <button class="btn btn-primary min-h-9 sm:min-h-auto px-3 sm:px-2" onClick={() => act('call')}>call {a.min_amount}</button>
                      if (a.kind === 'bet')
                        return <div class="flex items-center gap-1">
                          <input class="input-field w-16 sm:w-20 text-center text-11px" type="number"
                            min={a.min_amount} max={a.max_amount}
                            value={raiseVal()} onInput={e => setRaiseVal(+e.currentTarget.value)} />
                          <button class="btn min-h-9 sm:min-h-auto px-3 sm:px-2" onClick={() => act('bet', raiseVal())}>bet</button>
                        </div>
                      if (a.kind === 'raise')
                        return <div class="flex items-center gap-1">
                          <Show when={!actions().some(x => x.kind === 'bet')}>
                            <input class="input-field w-16 sm:w-20 text-center text-11px" type="number"
                              min={a.min_amount} max={a.max_amount}
                              value={raiseVal()} onInput={e => setRaiseVal(+e.currentTarget.value)} />
                          </Show>
                          <button class="btn min-h-9 sm:min-h-auto px-3 sm:px-2" onClick={() => act('raise', raiseVal())}>raise</button>
                        </div>
                      if (a.kind === 'allin')
                        return <button class="btn btn-allin min-h-9 sm:min-h-auto px-3 sm:px-2" onClick={() => act('allin')}>all in</button>
                      return null
                    }}
                  </For>
                </Show>
              </div>

              {/* auto-action presets */}
              <div class="flex gap-1 justify-center py-1 flex-wrap">
                {(['check/fold', 'check', 'fold', 'call any'] as const).map(mode =>
                  <button
                    class={`text-8px px-2 py-0.5 rounded border ${autoAction() === mode ? 'border-zec-yellow text-zec-yellow bg-zec-yellow/10' : 'border-neutral-700 text-neutral-600 hover:text-neutral-400'}`}
                    onClick={() => setAutoAction(autoAction() === mode ? 'none' : mode)}
                  >
                    {mode}
                  </button>
                )}
              </div>

              {/* hotkey legend + mode toggle */}
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
                  F1 fold · F2 check/call · F3 raise · F4 pot · Space call · Q all-in · 1-4 sizing
                </Show>
                <Show when={keyMode() === 'vim'}>
                  f fold · d check · s call · r/w raise · a all-in · j/k size ±  · H ½p · M ¾p · G pot · L 2x
                </Show>
              </div>

              {/* media controls + video */}
              <div class="flex items-center justify-between px-1 py-1">
                <div class="flex gap-1">
                  <button
                    class={`text-9px px-2 py-0.5 rounded border ${media()?.micEnabled() ? 'border-green-500 text-green-400' : 'border-neutral-700 text-neutral-600 hover:text-neutral-400'}`}
                    onClick={() => media()?.toggleMic()}
                  >{media()?.micEnabled() ? 'mic on' : 'mic'}</button>
                  <button
                    class={`text-9px px-2 py-0.5 rounded border ${media()?.camEnabled() ? 'border-green-500 text-green-400' : 'border-neutral-700 text-neutral-600 hover:text-neutral-400'}`}
                    onClick={() => media()?.toggleCam()}
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

              {/* log + chat */}
              <div ref={logEl!} class="bg-zec-surface border border-neutral-800 p-2 max-h-28 overflow-y-auto font-mono text-10px mb-1 leading-relaxed">
                <For each={logs()}>
                  {l => <div class={`text-neutral-600 ${l.cls}`}>{l.text}</div>}
                </For>
              </div>
              <form class="flex gap-1 mb-2" onSubmit={(e) => {
                e.preventDefault()
                const input = e.currentTarget.querySelector('input') as HTMLInputElement
                const msg = input.value.trim()
                if (!msg) return
                input.value = ''
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
          </Show>

          <div class="text-center py-1.5 text-8px text-neutral-600 uppercase tracking-widest">poker.zk.bot</div>
        </div>
      </div>
    </div>
  )
}
