import { createSignal, For, Show, createEffect, onCleanup } from 'solid-js'
import { createSocket } from './ws'
import { Card } from './Card'
import Lobby, { type Table } from './Lobby'
import { detectZafu } from './zid/provider'
import { getPositionShort } from './positions'
import { requestPokerDkg, requestDeletePokerMultisig, requestPokerSign } from './dkg'
import type { ServerMsg, CardJson, ValidAction } from './types'

export default function App() {
  const [view, setView] = createSignal<'casino' | 'lobby' | 'waiting' | 'deposit' | 'game' | 'settlement'>(
    location.pathname.length > 1 ? 'lobby' : 'casino'
  )
  // settlement state: populated by PayoutSigningRequest, transitions on PayoutComplete/PayoutFailed
  type SettlementStatus =
    | { phase: 'pending' }
    | { phase: 'signing' }
    | { phase: 'complete'; txid: string }
    | { phase: 'failed'; reason: string }
  const [settleStatus, setSettleStatus] = createSignal<SettlementStatus>({ phase: 'pending' })
  const [settleRelayRoom, setSettleRelayRoom] = createSignal('')
  const [settlePlan, setSettlePlan] = createSignal<{ seat: number; address: string; amount_zat: number }[]>([])
  const [settlePrioritySeat, setSettlePrioritySeat] = createSignal(-1)
  const [settleReason, setSettleReason] = createSignal('')
  const [settleFrostRelay, setSettleFrostRelay] = createSignal('')
  // tick for the fallback-signer countdown. Server swaps priority every 90s of inactivity;
  // the SPA tracks Date.now() at each PayoutSigningRequest and renders 90 - elapsed.
  const SETTLE_FALLBACK_SECS = 90
  const [settleBroadcastAt, setSettleBroadcastAt] = createSignal(0)
  const [settleFallbackTick, setSettleFallbackTick] = createSignal(SETTLE_FALLBACK_SECS)
  createEffect(() => {
    const baseAt = settleBroadcastAt()
    const phase = settleStatus().phase
    if (!baseAt || (phase !== 'pending' && phase !== 'signing')) return
    const tick = () => {
      const left = Math.max(0, SETTLE_FALLBACK_SECS - Math.floor((Date.now() - baseAt) / 1000))
      setSettleFallbackTick(left)
    }
    tick()
    const iv = setInterval(tick, 1000)
    onCleanup(() => clearInterval(iv))
  })
  // deposit-panel state — populated by RoomInfo + DepositStatus
  const [requiredDeposit, setRequiredDeposit] = createSignal(0)
  const [depositBuyinZat, setDepositBuyinZat] = createSignal(0)
  const [depositFeePerSeat, setDepositFeePerSeat] = createSignal(0)
  const [seatAddresses, setSeatAddresses] = createSignal<(string | null)[]>([])
  const [depositA, setDepositA] = createSignal(0)
  const [depositB, setDepositB] = createSignal(0)
  const [depositReady, setDepositReady] = createSignal(false)
  // payout address the player pastes in the deposit panel; required before "Send with zafu"
  // is enabled. embedded into the deposit memo so the escrow scanner knows where to refund.
  const [payoutOverride, setPayoutOverride] = createSignal<string | null>(null)
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
  const currentRoom = () => location.pathname.replace(/^\/+|\/+$/g, '')
  // last_session: {room, name} saved on Seated, cleared on Leave — used to detect reconnect
  const initialLastSession = (() => {
    try { return JSON.parse(localStorage.getItem('poker_last_session') || 'null') as { room: string; name: string } | null } catch { return null }
  })()
  const [lastSession, setLastSession] = createSignal(initialLastSession)
  const isReconnect = () => {
    const s = lastSession()
    return !!s && !!currentRoom() && s.room === currentRoom()
  }
  const initialName = (isReconnect() && initialLastSession?.name) || localStorage.getItem('poker_nickname') || ''
  const [name, setName] = createSignal(initialName)
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
  // guard so re-broadcast RoomInfo doesn't spawn a second zafu popup
  let dkgStartedFor = ''
  const [pendingRules, setPendingRules] = createSignal<{ buyin: number; smallBlind: number; bigBlind: number; turnTimeout: number; fromSelf: boolean } | null>(null)
  const [oppDisconnected, setOppDisconnected] = createSignal(false)
  const [reconnectCountdown, setReconnectCountdown] = createSignal(0)
  // tracked so OpponentReconnected / Seated / OpponentLeft can clear the running tick
  let reconnectInterval: ReturnType<typeof setInterval> | null = null
  const clearReconnectInterval = () => {
    if (reconnectInterval !== null) { clearInterval(reconnectInterval); reconnectInterval = null }
  }
  const [actionTimer, setActionTimer] = createSignal(0)
  const [autoAction, setAutoAction] = createSignal<'none' | 'check/fold' | 'check' | 'fold' | 'call any'>('none')
  const [deckVerified, setDeckVerified] = createSignal(false)
  const [gameStatus, setGameStatus] = createSignal('')
  const [lastResult, setLastResult] = createSignal<{ won: boolean; amount: number } | null>(null)

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
  const isMyTurn = () => acting() === mySeat() && !oppDisconnected()

  function log(text: string, cls = '') {
    setLogs(l => [...l.slice(-60), { text, cls }])
  }

  function onMsg(msg: ServerMsg) {
    switch (msg.type) {
      case 'Seated':
        setMySeat(msg.seat)
        setView('waiting')
        // clear stale connection state — server only sends OpponentDisconnected when an opp
        // *transitions* offline, never a snapshot. On our own reconnect, we'd otherwise carry
        // over any oppDisconnected=true from before our WS bounced.
        clearReconnectInterval()
        setOppDisconnected(false)
        setReconnectCountdown(0)
        // remember the seat so a future visit to this room shows reconnect UI
        if (currentRoom() && msg.name) {
          const s = { room: currentRoom(), name: msg.name }
          localStorage.setItem('poker_last_session', JSON.stringify(s))
          setLastSession(s)
        }
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
        clearReconnectInterval()
        setOppDisconnected(false)
        setReconnectCountdown(0)
        setOppName('\u2014')
        setActions([])
        setView('waiting')
        log('opponent left')
        break
      case 'OpponentDisconnected': {
        // clear any prior countdown so we don't end up with two racing intervals
        clearReconnectInterval()
        setOppDisconnected(true)
        setReconnectCountdown(msg.reconnect_secs)
        log(`opponent disconnected (${msg.reconnect_secs}s to reconnect)`, 'c-red')
        reconnectInterval = setInterval(() => {
          setReconnectCountdown(c => {
            if (c <= 1) { clearReconnectInterval(); return 0 }
            return c - 1
          })
        }, 1000)
        break
      }
      case 'OpponentReconnected':
        clearReconnectInterval()
        setOppDisconnected(false)
        setReconnectCountdown(0)
        log('opponent reconnected', 'c-green')
        break
      case 'TimerTick':
        setActionTimer(msg.seconds_left)
        break
      case 'ActionPaused':
        // freeze the visible action timer; the OPPONENT DISCONNECTED overlay covers the table
        setActionTimer(0)
        log(`hand paused while seat ${msg.seat} is offline`, 'c-zec-yellow')
        break
      case 'ActionResumed':
        setActionTimer(msg.seconds_left)
        log(`hand resumed (seat ${msg.seat} to act, ${msg.seconds_left}s)`, 'c-green')
        break
      case 'ActionTimeout':
        log(`${msg.seat === mySeat() ? 'you' : 'opp'} timed out (auto-fold)`, 'c-red')
        setActionTimer(0)
        break
      case 'HandStarted':
        setView('game')
        setStacks(msg.stacks)
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
        const stackBefore = stacks()[msg.seat] ?? 0
        const s = [...stacks()]
        s[msg.seat] = msg.new_stack
        setStacks(s)
        const increment = stackBefore - msg.new_stack
        const b = [...bets()]
        if (increment > 0) b[msg.seat] = (b[msg.seat] ?? 0) + increment
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
          setLogs([])
          log(`table: ${msg.code}`, 'c-green')
        }
        if (msg.escrow && msg.escrow.length > 5) {
          setEscrow(msg.escrow)
        }
        if (msg.required_deposit) setRequiredDeposit(msg.required_deposit)
        if (typeof msg.buyin_zat === 'number') setDepositBuyinZat(msg.buyin_zat)
        if (typeof msg.fee_per_seat === 'number') setDepositFeePerSeat(msg.fee_per_seat)
        if (typeof msg.frost_relay_url === 'string') setSettleFrostRelay(msg.frost_relay_url)
        if (msg.seat_addresses && msg.seat_addresses.length > 0) {
          setSeatAddresses(msg.seat_addresses)
        }
        // DKG-mode escrow: kick off zafu's join flow once per table
        if (
          msg.frost_relay_url && msg.frost_room_code &&
          (!msg.escrow || msg.escrow.length === 0) &&
          dkgStartedFor !== msg.code
        ) {
          dkgStartedFor = msg.code
          log('setting up multisig escrow (zafu)...', 'c-zec-yellow')
          void requestPokerDkg({
            relayUrl: msg.frost_relay_url,
            roomCode: msg.frost_room_code,
            threshold: 2,
            maxSigners: 3,
            labelPrefix: `POKER-${msg.code}`,
          }).then(res => {
            if (res.success) {
              log(`multisig ready: ${res.address.slice(0, 16)}…`, 'c-green')
              send({ type: 'DkgComplete', escrow_ua: res.address, orchard_fvk: res.orchardFvk })
            } else {
              log(`multisig setup failed: ${res.error}`, 'c-red')
              dkgStartedFor = ''
            }
          })
        }
        break
      case 'DepositStatus': {
        if (msg.seat_addresses && msg.seat_addresses.length > 0) {
          setSeatAddresses(msg.seat_addresses)
        }
        if (msg.escrow_address) setEscrow(msg.escrow_address)
        setDepositA(msg.player_a_deposit)
        setDepositB(msg.player_b_deposit)
        setRequiredDeposit(msg.required)
        setDepositReady(msg.ready)
        // once DKG produced seat addresses, sit users in the deposit view
        if (mySeat() >= 0 && !msg.ready && view() !== 'game' && msg.seat_addresses && msg.seat_addresses.some(a => a)) {
          setView('deposit')
        }
        break
      }
      case 'GameOver': {
        const myPayout = msg.payouts.find((p: any) => p[0] === mySeat())
        log(`game over: ${msg.reason}`, 'c-red font-500')
        if (myPayout) log(`your payout: ${myPayout[1]}`, 'c-zec-yellow font-500')
        setActions([])
        setActing(-1)
        setSettleReason(msg.reason)
        break
      }
      case 'PayoutSigningRequest': {
        log(`settling: ${msg.plan.map(p => `seat${p.seat}=${p.amount_zat}`).join(', ')}`, 'c-zec-yellow')
        setSettleRelayRoom(msg.relay_room)
        setSettlePlan(msg.plan)
        setSettlePrioritySeat(msg.priority_seat)
        setSettleStatus({ phase: 'pending' })
        // reset the fallback countdown; server will re-broadcast PayoutSigningRequest with
        // a flipped priority_seat in SETTLE_FALLBACK_SECS if the current signer doesn't act
        setSettleBroadcastAt(Date.now())
        setSettleFallbackTick(SETTLE_FALLBACK_SECS)
        setView('settlement')
        setActions([])
        setActing(-1)
        break
      }
      case 'OpponentAbandoned': {
        log(`opponent (seat ${msg.seat}) disconnected and didn't return`, 'c-red')
        setSettleReason(`seat ${msg.seat} abandoned`)
        // PayoutSigningRequest follows from the server's auto-settle path; view-flip happens there
        break
      }
      case 'PayoutComplete': {
        log(`✓ paid out: tx ${msg.txid}`, 'c-green font-500')
        setSettleStatus({ phase: 'complete', txid: msg.txid })
        setView('settlement')
        // settlement done — drop the reconnect marker; the "return to lobby" button can
        // safely take the user home without auto-rejoining a settled room
        localStorage.removeItem('poker_last_session')
        setLastSession(null)
        // schedule deletion of the multisig vault 24h from now — it's spent + useless
        void requestDeletePokerMultisig({
          multisigLabel: `POKER-${roomCode()}`,
          delayMs: 24 * 60 * 60 * 1000,
        })
        break
      }
      case 'PayoutFailed': {
        log(`✗ payout failed: ${msg.reason}`, 'c-red font-500')
        setSettleStatus({ phase: 'failed', reason: msg.reason })
        setView('settlement')
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

  function leaveTable() {
    if (!confirm('Leave table and cash out?')) return
    send({ type: 'Leave' })
    setActions([])
    setActing(-1)
    // KEEP `poker_last_session` so a reload during settlement triggers reconnect (server
    // replays PayoutSigningRequest into the settlement view rather than dumping us at the
    // rename UI). PayoutComplete / PayoutFailed handlers + the manual "return to lobby"
    // button clear it. For bot tables we still want a quick bounce to lobby; the 5s
    // fallback below handles that.
    setTimeout(() => {
      if (view() !== 'settlement') {
        // bot table or server didn't open a settlement → safe to scrub session + bounce
        localStorage.removeItem('poker_last_session')
        setLastSession(null)
        history.pushState(null, '', '/')
        setView('casino')
      }
    }, 5000)
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

  return (
    <div class="min-h-screen min-h-[100dvh] flex items-center justify-center p-1 sm:p-4 bg-zec-dark font-sans text-white">
      <div class="w-full max-w-160 lg:max-w-4xl xl:max-w-5xl">
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
              hasWallet={hasWallet()}
              pubkey={walletPubkey()}
              onJoin={(table, playerName, bot) => {
                setSelectedTable(table)
                setName(playerName)
                const params = new URLSearchParams({
                  sb: String(table.sb), bb: String(table.bb),
                  buyin: String(table.buyin), timeout: String(table.timeout),
                  rake_bps: String(table.rakeBps), rake_cap: String(table.rakeCap),
                  access: bot ? 'public' : 'private',
                  ...(bot ? { bot: 'true' } : {}),
                })
                fetch(`/new?${params}`, { redirect: 'follow' }).then(resp => {
                  const url = resp.url || resp.headers.get('location') || ''
                  const code = url.split('/').pop() || ''
                  if (code) {
                    history.pushState(null, '', '/' + code)
                    setView('lobby')
                  }
                })
              }}
              onJoinCode={(code, playerName) => {
                setName(playerName)
                history.pushState(null, '', '/' + code)
                setView('lobby')
              }}
            />
          </Show>

          {/* old lobby (for direct room links) */}
          <Show when={view() === 'lobby'}>
            <div class="p-8 text-center">
              <div class="text-zec-yellow text-10px font-semibold uppercase tracking-3px mb-5">
                no-limit hold'em
              </div>
              {/* game parameters — only for host (creating table) */}
              <Show when={location.pathname.length <= 1} fallback={
                <Show when={isReconnect()} fallback={
                  <div class="text-neutral-500 text-11px tracking-wider mb-4">
                    joining table &middot; host sets rules
                  </div>
                }>
                  <div class="text-zec-yellow text-11px tracking-wider mb-1 uppercase">reconnecting</div>
                  <div class="text-neutral-400 text-10px mb-4">as {lastSession()!.name}</div>
                </Show>
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
                    {isReconnect() ? 'reconnect' : location.pathname.length > 1 ? 'sit down' : 'create table'}
                  </button>
                </div>
              </div>
            </div>
          </Show>

          {/* waiting */}
          <Show when={view() === 'waiting'}>
            <div class="p-10 text-center relative">
              <button
                class="absolute top-2 right-2 px-1.5 py-0.5 rounded text-7px border border-red-900 text-red-400 hover:bg-red-900/20"
                onClick={leaveTable}
                title="leave table — settles escrow and pays out"
              >leave</button>
              <div class="text-zec-yellow text-11px uppercase tracking-2px mb-4">
                waiting for players
              </div>

              {/* invite from contacts */}
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

          {/* deposit */}
          <Show when={view() === 'deposit'}>
            <div class="p-6 relative">
              <button
                class="absolute top-2 right-2 px-1.5 py-0.5 rounded text-7px border border-red-900 text-red-400 hover:bg-red-900/20"
                onClick={leaveTable}
                title="leave table — refund both deposits"
              >leave</button>
              <div class="text-zec-yellow text-11px uppercase tracking-2px mb-1 text-center">deposit to play</div>
              <div class="text-neutral-500 text-9px text-center mb-4">2-of-3 multisig escrow (you + opp + house)</div>

              {(() => {
                const seat = mySeat()
                const myAddr = seatAddresses()[seat] ?? null
                const myDep = seat === 0 ? depositA() : depositB()
                const oppDep = seat === 0 ? depositB() : depositA()
                const req = requiredDeposit()
                const myReady = myDep >= req
                const oppReady = oppDep >= req
                const reqZec = (req / 1e8).toFixed(8).replace(/0+$/, '').replace(/\.$/, '')
                const myZec = (myDep / 1e8).toFixed(8).replace(/0+$/, '').replace(/\.$/, '')
                const oppZec = (oppDep / 1e8).toFixed(8).replace(/0+$/, '').replace(/\.$/, '')
                return <>
                  <div class="mb-4 p-3 border border-neutral-800 rounded-lg bg-zec-surface">
                    <div class="text-neutral-500 text-9px uppercase tracking-wider mb-2">your deposit address</div>
                    <Show when={myAddr} fallback={
                      <div class="text-neutral-600 text-9px">waiting for multisig setup…</div>
                    }>
                      <div
                        class="font-mono text-9px text-zec-yellow break-all cursor-pointer select-all"
                        onClick={() => { navigator.clipboard?.writeText(myAddr!); log('copied deposit address', 'c-green') }}
                        title="click to copy"
                      >{myAddr}</div>
                      <div class="mt-2 flex items-center gap-2">
                        <span class="text-neutral-500 text-9px uppercase tracking-wider">send</span>
                        <span class="text-zec-yellow text-11px tabular" title="buy-in + your share of the on-chain payout fee">{reqZec} ZEC</span>
                      </div>
                      {depositFeePerSeat() > 0 && (
                        <div class="mt-1 text-neutral-500 text-9px tabular">
                          = {(depositBuyinZat() / 1e8).toFixed(8).replace(/0+$/, '').replace(/\.$/, '')} buy-in
                          {' + '}
                          {(depositFeePerSeat() / 1e8).toFixed(8).replace(/0+$/, '').replace(/\.$/, '')} payout fee
                        </div>
                      )}
                      <div class="mt-3 pt-2 border-t border-neutral-800">
                        <div class="text-neutral-500 text-9px uppercase tracking-wider mb-1">payouts will go to</div>
                        <input
                          class="w-full bg-zec-bg border border-neutral-700 rounded px-2 py-1 font-mono text-10px text-zec-yellow"
                          placeholder="u1... (your orchard UA)"
                          value={payoutOverride() ?? ''}
                          onInput={(e) => {
                            const v = e.currentTarget.value.trim()
                            if (v === '') setPayoutOverride(null)
                            else if (v.startsWith('u1') || v.startsWith('utest1') || v.startsWith('uregtest1')) setPayoutOverride(v)
                            else setPayoutOverride(null)
                          }}
                        />
                        <div class="mt-1 text-neutral-600 text-9px">refunds and winnings land here. paste your zafu / wallet orchard UA.</div>
                      </div>
                      <button
                        class="mt-3 w-full btn btn-secondary text-10px disabled:opacity-40 disabled:cursor-not-allowed"
                        disabled={!payoutOverride()}
                        onClick={() => {
                          try {
                            const addr = payoutOverride()
                            if (!addr) { log('paste a payout address first', 'c-red'); return }
                            const providers = (window as any)[Symbol.for('penumbra')]
                            const extId = providers ? (Object.keys(providers)[0]?.replace('chrome-extension://','').replace(/\/$/, '')) : null
                            if (!extId) { log('zafu not detected', 'c-red'); return }
                            chrome.runtime.sendMessage(extId, {
                              type: 'send',
                              address: myAddr,
                              amount_zat: req,
                              memo: `zk.poker/v1/payout:${addr}`,
                            }, () => {})
                          } catch (e: any) { log(`zafu send failed: ${e?.message ?? e}`, 'c-red') }
                        }}
                      >Send with zafu</button>
                      <div class="mt-2 text-neutral-600 text-9px leading-relaxed">
                        sending from an external wallet? attach memo:{' '}
                        <span class="font-mono text-neutral-500">zk.poker/v1/payout:&lt;your-u1-address&gt;</span>
                      </div>
                    </Show>
                  </div>

                  <div class="grid grid-cols-2 gap-3 mb-4">
                    <div class="p-2 border border-neutral-800 rounded bg-zec-surface">
                      <div class="text-neutral-500 text-8px uppercase">you</div>
                      <div class="tabular text-11px mt-1">
                        <span class={myReady ? 'c-green' : 'c-zec-yellow'}>{myZec}</span>
                        <span class="text-neutral-600"> / {reqZec} ZEC</span>
                      </div>
                      <div class={`text-9px mt-1 ${myReady ? 'c-green' : 'text-neutral-500'}`}>{myReady ? '✓ deposited' : '⌛ waiting'}</div>
                    </div>
                    <div class="p-2 border border-neutral-800 rounded bg-zec-surface">
                      <div class="text-neutral-500 text-8px uppercase">opponent</div>
                      <div class="tabular text-11px mt-1">
                        <span class={oppReady ? 'c-green' : 'c-zec-yellow'}>{oppZec}</span>
                        <span class="text-neutral-600"> / {reqZec} ZEC</span>
                      </div>
                      <div class={`text-9px mt-1 ${oppReady ? 'c-green' : 'text-neutral-500'}`}>{oppReady ? '✓ deposited' : '⌛ waiting'}</div>
                    </div>
                  </div>

                  <div class="text-center text-neutral-500 text-9px">
                    table starts when both players have deposited
                  </div>
                </>
              })()}
            </div>
          </Show>

          {/* game */}
          <Show when={view() === 'game'}>
            <div class="px-2 lg:flex lg:gap-4">
             <div class="lg:flex-1">
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
                <span class="flex items-center gap-2">
                  <span>you: {getPositionShort(mySeat(), button(), maxSeats())}</span>
                  <button
                    class={`px-1.5 py-0.5 rounded text-7px border ${broadcasting() ? 'border-red-500 text-red-400 bg-red-900/20' : 'border-neutral-700 text-neutral-600 hover:text-neutral-400'}`}
                    onClick={() => setBroadcasting(b => !b)}
                    title={broadcasting() ? 'stop broadcasting to spectators' : 'broadcast game to spectators (public info only)'}
                  >{broadcasting() ? 'LIVE' : 'broadcast'}</button>
                  <Show when={broadcasting()}>
                    <button
                      class={`px-1.5 py-0.5 rounded text-7px border ${showMyCards() ? 'border-zec-yellow text-zec-yellow' : 'border-neutral-700 text-neutral-600'}`}
                      onClick={() => setShowMyCards(s => !s)}
                      title="toggle showing your hole cards to spectators"
                    >{showMyCards() ? 'cards: shown' : 'cards: hidden'}</button>
                  </Show>
                  <button
                    class="px-1.5 py-0.5 rounded text-7px border border-red-900 text-red-400 hover:bg-red-900/20"
                    onClick={leaveTable}
                    title="leave table — settles escrow and pays out"
                  >leave</button>
                </span>
              </div>

              {/* felt */}
              <div class="bg-zec-felt border-2 border-zec-feltb rounded-15 sm:rounded-25 px-2 sm:px-5 py-4 sm:py-6 lg:py-10 relative" style="min-height: 220px; box-shadow: inset 0 2px 20px rgba(0,0,0,0.4)">

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
                <div class="absolute top--4 left-50% -translate-x-50% text-center">
                  <div class="flex items-center justify-center gap-2">
                    <div class="font-mono text-11px text-zec-yellow whitespace-nowrap w-16 text-right">bet: {oppBet()}</div>
                    <div class={`inline-block px-3 py-1 bg-zec-surface border ${acting() === opp() ? 'border-zec-yellow shadow-[0_0_8px_rgba(244,183,40,0.3)]' : oppDisconnected() ? 'border-red-800' : 'border-neutral-800'}`}>
                      <div class={`text-9px font-semibold uppercase tracking-wider ${acting() === opp() ? 'text-zec-yellow' : oppDisconnected() ? 'text-red-400' : 'text-neutral-500'}`}>
                        {oppName()} <span class="text-neutral-600">{getPositionShort(opp(), button(), maxSeats())}</span> {oppDisconnected() ? '(dc)' : ''}
                      </div>
                      <div class="font-mono text-13px text-zec-yellow">{oppStack()}</div>
                      <Show when={acting() === opp() && actionTimer() > 0}>
                        <div class={`font-mono text-11px font-bold ${actionTimer() <= 5 ? 'text-red-500 animate-pulse' : actionTimer() <= 10 ? 'text-orange-400' : actionTimer() <= 20 ? 'text-zec-yellow' : 'text-neutral-400'}`}>{actionTimer()}s</div>
                      </Show>
                    </div>
                    <div class="w-16" aria-hidden="true"></div>
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

                {/* pot + deck + board — pot sits left of the community cards,
                    invisible spacer on the right keeps cards page-centered */}
                <div class="flex gap-3 justify-center items-center my-13">
                  <div class="font-mono text-13px font-500 text-zec-yellow whitespace-nowrap w-20 text-right">
                    <Show when={lastResult()} fallback={<>pot: {pot()}</>}>
                      <span class={`animate-pulse ${lastResult()!.won ? 'text-green-400' : 'text-red-400'}`}>
                        {lastResult()!.won ? '+' : ''}{lastResult()!.amount}
                      </span>
                    </Show>
                  </div>
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
                  <div class="w-20" aria-hidden="true"></div>
                </div>

                {/* you (bottom) */}
                <div class="absolute bottom--4 left-50% -translate-x-50% text-center">
                  <div class="flex gap-1 justify-center mb-1.5">
                    <Show when={myCards()}>
                      <Card card={myCards()![0]} />
                      <Card card={myCards()![1]} />
                    </Show>
                  </div>
                  <div class="flex items-center justify-center gap-2">
                    <div class="font-mono text-11px text-zec-yellow whitespace-nowrap w-16 text-right">bet: {myBet()}</div>
                    <div class={`inline-block px-3 py-1 bg-zec-surface border ${acting() === mySeat() ? 'border-zec-yellow shadow-[0_0_8px_rgba(244,183,40,0.3)]' : 'border-neutral-800'}`}>
                      <Show when={acting() === mySeat() && actionTimer() > 0}>
                        <div class={`font-mono text-11px font-bold ${actionTimer() <= 5 ? 'text-red-500 animate-pulse' : actionTimer() <= 10 ? 'text-orange-400' : actionTimer() <= 20 ? 'text-zec-yellow' : 'text-neutral-400'}`}>{actionTimer()}s</div>
                      </Show>
                      <div class="font-mono text-13px text-zec-yellow">{myStack()}</div>
                      <div class={`text-9px font-semibold uppercase tracking-wider ${acting() === mySeat() ? 'text-zec-yellow' : 'text-neutral-500'}`}>
                        {name() || 'you'} <span class="text-neutral-600">{button() === mySeat() ? 'BTN/SB' : 'BB'}</span>
                      </div>
                    </div>
                    <div class="w-16" aria-hidden="true"></div>
                  </div>
                </div>
              </div>

              {/* actions */}
              <div class="flex gap-1 sm:gap-1.5 justify-center items-center py-2 sm:py-3 min-h-11 flex-wrap">
                <Show when={isMyTurn() && actions().length > 0} fallback={
                  <Show when={acting() >= 0 && !isMyTurn()}>
                    <span class="text-neutral-600 text-10px uppercase tracking-wider">
                      {oppDisconnected() ? 'hand paused — opponent offline' : 'opponent to act'}
                    </span>
                  </Show>
                }>
                  {/* sizing buttons — Pluribus-style: 1/4, 1/2, 3/4, pot, 2x */}
                  {(() => {
                    const betAction = actions().find(v => v.kind === 'raise') || actions().find(v => v.kind === 'bet')
                    if (!betAction) return null
                    const p = pot()
                    const min = betAction.min_amount || 0
                    const max = betAction.max_amount || 0
                    const clamp = (v: number) => Math.min(Math.max(Math.round(v), min), max)
                    const sizes = [
                      { label: '¼', val: clamp(p / 4) },
                      { label: '½', val: clamp(p / 2) },
                      { label: '¾', val: clamp(p * 3 / 4) },
                      { label: 'pot', val: clamp(p) },
                      { label: '2x', val: clamp(p * 2) },
                    ].filter(s => s.val >= min && s.val <= max)
                    // dedupe sizes that collapse to same value
                    const unique = sizes.filter((s, i) => i === 0 || s.val !== sizes[i-1].val)
                    return <div class="flex gap-0.5 justify-center mb-1">
                      {unique.map(s =>
                        <button class={`btn btn-xs px-2 py-0.5 text-9px ${raiseVal() === s.val ? 'btn-active' : 'btn-ghost'}`}
                          onClick={() => setRaiseVal(s.val)}>{s.label}</button>
                      )}
                    </div>
                  })()}
                  {/* main action buttons */}
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
                          <input class="input-field w-14 sm:w-16 text-center text-11px" type="number"
                            min={a.min_amount} max={a.max_amount}
                            value={raiseVal()} onInput={e => setRaiseVal(+e.currentTarget.value)} />
                          <button class="btn min-h-9 sm:min-h-auto px-3 sm:px-2" onClick={() => act('bet', raiseVal())}>bet {raiseVal()}</button>
                        </div>
                      if (a.kind === 'raise')
                        return <div class="flex items-center gap-1">
                          <Show when={!actions().some(x => x.kind === 'bet')}>
                            <input class="input-field w-14 sm:w-16 text-center text-11px" type="number"
                              min={a.min_amount} max={a.max_amount}
                              value={raiseVal()} onInput={e => setRaiseVal(+e.currentTarget.value)} />
                          </Show>
                          <button class="btn min-h-9 sm:min-h-auto px-3 sm:px-2" onClick={() => act('raise', raiseVal())}>raise {raiseVal()}</button>
                        </div>
                      if (a.kind === 'allin')
                        return <button class="btn btn-allin min-h-9 sm:min-h-auto px-3 sm:px-2" onClick={() => act('allin')}>all in</button>
                      return null
                    }}
                  </For>
                </Show>
              </div>

              {/* auto-action presets — always visible */}
              <div class="flex gap-1.5 justify-center py-2">
                {(['check/fold', 'check', 'fold', 'call any'] as const).map(mode =>
                  <button
                    class={`text-9px px-3 py-1.5 rounded-md border transition-all ${
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
                  F1 fold · F2 check/call · F3 raise · F4 pot · Space call · Q all-in · 1-5 sizing
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

             </div>{/* end main column */}

              {/* log + chat sidebar */}
              <div class="lg:w-72 lg:flex-shrink-0">
              <div ref={logEl!} class="bg-zec-surface border border-neutral-800 p-2 max-h-28 lg:max-h-80 overflow-y-auto font-mono text-10px mb-1 leading-relaxed">
                <For each={logs()}>
                  {l => <div class={`text-neutral-600 ${l.cls}`}>{l.text}</div>}
                </For>
              </div>
              {/* quick reactions */}
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
                // slash commands
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
              </div>{/* end sidebar */}
            </div>
          </Show>

          {/* settlement view — on-chain payout in progress / complete / failed */}
          <Show when={view() === 'settlement'}>
            <div class="mx-auto max-w-md p-4 mt-2">
              <div class="text-center mb-3">
                <div class="text-zec-yellow text-12px uppercase tracking-2px">table closed</div>
                <div class="text-neutral-500 text-9px mt-1">settling on-chain</div>
                <Show when={settleReason()}>
                  <div class="text-neutral-400 text-9px mt-1">{settleReason()}</div>
                </Show>
              </div>

              <div class="rounded-md border border-neutral-800 bg-zec-surface p-3 mb-3">
                <div class="text-neutral-500 text-9px uppercase tracking-wider mb-2">payout plan</div>
                <For each={settlePlan()}>
                  {(line) => {
                    const isMe = line.seat === mySeat()
                    const zec = (line.amount_zat / 1e8).toFixed(8).replace(/0+$/, '').replace(/\.$/, '')
                    return (
                      <div class="flex items-center justify-between text-10px tabular py-1">
                        <span class={isMe ? 'c-green' : 'text-neutral-400'}>
                          seat {line.seat}{isMe ? ' (you)' : ''} → {line.address.slice(0, 12)}…{line.address.slice(-6)}
                        </span>
                        <span class={isMe ? 'c-zec-yellow font-500' : 'text-neutral-500'}>{zec} ZEC</span>
                      </div>
                    )
                  }}
                </For>
              </div>

              <Show when={settleStatus().phase === 'pending' || settleStatus().phase === 'signing'}>
                <div class="text-center text-9px tabular text-neutral-500 mb-2">
                  {mySeat() === settlePrioritySeat()
                    ? <>you have <span class="c-zec-yellow">{settleFallbackTick()}s</span> to approve before the signer flips to your opponent</>
                    : <>opponent can sign for <span class="c-zec-yellow">{settleFallbackTick()}s</span>; after that you can take over</>
                  }
                </div>
                <Show when={mySeat() === settlePrioritySeat()}>
                  <button
                    class="w-full btn btn-primary text-11px py-2"
                    disabled={settleStatus().phase === 'signing'}
                    onClick={() => {
                      setSettleStatus({ phase: 'signing' })
                      const code = roomCode()
                      const relayUrl = settleFrostRelay() || 'wss://zrelay.rotko.net'
                      void requestPokerSign({
                        relayUrl,
                        roomCode: settleRelayRoom(),
                        plan: settlePlan().map(p => ({ address: p.address, amount_zat: p.amount_zat })),
                        feeZat: 10_000,
                        multisigLabel: `POKER-${code}`,
                      }).then(res => {
                        if (!res.success) {
                          log(`zafu sign failed: ${res.error}`, 'c-red')
                          // back to pending so user can retry; PayoutFailed will land via WS too
                          setSettleStatus({ phase: 'pending' })
                        } else {
                          log('signing shares sent — waiting for escrow to broadcast', 'c-green')
                        }
                      })
                    }}
                  >
                    {settleStatus().phase === 'signing' ? 'signing…' : 'approve payout in zafu'}
                  </button>
                </Show>
                <Show when={mySeat() !== settlePrioritySeat()}>
                  <div class="text-center py-3">
                    <div class="i-lucide-loader-2 animate-spin mx-auto h-5 w-5 text-zec-yellow" />
                    <div class="text-neutral-400 text-10px mt-2">waiting for opponent to approve payout</div>
                  </div>
                </Show>
              </Show>

              <Show when={settleStatus().phase === 'complete'}>
                {(() => {
                  const status = settleStatus()
                  if (status.phase !== 'complete') return null
                  return (
                    <div class="text-center">
                      <div class="text-green-400 text-11px mb-2">✓ paid out on-chain</div>
                      <div class="text-neutral-500 text-9px uppercase mb-1">tx</div>
                      <div class="font-mono text-9px text-zec-yellow break-all px-2 mb-3">{status.txid}</div>
                      <button
                        class="w-full btn btn-secondary text-10px py-1.5"
                        onClick={() => {
                          setView('casino')
                          setSettleStatus({ phase: 'pending' })
                          setSettlePlan([])
                          setSettleRelayRoom('')
                          setSettlePrioritySeat(-1)
                          setSettleReason('')
                          history.replaceState(null, '', '/')
                        }}
                      >return to lobby</button>
                    </div>
                  )
                })()}
              </Show>

              <Show when={settleStatus().phase === 'failed'}>
                {(() => {
                  const status = settleStatus()
                  if (status.phase !== 'failed') return null
                  return (
                    <div class="text-center">
                      <div class="text-red-400 text-11px mb-2">✗ payout failed</div>
                      <div class="text-neutral-400 text-9px mb-3 break-words">{status.reason}</div>
                      <Show when={mySeat() === settlePrioritySeat()}>
                        <button
                          class="w-full btn btn-primary text-11px py-2"
                          onClick={() => setSettleStatus({ phase: 'pending' })}
                        >retry approval</button>
                      </Show>
                    </div>
                  )
                })()}
              </Show>
            </div>
          </Show>

          <div class="text-center py-1.5 text-8px text-neutral-600 uppercase tracking-widest">poker.zk.bot</div>
        </div>
      </div>
    </div>
  )
}
